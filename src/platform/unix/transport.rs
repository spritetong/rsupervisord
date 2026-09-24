// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::error::ProgramError;
use crate::logging::transport::{LogTransport, ProcessStdioHandles, TransportStreams};
use crate::platform::traits::ProcessTransportConfig;
use nix::fcntl::{FcntlArg, FdFlag, OFlag, fcntl};
use nix::unistd::pipe;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
use std::pin::Pin;
use std::process::Stdio;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, ReadBuf};
use tokio::io::unix::AsyncFd;

/// Asynchronous pipe reader wrapping an `AsyncFd<OwnedFd>` for zero-thread reading via epoll/kqueue.
pub struct UnixPipeReader {
    inner: AsyncFd<OwnedFd>,
}

impl UnixPipeReader {
    pub fn new(fd: OwnedFd) -> io::Result<Self> {
        let async_fd = AsyncFd::new(fd)?;
        Ok(Self { inner: async_fd })
    }
}

impl AsyncRead for UnixPipeReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            let mut guard = match self.inner.poll_read_ready(cx) {
                Poll::Ready(Ok(g)) => g,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            };

            let unfilled = buf.initialize_unfilled();
            let raw_fd = self.inner.get_ref().as_raw_fd();
            let res = unsafe {
                libc::read(
                    raw_fd,
                    unfilled.as_mut_ptr() as *mut libc::c_void,
                    unfilled.len(),
                )
            };

            if res >= 0 {
                buf.advance(res as usize);
                return Poll::Ready(Ok(()));
            }

            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::WouldBlock {
                guard.clear_ready();
                continue;
            }
            return Poll::Ready(Err(err));
        }
    }
}

pub struct UnixProcessLogTransport {
    stdio_handles: Option<ProcessStdioHandles>,
    stdout_reader: Option<UnixPipeReader>,
    stderr_reader: Option<UnixPipeReader>,
}

impl UnixProcessLogTransport {
    pub async fn create(config: &ProcessTransportConfig) -> Result<Self, ProgramError> {
        let mut stdio_stdout = None;
        let mut stdio_stderr = None;
        let mut stdout_reader = None;
        let mut stderr_reader = None;

        if config.capture_stdout {
            let (read_fd, write_fd) = pipe().map_err(|e| {
                ProgramError::PlatformError(format!("Failed to create stdout pipe: {}", e))
            })?;

            // Set read end to non-blocking and close-on-exec
            let _ = fcntl(read_fd.as_raw_fd(), FcntlArg::F_SETFL(OFlag::O_NONBLOCK));
            let _ = fcntl(read_fd.as_raw_fd(), FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC));

            let reader = UnixPipeReader::new(read_fd).map_err(|e| {
                ProgramError::PlatformError(format!(
                    "Failed to register stdout pipe with Tokio: {}",
                    e
                ))
            })?;

            if config.redirect_stderr {
                let write_fd_dup = nix::unistd::dup(write_fd.as_raw_fd()).map_err(|e| {
                    ProgramError::PlatformError(format!(
                        "Failed to dup stdout write fd for stderr redirection: {}",
                        e
                    ))
                })?;
                let stderr_file = unsafe { std::fs::File::from_raw_fd(write_fd_dup) };
                stdio_stderr = Some(Stdio::from(stderr_file));
            }

            let stdout_file = unsafe { std::fs::File::from_raw_fd(write_fd.into_raw_fd()) };
            stdio_stdout = Some(Stdio::from(stdout_file));
            stdout_reader = Some(reader);
        }

        if config.capture_stderr && !config.redirect_stderr {
            let (read_fd, write_fd) = pipe().map_err(|e| {
                ProgramError::PlatformError(format!("Failed to create stderr pipe: {}", e))
            })?;

            let _ = fcntl(read_fd.as_raw_fd(), FcntlArg::F_SETFL(OFlag::O_NONBLOCK));
            let _ = fcntl(read_fd.as_raw_fd(), FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC));

            let reader = UnixPipeReader::new(read_fd).map_err(|e| {
                ProgramError::PlatformError(format!(
                    "Failed to register stderr pipe with Tokio: {}",
                    e
                ))
            })?;

            let stderr_file = unsafe { std::fs::File::from_raw_fd(write_fd.into_raw_fd()) };
            stdio_stderr = Some(Stdio::from(stderr_file));
            stderr_reader = Some(reader);
        }

        Ok(Self {
            stdio_handles: Some(ProcessStdioHandles {
                stdout: stdio_stdout,
                stderr: stdio_stderr,
            }),
            stdout_reader,
            stderr_reader,
        })
    }
}

impl LogTransport for UnixProcessLogTransport {
    fn take_child_stdio(&mut self) -> Result<ProcessStdioHandles, ProgramError> {
        self.stdio_handles.take().ok_or_else(|| {
            ProgramError::PlatformError("ProcessStdioHandles already taken".to_string())
        })
    }

    fn into_streams(mut self: Box<Self>) -> Result<TransportStreams, ProgramError> {
        let stdout: Option<Box<dyn AsyncRead + Send + Unpin>> = self
            .stdout_reader
            .take()
            .map(|r| Box::new(r) as Box<dyn AsyncRead + Send + Unpin>);

        let stderr: Option<Box<dyn AsyncRead + Send + Unpin>> = self
            .stderr_reader
            .take()
            .map(|r| Box::new(r) as Box<dyn AsyncRead + Send + Unpin>);

        Ok(TransportStreams { stdout, stderr })
    }
}
