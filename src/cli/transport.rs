// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use crate::platform::AsyncStream;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Target endpoint address for connecting to the rsupervisord daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// Unix Domain Socket / AF_UNIX local IPC.
    Ipc(PathBuf),
    /// Windows Named Pipe (\\.\pipe\...).
    NamedPipe(PathBuf),
    /// TCP socket address or URL.
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
            Self::NamedPipe(PathBuf::from(trimmed))
        } else if trimmed.contains(':') && !trimmed.contains('/') && !trimmed.contains('\\') {
            Self::Tcp(trimmed.to_string())
        } else {
            Self::Ipc(PathBuf::from(trimmed))
        }
    }

    /// Returns the system default local IPC endpoint.
    pub fn default_local() -> Self {
        let default_path = crate::platform::native_platform().default_uds_path();
        Self::Ipc(default_path)
    }

    /// Connects to the daemon endpoint asynchronously.
    pub async fn connect(&self) -> io::Result<StreamTransport> {
        let platform = crate::platform::native_platform();
        match self {
            Self::Ipc(path) => {
                let stream = platform.connect_ipc(path).await?;
                Ok(StreamTransport::new(stream))
            }
            Self::NamedPipe(path) => {
                let stream = platform.connect_named_pipe(path).await?;
                Ok(StreamTransport::new(stream))
            }
            Self::Tcp(addr) => {
                let stream = tokio::net::TcpStream::connect(addr).await?;
                Ok(StreamTransport::new(Box::new(stream)))
            }
        }
    }
}

/// Unified stream transport implementing AsyncRead and AsyncWrite across platforms.
pub struct StreamTransport(Box<dyn AsyncStream>);

impl StreamTransport {
    pub fn new(stream: Box<dyn AsyncStream>) -> Self {
        Self(stream)
    }
}

impl AsyncRead for StreamTransport {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for StreamTransport {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}
