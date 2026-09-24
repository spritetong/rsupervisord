// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::config::SupervisorConfig;
use crate::manager::SupervisorManager;
use crate::server::ServerEngine;
use crate::service::ServiceOp;
use clap::{Parser, Subcommand};
use std::io::Write;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

/// Command-line arguments for the rsupervisord daemon.
#[derive(Parser, Debug, Clone)]
#[command(
    name = "supervisord",
    version,
    about = env!("CARGO_PKG_DESCRIPTION")
)]
pub struct DaemonArgs {
    /// Path to YAML configuration file
    #[arg(short = 'c', long = "config", global = true)]
    pub config: Option<PathBuf>,

    /// Run daemon in the foreground (default: true)
    #[arg(short = 'n', long = "nodaemon")]
    pub nodaemon: bool,

    /// Log level filter (trace, debug, info, warn, error, off)
    #[arg(short = 'l', long = "loglevel", default_value_t = crate::consts::DEFAULT_LOG_LEVEL.to_string())]
    pub loglevel: String,

    /// Number of worker threads for Tokio runtime (1 = single-threaded current_thread)
    #[arg(long = "worker-threads")]
    pub worker_threads: Option<usize>,

    /// Run as a system service (e.g. Windows SCM)
    #[arg(long = "service")]
    pub service: bool,

    /// Allow non-elevated callers to connect to daemon IPC when daemon is running elevated
    #[arg(long = "allow-unelevated")]
    pub allow_unelevated: bool,

    /// Service lifecycle management (e.g. 'service install')
    #[command(subcommand)]
    pub action: Option<DaemonAction>,
}

/// Top-level subcommands of the daemon binary.
#[derive(Subcommand, Debug, Clone)]
pub enum DaemonAction {
    /// Manage the system service (install/uninstall/start/stop/restart)
    Service {
        #[command(subcommand)]
        op: ServiceOp,
    },
}

/// Orchestrator for the rsupervisord daemon process lifecycle.
pub struct SupervisorDaemon {
    args: DaemonArgs,
    config_path: PathBuf,
    external_cancel: Option<CancellationToken>,
}

impl SupervisorDaemon {
    /// Creates a new SupervisorDaemon instance for the given arguments and configuration path.
    pub fn new(args: DaemonArgs, config_path: PathBuf) -> Self {
        Self {
            args,
            config_path,
            external_cancel: None,
        }
    }

    /// Attaches an external cancellation token (e.g. from Windows SCM or signal handler).
    pub fn with_external_cancel(mut self, cancel: Option<CancellationToken>) -> Self {
        self.external_cancel = cancel;
        self
    }

    /// Runs the supervisor daemon lifecycle to completion.
    pub async fn run(self) -> anyhow::Result<()> {
        let SupervisorDaemon {
            args,
            config_path,
            external_cancel,
        } = self;

        if !config_path.exists() {
            anyhow::bail!(
                "Configuration file not found: {:?}. Please specify a valid file using -c/--config.",
                config_path
            );
        }

        let mut config = match SupervisorConfig::from_file(&config_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Failed to load configuration from {:?}: {}", config_path, e);
                return Err(e.into());
            }
        };

        if args.allow_unelevated {
            config.server.allow_unelevated = true;
        }
        // CLI -n/--nodaemon forces foreground. Daemonize for the false case is
        // not yet implemented (Python parity double-fork).
        // TODO: Python parity double-fork daemonize when nodaemon is false.
        if args.nodaemon {
            config.nodaemon = true;
        }

        // Initialize tracing before any early startup logs so environment
        // application, rlimit, and pidfile messages are not lost to the
        // default no-op subscriber.
        init_tracing(&args, &config);

        let cmd_name = crate::config::paths::get_cmd_name();

        // OI-6: apply `[supervisord] environment` at daemon startup (not parse time),
        // before any children spawn so they inherit the expanded environment.
        for (k, v) in &config.environment {
            tracing::debug!(key = %k, "applying supervisord environment");
            unsafe {
                std::env::set_var(k, v);
            }
        }

        // OI-8: raise soft rlimits best-effort (Unix only; Windows has no rlimit).
        #[cfg(unix)]
        {
            if let Some(minfds) = config.minfds {
                apply_rlimit(
                    nix::sys::resource::Resource::RLIMIT_NOFILE,
                    minfds,
                    "minfds",
                );
            }
            if let Some(minprocs) = config.minprocs {
                apply_rlimit(
                    nix::sys::resource::Resource::RLIMIT_NPROC,
                    minprocs,
                    "minprocs",
                );
            }
        }

