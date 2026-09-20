// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

pub mod traits;
#[cfg(unix)]
pub mod unix;
#[cfg(windows)]
pub mod windows;

pub use traits::{PlatformBackend, PlatformProcessGuard};

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
        let mut sigterm =
            signal(SignalKind::terminate()).expect("Failed to register SIGTERM handler");
        let mut sigint =
            signal(SignalKind::interrupt()).expect("Failed to register SIGINT handler");

        tokio::select! {
            _ = sigterm.recv() => {
                tracing::info!("Received SIGTERM, initiating graceful shutdown");
            }
            _ = sigint.recv() => {
                tracing::info!("Received SIGINT (Ctrl+C), initiating graceful shutdown");
            }
        }
    }
    #[cfg(windows)]
    {
        match tokio::signal::ctrl_c().await {
            Ok(()) => {
                tracing::info!("Received Ctrl+C, initiating graceful shutdown");
            }
            Err(err) => {
                tracing::error!("Failed to listen for Ctrl+C signal: {}", err);
            }
        }
    }
}
