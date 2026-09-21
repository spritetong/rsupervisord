// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use clap::Parser;
use std::io::Write;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

use crate::config::SupervisorConfig;
use crate::manager::SupervisorManager;
use crate::server::ServerEngine;

/// Command-line arguments for the rsupervisord daemon.
#[derive(Parser, Debug, Clone)]
#[command(
    name = "rsupervisord",
    version,
    about = "Next-generation process supervision daemon"
)]
pub struct DaemonArgs {
    /// Path to YAML configuration file
    #[arg(short = 'c', long = "config")]
    pub config: Option<PathBuf>,

    /// Run daemon in the foreground (default: true)
    #[arg(short = 'n', long = "nodaemon")]
    pub nodaemon: bool,

    /// Log level filter (trace, debug, info, warn, error, off)
    #[arg(short = 'l', long = "loglevel", default_value = "info")]
    pub loglevel: String,

    /// Number of worker threads for Tokio runtime (1 = single-threaded current_thread)
    #[arg(long = "worker-threads")]
    pub worker_threads: Option<usize>,

    /// Install as a system service
    #[arg(long = "install")]
    pub install: bool,

    /// Uninstall the system service
    #[arg(long = "uninstall")]
    pub uninstall: bool,

    /// Start the system service
    #[arg(long = "start")]
    pub start: bool,

    /// Stop the system service
    #[arg(long = "stop")]
    pub stop: bool,

    /// Restart the system service
    #[arg(long = "restart")]
    pub restart: bool,

    /// Run as a Windows service (invoked by SCM)
    #[cfg(windows)]
    #[arg(long = "service")]
    pub service: bool,
}

/// Runs the supervisor daemon lifecycle to completion.
///
/// If `external_cancel` is supplied (e.g. from a Windows Service Control Manager handler),
/// cancellation will trigger graceful daemon teardown alongside native OS signals.
pub async fn run_daemon(
    args: DaemonArgs,
    config_path: PathBuf,
    external_cancel: Option<CancellationToken>,
) -> anyhow::Result<()> {
    if !config_path.exists() {
        anyhow::bail!(
            "Configuration file not found: {:?}. Please specify a valid file using -c/--config.",
            config_path
        );
    }

    let config = match SupervisorConfig::from_file(&config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load configuration from {:?}: {}", config_path, e);
            return Err(e.into());
        }
    };

    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    // Check if logging is completely disabled
    let is_logging_disabled = !config.logging.enabled
        || args.loglevel.to_lowercase() == "off"
        || config.logging.level.to_lowercase() == "off";

    if is_logging_disabled {
        let filter = tracing_subscriber::EnvFilter::new("off");
        let _ = tracing_subscriber::registry().with(filter).try_init();
    } else {
        // Initialize logging subscriber with console and optional rotating file output
        let log_level = if args.loglevel != "info" {
            &args.loglevel
        } else {
            &config.logging.level
        };

        let filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(log_level));

        let console_layer = tracing_subscriber::fmt::layer().with_target(false);

        if let Some(ref log_file) = config.logging.file {
            if let Some(parent) = log_file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let max_bytes = match &config.logging.max_bytes {
                Some(s) => crate::logging::parse_byte_size(s).unwrap_or(20 * 1024 * 1024),
                None => 20 * 1024 * 1024,
            };
            let file_rotator = file_rotate::FileRotate::new(
                log_file,
                file_rotate::suffix::AppendCount::new(config.logging.backups),
                file_rotate::ContentLimit::Bytes(max_bytes),
                file_rotate::compression::Compression::None,
                None,
            );
            let file_writer_arc = std::sync::Arc::new(std::sync::Mutex::new(file_rotator));
            let make_writer = move || MutexWriter(file_writer_arc.clone());

            let file_layer = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_target(false)
                .with_writer(make_writer);

            let _ = tracing_subscriber::registry()
                .with(filter)
                .with(console_layer)
                .with(file_layer)
                .try_init();
        } else {
            let _ = tracing_subscriber::registry()
                .with(filter)
                .with(console_layer)
                .try_init();
        }
    }

    let cmd_name = crate::config::paths::get_cmd_name();
    tracing::info!(
        "Starting {} v{} (elevated: {})",
        cmd_name,
        env!("CARGO_PKG_VERSION"),
        crate::platform::native_platform().is_elevated()
    );

    let mut manager = SupervisorManager::new(&config)?;
    let manager_handle = manager.handle();
    tracing::info!(
        "Supervisor manager initialized with {} program(s)",
        config.programs.len()
    );

    // Initial autostart sequence according to dependency graph
    if let Err(e) = manager_handle.start_all().await {
        tracing::warn!("Failed during initial autostart sequence: {}", e);
    }

    // Spawn server engine
    let cancel_token = CancellationToken::new();
    let server = ServerEngine::new(manager_handle, Some(config_path), config.server);
    let server_token = cancel_token.clone();
    let server_handle = tokio::spawn(async move {
        if let Err(e) = server.run(server_token).await {
            tracing::error!("ServerEngine terminated with error: {}", e);
        }
    });

    if let Some(ext) = external_cancel {
        let ct = cancel_token.clone();
        tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = ext.cancelled() => {
                    ct.cancel();
                }
                _ = ct.cancelled() => {}
            }
        });
    }

    // Wait cooperatively for OS shutdown signal (Ctrl+C on Windows, SIGTERM/SIGINT on Unix)
    tokio::select! {
        _ = crate::platform::wait_for_shutdown_signal() => {
            tracing::info!("Shutdown signal received, initiating graceful shutdown...");
        }
        _ = cancel_token.cancelled() => {
            tracing::info!("Daemon cancellation triggered");
        }
    }

    // Graceful teardown
    cancel_token.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(3), server_handle).await;

    tracing::info!("Stopping all supervised processes...");
    if let Err(e) = manager.shutdown().await {
        tracing::error!("Error shutting down manager: {}", e);
    }

    tracing::info!("{} shutdown cleanly", cmd_name);
    Ok(())
}

struct MutexWriter(
    std::sync::Arc<std::sync::Mutex<file_rotate::FileRotate<file_rotate::suffix::AppendCount>>>,
);

impl Write for MutexWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|e| std::io::Error::other(e.to_string()))?
            .write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0
            .lock()
            .map_err(|e| std::io::Error::other(e.to_string()))?
            .flush()
    }
}