        // OI-8: write pidfile on startup; scopeguard removes it on any exit path.
        let pidfile_guard = config.pidfile.clone().map(|pidfile| {
            if let Some(parent) = pidfile.parent()
                && !parent.as_os_str().is_empty()
            {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::write(&pidfile, format!("{}\n", std::process::id())) {
                Ok(()) => tracing::info!("Wrote pidfile {:?}", pidfile),
                Err(e) => tracing::warn!("Failed to write pidfile {:?}: {}", pidfile, e),
            }
            scopeguard::guard(pidfile, |path| {
                if let Err(e) = std::fs::remove_file(&path)
                    && e.kind() != std::io::ErrorKind::NotFound
                {
                    tracing::warn!("Failed to remove pidfile {:?}: {}", path, e);
                }
            })
        });

        tracing::info!(
            "Starting {} v{} (elevated: {})",
            cmd_name,
            env!("CARGO_PKG_VERSION"),
            crate::platform::native_platform().is_elevated()
        );

        let cancel_token = CancellationToken::new();
        let mut manager = SupervisorManager::builder(config.clone())
            .with_cancel_token(cancel_token.clone())
            .build()?;
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

        // Wait cooperatively for OS shutdown signal or cancellation
        tokio::select! {
            biased;

            _ = cancel_token.cancelled() => {
                tracing::info!("Daemon cancellation triggered");
            }
            _ = crate::platform::wait_for_shutdown_signal() => {
                tracing::info!("Shutdown signal received, initiating graceful shutdown...");
            }
        }

        // Graceful teardown
        cancel_token.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), server_handle).await;

        tracing::info!("Stopping all supervised processes...");
        if let Err(e) = manager.shutdown().await {
            tracing::error!("Error shutting down manager: {}", e);
        }

        // `pidfile_guard` drops here and removes the pidfile (clean or error path).
        drop(pidfile_guard);

        tracing::info!("{} shutdown cleanly", cmd_name);
        Ok(())
    }
}

/// Initializes the tracing subscriber from daemon args + config (OI-4 silent/file).
fn init_tracing(args: &DaemonArgs, config: &SupervisorConfig) {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let is_logging_disabled = !config.logging.enabled
        || args.loglevel.to_lowercase() == "off"
        || config.logging.level.to_lowercase() == "off";

    if is_logging_disabled {
        let filter = tracing_subscriber::EnvFilter::new("off");
        let _ = tracing_subscriber::registry().with(filter).try_init();
        return;
    }

    let log_level = if args.loglevel != "info" {
        &args.loglevel
    } else {
        &config.logging.level
    };

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(log_level));

    // OI-4: `silent` suppresses the console layer; file layer still applies.
    // Build the file layer inside each branch so `Layer<S>` types match
    // the subscriber stack (filter-only vs filter+console).
    let silent = config.logging.silent;
    let log_file = config.logging.file.clone();

    macro_rules! try_with_file {
        ($reg:expr) => {
            if let Some(ref path) = log_file {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let max_bytes = config
                    .logging
                    .max_bytes
                    .unwrap_or(crate::consts::DEFAULT_LOG_MAX_BYTES);
                let file_rotator = file_rotate::FileRotate::new(
                    path,
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
                let _ = $reg.with(file_layer).try_init();
            } else {
                let _ = $reg.try_init();
            }
        };
    }

    if silent {
        try_with_file!(tracing_subscriber::registry().with(filter));
    } else {
        let console_layer = tracing_subscriber::fmt::layer().with_target(false);
        try_with_file!(
            tracing_subscriber::registry()
                .with(filter)
                .with(console_layer)
        );
    }
}

/// Best-effort raise of a soft rlimit to at least `min_value` (OI-8).
/// Logs a warning and continues on failure (matches Python's non-fatal path
/// when the hard limit cannot be raised without privilege).
#[cfg(unix)]
fn apply_rlimit(resource: nix::sys::resource::Resource, min_value: u32, name: &str) {
    use nix::sys::resource::{getrlimit, setrlimit};

    let want = min_value as u64;
    match getrlimit(resource) {
        Ok((soft, hard)) => {
            if soft >= want {
                return;
            }
            let new_soft = want.min(hard.max(want));
            // Prefer raising soft only; if hard is below want, try both (needs priv).
            let new_hard = if hard < want { want } else { hard };
            if let Err(e) = setrlimit(resource, new_soft.min(new_hard), new_hard) {
                tracing::warn!(
                    resource = name,
                    min = min_value,
                    "setrlimit failed (continuing): {}",
                    e
                );
            } else {
                tracing::info!(resource = name, min = min_value, "rlimit raised");
            }
        }
        Err(e) => {
            tracing::warn!(resource = name, "getrlimit failed (continuing): {}", e);
        }
    }
}

/// Runs the supervisor daemon lifecycle to completion.
///
/// If `external_cancel` is supplied (e.g. from a Windows Service Control Manager handler),
/// cancellation will trigger graceful daemon teardown alongside native OS signals.
#[inline]
pub async fn run_daemon(
    args: DaemonArgs,
    config_path: PathBuf,
    external_cancel: Option<CancellationToken>,
) -> anyhow::Result<()> {
    SupervisorDaemon::new(args, config_path)
        .with_external_cancel(external_cancel)
        .run()
        .await
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
