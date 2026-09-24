// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::error::ProgramError;
use crate::logging::transport::{LogTransport, ProcessStdioHandles, TransportStreams};
use crate::platform::traits::ProcessTransportConfig;
use std::fs::OpenOptions;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_WRITE_THROUGH;

static PIPE_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Windows process log transport leveraging Overlapped Named Pipes.
///
/// Unlike standard anonymous pipes (`Stdio::piped()`) which Tokio maps to synchronous
/// `ReadFile` calls inside dedicated blocking threads (`spawn_blocking`), Overlapped
/// Named Pipes integrate natively with Tokio's I/O Completion Port (IOCP) reactor,
/// completely eliminating the 2-thread leak per supervised child process.
pub struct WindowsProcessLogTransport {
    stdio_handles: Option<ProcessStdioHandles>,
    stdout_server: Option<NamedPipeServer>,
    stderr_server: Option<NamedPipeServer>,
}

impl WindowsProcessLogTransport {
    pub async fn create(config: &ProcessTransportConfig) -> Result<Self, ProgramError> {
        let mut stdio_stdout = None;
        let mut stdio_stderr = None;
        let mut stdout_server = None;
        let mut stderr_server = None;

        let pid = std::process::id();

        if config.capture_stdout {
            let seq = PIPE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let pipe_name = format!(
                r"\\.\pipe\rsupervisord-{}-{}-stdout-{:x}",
                pid, config.program_name, seq
            );

            let server = ServerOptions::new()
                .first_pipe_instance(true)
                .create(&pipe_name)
                .map_err(|e| {
                    ProgramError::PlatformError(format!(
                        "Failed to create stdout named pipe server '{}': {}",
                        pipe_name, e
                    ))
                })?;

            let client_file = OpenOptions::new()
                .read(false)
                .write(true)
                .custom_flags(FILE_FLAG_WRITE_THROUGH)
                .open(&pipe_name)
                .map_err(|e| {
                    ProgramError::PlatformError(format!(
                        "Failed to open stdout named pipe client '{}': {}",
                        pipe_name, e
                    ))
                })?;

            let ok = unsafe {
                SetHandleInformation(
                    client_file.as_raw_handle() as _,
                    HANDLE_FLAG_INHERIT,
                    HANDLE_FLAG_INHERIT,
                )
            };
            if ok == 0 {
                let err = std::io::Error::last_os_error();
                return Err(ProgramError::PlatformError(format!(
                    "Failed to set handle inheritance for stdout pipe: {}",
                    err
                )));
            }

            // Await pipe connection from client (which succeeds immediately as client is already open)
            server.connect().await.map_err(|e| {
                ProgramError::PlatformError(format!("Failed to connect stdout named pipe: {}", e))
            })?;

            if config.redirect_stderr {
                // If stderr is redirected, clone client_file so both stdout and stderr write to the same pipe
                let stderr_client = client_file.try_clone().map_err(|e| {
                    ProgramError::PlatformError(format!(
                        "Failed to clone stdout client handle for stderr redirection: {}",
                        e
                    ))
                })?;
                let ok = unsafe {
                    SetHandleInformation(
                        stderr_client.as_raw_handle() as _,
                        HANDLE_FLAG_INHERIT,
                        HANDLE_FLAG_INHERIT,
                    )
                };
                if ok == 0 {
                    let err = std::io::Error::last_os_error();
                    return Err(ProgramError::PlatformError(format!(
                        "Failed to set handle inheritance for redirected stderr pipe: {}",
                        err
                    )));
                }
                stdio_stderr = Some(Stdio::from(stderr_client));
            }

            stdio_stdout = Some(Stdio::from(client_file));
            stdout_server = Some(server);
        }

        if config.capture_stderr && !config.redirect_stderr {
            let seq = PIPE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let pipe_name = format!(
                r"\\.\pipe\rsupervisord-{}-{}-stderr-{:x}",
                pid, config.program_name, seq
            );

            let server = ServerOptions::new()
                .first_pipe_instance(true)
                .create(&pipe_name)
                .map_err(|e| {
                    ProgramError::PlatformError(format!(
                        "Failed to create stderr named pipe server '{}': {}",
                        pipe_name, e
                    ))
                })?;

            let client_file = OpenOptions::new()
                .read(false)
                .write(true)
                .custom_flags(FILE_FLAG_WRITE_THROUGH)
                .open(&pipe_name)
                .map_err(|e| {
                    ProgramError::PlatformError(format!(
                        "Failed to open stderr named pipe client '{}': {}",
                        pipe_name, e
                    ))
                })?;

            let ok = unsafe {
                SetHandleInformation(
                    client_file.as_raw_handle() as _,
                    HANDLE_FLAG_INHERIT,
                    HANDLE_FLAG_INHERIT,
                )
            };
            if ok == 0 {
                let err = std::io::Error::last_os_error();
                return Err(ProgramError::PlatformError(format!(
                    "Failed to set handle inheritance for stderr pipe: {}",
                    err
                )));
            }

            server.connect().await.map_err(|e| {
                ProgramError::PlatformError(format!("Failed to connect stderr named pipe: {}", e))
            })?;

            stdio_stderr = Some(Stdio::from(client_file));
            stderr_server = Some(server);
        }

        Ok(Self {
            stdio_handles: Some(ProcessStdioHandles {
                stdout: stdio_stdout,
                stderr: stdio_stderr,
            }),
            stdout_server,
            stderr_server,
        })
    }
}

impl LogTransport for WindowsProcessLogTransport {
    fn take_child_stdio(&mut self) -> Result<ProcessStdioHandles, ProgramError> {
        self.stdio_handles.take().ok_or_else(|| {
            ProgramError::PlatformError("ProcessStdioHandles already taken".to_string())
        })
    }

    fn into_streams(mut self: Box<Self>) -> Result<TransportStreams, ProgramError> {
        let stdout: Option<Box<dyn tokio::io::AsyncRead + Send + Unpin>> = self
            .stdout_server
            .take()
            .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>);

        let stderr: Option<Box<dyn tokio::io::AsyncRead + Send + Unpin>> = self
            .stderr_server
            .take()
            .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>);

        Ok(TransportStreams { stdout, stderr })
    }
}
