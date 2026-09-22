// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::program::config::{HealthCheckConfig, HealthCheckType};
use crate::program::state::HealthStatus;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Events dispatched by the health probe runner back to the program actor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthEvent {
    Healthy,
    Unhealthy,
}

/// Asynchronous runner executing active health probes (HTTP, TCP, or Exec).
pub struct HealthProbeRunner {
    name: String,
    config: HealthCheckConfig,
    directory: Option<PathBuf>,
    event_tx: mpsc::Sender<HealthEvent>,
    cancel_token: CancellationToken,
}

impl HealthProbeRunner {
    pub fn new(
        name: impl Into<String>,
        config: HealthCheckConfig,
        directory: Option<PathBuf>,
        event_tx: mpsc::Sender<HealthEvent>,
        cancel_token: CancellationToken,
    ) -> Self {
        Self {
            name: name.into(),
            config,
            directory,
            event_tx,
            cancel_token,
        }
    }

    /// Spawns the health probe background loop.
    pub async fn run(self) {
        // 1. Initial delay grace period before probing starts
        if self.config.initial_delay_secs > 0 {
            tokio::select! {
                biased;
                _ = self.cancel_token.cancelled() => return,
                _ = tokio::time::sleep(Duration::from_secs(self.config.initial_delay_secs)) => {}
            }
        }

        let mut consecutive_failures = 0u32;
        let mut last_reported = HealthStatus::Starting;
        let interval = Duration::from_secs(self.config.interval_secs.max(1));
        let timeout_dur = Duration::from_secs(self.config.timeout_secs.max(1));

        loop {
            tokio::select! {
                biased;
                _ = self.cancel_token.cancelled() => break,
                _ = tokio::time::sleep(interval) => {
                    let is_ok = match &self.config.check_type {
                        HealthCheckType::Http { url, expected_status } => {
                            Self::check_http(url, *expected_status, timeout_dur).await
                        }
                        HealthCheckType::Tcp { endpoint } => {
                            Self::check_tcp(endpoint, timeout_dur).await
                        }
                        HealthCheckType::Exec { command } => {
                            Self::check_exec(command, self.directory.as_deref(), timeout_dur).await
                        }
                    };

                    if is_ok {
                        consecutive_failures = 0;
                        if last_reported != HealthStatus::Healthy {
                            last_reported = HealthStatus::Healthy;
                            let _ = self.event_tx.send(HealthEvent::Healthy).await;
                        }
                    } else {
                        consecutive_failures += 1;
                        tracing::warn!(
                            program = %self.name,
                            failures = consecutive_failures,
                            threshold = self.config.failure_threshold,
                            "Health check probe failed"
                        );

                        if consecutive_failures >= self.config.failure_threshold
                            && last_reported != HealthStatus::Unhealthy
                        {
                            last_reported = HealthStatus::Unhealthy;
                            let _ = self.event_tx.send(HealthEvent::Unhealthy).await;
                        }
                    }
                }
            }
        }
    }

    /// TCP port connectivity probe with strict timeout.
    async fn check_tcp(endpoint: &str, timeout_dur: Duration) -> bool {
        matches!(
            tokio::time::timeout(timeout_dur, tokio::net::TcpStream::connect(endpoint)).await,
            Ok(Ok(_))
        )
    }

    /// HTTP GET probe with status code verification.
    async fn check_http(url_str: &str, expected_status: u16, timeout_dur: Duration) -> bool {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let url_no_proto = if let Some(rest) = url_str.strip_prefix("http://") {
            rest
        } else {
            return false;
        };

        let mut parts = url_no_proto.splitn(2, '/');
        let host_port = parts.next().unwrap_or("");
        let path = format!("/{}", parts.next().unwrap_or(""));

        let addr = if host_port.contains(':') {
            host_port.to_string()
        } else {
            format!("{}:80", host_port)
        };

        let connect_fut = tokio::net::TcpStream::connect(&addr);
        let mut stream = match tokio::time::timeout(timeout_dur, connect_fut).await {
            Ok(Ok(s)) => s,
            _ => return false,
        };

        let req = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nUser-Agent: rsupervisord-healthcheck\r\n\r\n",
            path, host_port
        );

        let io_fut = async {
            stream.write_all(req.as_bytes()).await?;
            stream.flush().await?;
            let mut buf = [0u8; 512];
            let n = stream.read(&mut buf).await?;
            Ok::<_, std::io::Error>((buf, n))
        };

        match tokio::time::timeout(timeout_dur, io_fut).await {
            Ok(Ok((buf, n))) if n > 0 => {
                let resp_str = String::from_utf8_lossy(&buf[..n]);
                let first_line = resp_str.lines().next().unwrap_or("");
                let status_code = first_line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|s| s.parse::<u16>().ok())
                    .unwrap_or(0);
                status_code == expected_status
            }
            _ => false,
        }
    }

    /// Shell execution probe verifying 0 exit code with strict timeout.
    async fn check_exec(command: &str, directory: Option<&Path>, timeout_dur: Duration) -> bool {
        let mut cmd = crate::platform::native_platform().build_shell_command(command);
        if let Some(dir) = directory {
            cmd.current_dir(dir);
        }
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());

        match tokio::time::timeout(timeout_dur, cmd.status()).await {
            Ok(Ok(status)) => status.success(),
            _ => false,
        }
    }
}
