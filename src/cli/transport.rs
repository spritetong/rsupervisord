// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::platform::AsyncStream;
use std::convert::Infallible;
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Target endpoint address for connecting to the supervisord daemon.
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

    /// Returns true if this endpoint connects via Unix Domain Socket / IPC.
    #[inline]
    pub fn is_ipc(&self) -> bool {
        matches!(self, Self::Ipc(_))
    }

    /// Returns true if this endpoint connects via Windows Named Pipe.
    #[inline]
    pub fn is_named_pipe(&self) -> bool {
        matches!(self, Self::NamedPipe(_))
    }

    /// Returns true if this endpoint connects via TCP.
    #[inline]
    pub fn is_tcp(&self) -> bool {
        matches!(self, Self::Tcp(_))
    }

    /// Returns the system default local IPC endpoint.
    pub fn default_local() -> Self {
        let default_path = crate::platform::native_platform().default_uds_path();
        Self::parse(&default_path.to_string_lossy())
    }

    /// Connects to the daemon endpoint asynchronously.
    pub async fn connect(&self) -> io::Result<StreamTransport> {
        let platform = crate::platform::native_platform();
        match self {
            Self::Ipc(path) => {
                if path.to_string_lossy().starts_with(r"\\.\pipe\") {
                    let stream = platform.connect_named_pipe(path).await?;
                    return Ok(StreamTransport::new(stream));
                }
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

impl FromStr for Endpoint {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::parse(s))
    }
}

/// Returns true when the OS error means the caller is not authorized to open
/// the endpoint. The CLI candidate chain must fail closed on these instead of
/// falling through to the next candidate.
pub fn is_authorization_error(err: &io::Error) -> bool {
    match err.raw_os_error() {
        Some(5)    // Windows ERROR_ACCESS_DENIED
        | Some(13) // Unix EACCES
        | Some(1314) // Windows ERROR_PRIVILEGE_NOT_HELD
        => true,
        _ => err.kind() == io::ErrorKind::PermissionDenied,
    }
}

/// Returns true when the endpoint is simply absent (pipe/file missing or
/// connection refused). Safe to skip when walking a candidate chain.
pub fn is_retryable_not_found(err: &io::Error) -> bool {
    match err.raw_os_error() {
        Some(2)     // ERROR_FILE_NOT_FOUND / ENOENT
        | Some(3)   // ERROR_PATH_NOT_FOUND
        | Some(53)  // ERROR_BAD_NETPATH
        | Some(111) // Unix ECONNREFUSED
        | Some(10061) // WSAECONNREFUSED
        => true,
        _ => matches!(
            err.kind(),
            io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
        ),
    }
}

impl<'a> From<&'a str> for Endpoint {
    fn from(s: &'a str) -> Self {
        Self::parse(s)
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ipc(p) => write!(f, "{}", p.display()),
            Self::NamedPipe(p) => write!(f, "{}", p.display()),
            Self::Tcp(addr) => write!(f, "tcp://{}", addr),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_endpoint_parse() {
        assert_eq!(
            Endpoint::parse("http://127.0.0.1:8500"),
            Endpoint::Tcp("127.0.0.1:8500".to_string())
        );
        assert_eq!(
            Endpoint::parse("tcp://localhost:9001"),
            Endpoint::Tcp("localhost:9001".to_string())
        );
        assert_eq!(
            Endpoint::parse("127.0.0.1:8500"),
            Endpoint::Tcp("127.0.0.1:8500".to_string())
        );
        assert_eq!(
            Endpoint::parse(r"\\.\pipe\testpipe"),
            Endpoint::NamedPipe(PathBuf::from(r"\\.\pipe\testpipe"))
        );
        assert_eq!(
            Endpoint::parse("/tmp/supervisor.sock"),
            Endpoint::Ipc(PathBuf::from("/tmp/supervisor.sock"))
        );
    }

    #[test]
    fn test_endpoint_traits() {
        let ep: Endpoint = "http://127.0.0.1:9001".parse().unwrap();
        assert!(ep.is_tcp());
        assert_eq!(ep.to_string(), "tcp://127.0.0.1:9001");
    }

    #[test]
    fn test_endpoint_default_local() {
        let ep = Endpoint::default_local();
        assert!(matches!(ep, Endpoint::NamedPipe(_) | Endpoint::Ipc(_)));
    }

    #[test]
    fn test_is_authorization_error() {
        // Windows ERROR_ACCESS_DENIED / Unix EACCES / PermissionDenied
        assert!(is_authorization_error(&io::Error::from_raw_os_error(5)));
        assert!(is_authorization_error(&io::Error::from_raw_os_error(13)));
        assert!(is_authorization_error(&io::Error::from_raw_os_error(1314)));
        assert!(is_authorization_error(&io::Error::new(
            io::ErrorKind::PermissionDenied,
            "denied"
        )));
        // Not-found must not be treated as authorization
        assert!(!is_authorization_error(&io::Error::from_raw_os_error(2)));
    }

    #[test]
    fn test_is_retryable_not_found() {
        assert!(is_retryable_not_found(&io::Error::from_raw_os_error(2)));
        assert!(is_retryable_not_found(&io::Error::from_raw_os_error(3)));
        assert!(is_retryable_not_found(&io::Error::from_raw_os_error(111)));
        assert!(is_retryable_not_found(&io::Error::from_raw_os_error(10061)));
        assert!(is_retryable_not_found(&io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "refused"
        )));
        assert!(!is_retryable_not_found(&io::Error::from_raw_os_error(5)));
    }
}
