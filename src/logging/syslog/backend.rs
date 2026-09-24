// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::error::ProgramError;
use crate::logging::backend::LogBackend;
use crate::logging::destination::{SyslogFacility, SyslogSeverity, SyslogTarget};
use crate::logging::types::LogChunk;
use async_trait::async_trait;

#[cfg(unix)]
use crate::logging::destination::SyslogProto;
#[cfg(unix)]
use crate::logging::syslog::encoder::format_rfc3164;
#[cfg(unix)]
use tokio::sync::mpsc;

#[cfg(unix)]
fn get_local_hostname() -> String {
    nix::unistd::gethostname()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "localhost".to_string())
}

#[cfg(not(unix))]
fn get_local_hostname() -> String {
    "localhost".to_string()
}

/// Asynchronous RFC 3164 Syslog backend.
pub struct SyslogLogBackend {
    #[allow(dead_code)]
    target: SyslogTarget,
    #[allow(dead_code)]
    facility: SyslogFacility,
    #[allow(dead_code)]
    severity: SyslogSeverity,
    #[allow(dead_code)]
    tag: String,
    #[allow(dead_code)]
    hostname: String,
    #[cfg(unix)]
    sender: Option<SyslogSender>,
}

#[cfg(unix)]
enum SyslogSender {
    UnixDatagram(tokio::net::UnixDatagram),
    Udp(tokio::net::UdpSocket),
    Tcp(mpsc::Sender<Vec<u8>>),
}

impl SyslogLogBackend {
    /// Creates a new `SyslogLogBackend`.
    ///
    /// On Windows, configuring syslog is strictly unsupported and immediately returns
    /// `ProgramError::ConfigError` in compliance with the "fail loud" policy.
    pub fn new(
        target: SyslogTarget,
        facility: SyslogFacility,
        severity: SyslogSeverity,
        tag: impl Into<String>,
    ) -> Result<Self, ProgramError> {
        let tag = tag.into();
        let hostname = get_local_hostname();

        #[cfg(windows)]
        {
            let _ = (facility, severity, tag, hostname);
            Err(ProgramError::ConfigError(format!(
                "Syslog destination '{}' is not supported on Windows.",
                target
            )))
        }

        #[cfg(unix)]
        {
            let sender = match &target {
                SyslogTarget::Local => {
                    let candidates = ["/dev/log", "/var/run/syslog", "/var/run/log"];
                    let mut datagram_opt = None;
                    for path in candidates {
                        if std::path::Path::new(path).exists()
                            && let Ok(std_sock) = std::os::unix::net::UnixDatagram::unbound()
                        {
                            let _ = std_sock.set_nonblocking(true);
                            if std_sock.connect(path).is_ok()
                                && let Ok(tokio_sock) = tokio::net::UnixDatagram::from_std(std_sock)
                            {
                                datagram_opt = Some(SyslogSender::UnixDatagram(tokio_sock));
                                break;
                            }
                        }
                    }
                    if datagram_opt.is_none() {
                        tracing::warn!(
                            "No local syslog socket reachable at /dev/log, /var/run/syslog, or /var/run/log"
                        );
                    }
                    datagram_opt
                }
                SyslogTarget::Remote { proto, host, port } => match proto {
                    SyslogProto::Udp => {
                        let addr = format!("{}:{}", host, port);
                        let bind_addr = if host.contains(':') { "[::]:0" } else { "0.0.0.0:0" };
                        let std_sock = std::net::UdpSocket::bind(bind_addr).map_err(|e| {
                            ProgramError::PlatformError(format!(
                                "Failed to bind UDP socket for syslog '{}': {}",
                                addr, e
                            ))
                        })?;
                        std_sock.set_nonblocking(true).map_err(|e| {
                            ProgramError::PlatformError(format!(
                                "Failed to set nonblocking for syslog UDP socket: {}",
                                e
                            ))
                        })?;
                        std_sock.connect(&addr).map_err(|e| {
                            ProgramError::PlatformError(format!(
                                "Failed to connect syslog UDP to '{}': {}",
                                addr, e
                            ))
                        })?;
                        let tokio_sock = tokio::net::UdpSocket::from_std(std_sock).map_err(|e| {
                            ProgramError::PlatformError(format!(
                                "Failed to convert UDP socket to Tokio: {}",
                                e
                            ))
                        })?;
                        Some(SyslogSender::Udp(tokio_sock))
                    }
                    SyslogProto::Tcp => {
                        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(1024);
                        let addr = format!("{}:{}", host, port);
                        tokio::spawn(async move {
                            use tokio::io::AsyncWriteExt;
                            let mut stream: Option<tokio::net::TcpStream> = None;
                            while let Some(msg) = rx.recv().await {
                                if stream.is_none() {
                                    stream = tokio::net::TcpStream::connect(&addr).await.ok();
                                }
                                if let Some(ref mut s) = stream
                                    && s.write_all(&msg).await.is_err()
                                {
                                    stream = None;
                                }
                            }
                        });
                        Some(SyslogSender::Tcp(tx))
                    }
                },
            };

            Ok(Self {
                target,
                facility,
                severity,
                tag,
                hostname,
                sender,
            })
        }
    }
}

#[async_trait]
impl LogBackend for SyslogLogBackend {
    async fn write_chunk(&self, chunk: &LogChunk) -> Result<(), ProgramError> {
        #[cfg(windows)]
        {
            let _ = chunk;
            Ok(())
        }

        #[cfg(unix)]
        {
            let sender = match &self.sender {
                Some(s) => s,
                None => return Ok(()),
            };

            let text = String::from_utf8_lossy(&chunk.data);
            for line in text.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                let packet = format_rfc3164(
                    self.facility,
                    self.severity,
                    chunk.timestamp,
                    &self.hostname,
                    &self.tag,
                    line,
                );

                match sender {
                    SyslogSender::UnixDatagram(s) => {
                        let _ = s.send(&packet).await;
                    }
                    SyslogSender::Udp(s) => {
                        let _ = s.send(&packet).await;
                    }
                    SyslogSender::Tcp(tx) => {
                        let _ = tx.try_send(packet);
                    }
                }
            }

            Ok(())
        }
    }

    async fn flush(&self) -> Result<(), ProgramError> {
        Ok(())
    }
}
