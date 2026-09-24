// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

pub mod traits;
#[cfg(unix)]
pub mod unix;
#[cfg(windows)]
pub mod windows;

pub use traits::{
    AsyncStream, PlatformBackend, PlatformIpcListener, PlatformProcessGuard, PlatformService,
    ProcessMetrics, ProcessTransportConfig, abs_path, norm_path,
};

/// Returns the native platform backend singleton.
pub fn native_platform() -> &'static dyn PlatformBackend {
    #[cfg(unix)]
    {
        &unix::UnixPlatformBackend
    }
    #[cfg(windows)]
    {
        &windows::WindowsPlatformBackend
    }
}

/// Asynchronously waits for an OS termination signal (Ctrl+C on Windows, SIGTERM or SIGINT on Unix).
pub async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm = match signal(SignalKind::terminate()) {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::error!("Failed to register SIGTERM handler: {}", e);
                None
            }
        };
        let mut sigint = match signal(SignalKind::interrupt()) {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::error!("Failed to register SIGINT handler: {}", e);
                None
            }
        };

        tokio::select! {
            _ = async {
                if let Some(ref mut s) = sigterm {
                    s.recv().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {
                tracing::info!("Received SIGTERM, initiating graceful shutdown");
            }
            _ = async {
                if let Some(ref mut s) = sigint {
                    s.recv().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {
                tracing::info!("Received SIGINT (Ctrl+C), initiating graceful shutdown");
            }
        }
    }
    #[cfg(windows)]
    {
        windows::wait_for_windows_shutdown_signal().await;
    }
}
