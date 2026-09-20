// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Target endpoint address for connecting to the rsupervisord daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    #[cfg(unix)]
    Unix(PathBuf),
    #[cfg(windows)]
    NamedPipe(PathBuf),
    #[cfg(windows)]
    WindowsUds(PathBuf),
    Tcp(String),
}

impl Endpoint {
    /// Parses an endpoint string or URL into an Endpoint instance.
    pub fn parse(s: &str) -> Self {
        let trimmed = s.trim();
        if let Some(rest) = trimmed
            .strip_prefix("http://")
            .or_else(|| trimmed.strip_prefix("tcp://"))
        {
            Self::Tcp(rest.to_string())
        } else if trimmed.starts_with(r"\\.\pipe\") {
            #[cfg(windows)]
            {
                Self::NamedPipe(PathBuf::from(trimmed))
            }
            #[cfg(not(windows))]
            {
                Self::Tcp(trimmed.to_string())
            }
        } else if trimmed.contains(':') && !trimmed.contains('/') && !trimmed.contains('\\') {
            Self::Tcp(trimmed.to_string())
        } else {
            #[cfg(windows)]
            {
                if trimmed.starts_with(r"\\.\pipe\") {
                    Self::NamedPipe(PathBuf::from(trimmed))
                } else {
                    Self::WindowsUds(PathBuf::from(trimmed))
                }
            }
            #[cfg(unix)]
            {
                Self::Unix(PathBuf::from(trimmed))
            }
        }
    }

    /// Returns the system default local IPC endpoint.
    pub fn default_local() -> Self {
        let default_path = crate::platform::native_platform().default_uds_path();
        #[cfg(windows)]
        {
            Self::NamedPipe(default_path)
        }
        #[cfg(unix)]
        {
            Self::Unix(default_path)
        }
    }

    /// Connects to the daemon endpoint asynchronously.
    pub async fn connect(&self) -> io::Result<StreamTransport> {
        match self {
            #[cfg(unix)]
            Self::Unix(path) => {
                let stream = tokio::net::UnixStream::connect(path).await?;
                Ok(StreamTransport::Unix(stream))
            }
            #[cfg(windows)]
            Self::NamedPipe(path) => {
                let client = tokio::net::windows::named_pipe::ClientOptions::new().open(path)?;
                Ok(StreamTransport::NamedPipe(client))
            }
            #[cfg(windows)]
            Self::WindowsUds(path) => {
                use std::os::windows::io::{FromRawSocket, IntoRawSocket};
                let std_stream = uds_windows::UnixStream::connect(path)?;
                let raw = std_stream.into_raw_socket();
                let std_tcp = unsafe { std::net::TcpStream::from_raw_socket(raw) };
                std_tcp.set_nonblocking(true)?;
                let stream = tokio::net::TcpStream::from_std(std_tcp)?;
                Ok(StreamTransport::Tcp(stream))
            }
            Self::Tcp(addr) => {
                let stream = tokio::net::TcpStream::connect(addr).await?;
                Ok(StreamTransport::Tcp(stream))
            }
        }
    }
}

/// Unified stream transport implementing AsyncRead and AsyncWrite across platforms.
pub enum StreamTransport {
    #[cfg(unix)]
    Unix(tokio::net::UnixStream),
    #[cfg(windows)]
    NamedPipe(tokio::net::windows::named_pipe::NamedPipeClient),
    Tcp(tokio::net::TcpStream),
}

impl AsyncRead for StreamTransport {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            #[cfg(unix)]
            Self::Unix(s) => Pin::new(s).poll_read(cx, buf),
            #[cfg(windows)]
            Self::NamedPipe(s) => Pin::new(s).poll_read(cx, buf),
            Self::Tcp(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for StreamTransport {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            #[cfg(unix)]
            Self::Unix(s) => Pin::new(s).poll_write(cx, buf),
            #[cfg(windows)]
            Self::NamedPipe(s) => Pin::new(s).poll_write(cx, buf),
            Self::Tcp(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            #[cfg(unix)]
            Self::Unix(s) => Pin::new(s).poll_flush(cx),
            #[cfg(windows)]
            Self::NamedPipe(s) => Pin::new(s).poll_flush(cx),
            Self::Tcp(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            #[cfg(unix)]
            Self::Unix(s) => Pin::new(s).poll_shutdown(cx),
            #[cfg(windows)]
            Self::NamedPipe(s) => Pin::new(s).poll_shutdown(cx),
            Self::Tcp(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}
