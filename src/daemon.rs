// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::config::SupervisorConfig;
use crate::error::ProgramError;
use crate::manager::SupervisorManager;
use crate::server::ServerEngine;
use crate::service::ServiceOp;
use clap::{Parser, Subcommand};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
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
    #[arg(short = 'l', long = "loglevel")]
    pub loglevel: Option<String>,

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

static DAEMON_ARGS: OnceLock<DaemonArgs> = OnceLock::new();

/// Stores the parsed daemon args so config reload paths can re-apply CLI overrides.
/// Subsequent calls are no-ops (first write wins).
pub fn set_daemon_args(args: &DaemonArgs) {
    let _ = DAEMON_ARGS.set(args.clone());
}

/// Returns the globally stored daemon args, if [`set_daemon_args`] has run.
pub fn daemon_args() -> Option<&'static DaemonArgs> {
    DAEMON_ARGS.get()
}

impl DaemonArgs {
    /// Applies explicit CLI overrides onto a freshly loaded config.
    ///
    /// This is the only place command-line flags write into [`SupervisorConfig`].
    /// Absent options leave the config value untouched. Idempotent; safe to call
    /// on every load and reload.
    pub fn apply(&self, config: &mut SupervisorConfig) {
        if self.nodaemon {
            config.nodaemon = true;
        }
        if self.allow_unelevated {
            config.server.allow_unelevated = true;
        }
        if let Some(threads) = self.worker_threads {
            config.worker_threads = Some(threads);
        }
        if let Some(level) = &self.loglevel {
            config.logging.level = level.clone();
        }
    }
}

/// Loads config from `path` and applies CLI overrides from `args`.
///
/// Pass `Some(&args)` when args are in scope (initial load, runtime peek);
/// pass [`daemon_args`] on reload paths where only the global is available.
pub fn load_config<P: AsRef<Path>>(
    path: P,
    args: Option<&DaemonArgs>,
) -> Result<SupervisorConfig, ProgramError> {
    let mut config = SupervisorConfig::from_file(path)?;
    if let Some(a) = args {
        a.apply(&mut config);
    }
    Ok(config)
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

        let config = match load_config(&config_path, Some(&args)) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Failed to load configuration from {:?}: {}", config_path, e);
                return Err(e.into());
            }
        };

        let main_log_rotator = config.logging.build_main_rotator();

        // Initialize tracing before any early startup logs so environment
        // application, rlimit, and pidfile messages are not lost to the
        // default no-op subscriber.
        let main_file_rotator = init_tracing(&config, main_log_rotator.clone());

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
            .with_main_log_rotator(main_log_rotator)
            .with_main_file_rotator(main_file_rotator)
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
        let _ = tokio::time::timeout(crate::consts::DAEMON_SERVER_SHUTDOWN_TIMEOUT, server_handle)
            .await;

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

/// Initializes the tracing subscriber from config (OI-4 silent/file).
///
struct ArcLogWriter(std::sync::Arc<crate::logging::LogRotator>);

impl std::io::Write for ArcLogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

/// Initializes tracing subscriber with console and optional file layers.
///
/// Log level precedence: `RUST_LOG` env var > `config.logging.level`.
/// CLI `-l/--loglevel` is folded into `config.logging.level` by
/// [`DaemonArgs::apply`] before this runs; `RUST_LOG` still wins when set.
fn init_tracing(
    config: &SupervisorConfig,
    main_log_rotator: Option<std::sync::Arc<crate::logging::InMemoryChannelRotator>>,
) -> Option<std::sync::Arc<crate::logging::LogRotator>> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let is_logging_disabled =
        !config.logging.enabled.is_enabled() || config.logging.level.to_lowercase() == "off";

    if is_logging_disabled {
        let filter = tracing_subscriber::EnvFilter::new("off");
        let _ = tracing_subscriber::registry().with(filter).try_init();
        return None;
    }

    let log_level = &config.logging.level;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(log_level));

    // OI-4: `silent` suppresses the console layer; file or in-memory layer still applies.
    let silent = config.logging.silent;

    let log_file = config.logging.file.clone();

    let mut main_file_rotator = None;
    let writer_box: Option<Box<dyn std::io::Write + Send>> = if let Some(ref rotator) =
        main_log_rotator
    {
        Some(Box::new(crate::logging::InMemoryLogWriter(
            std::sync::Arc::clone(rotator),
        )))
    } else if let Some(ref path) = log_file {
        let path_str = path.to_string_lossy();
        let dest = crate::logging::LogDestination::parse(&path_str)
            .unwrap_or(crate::logging::LogDestination::File(path.clone()));

        // Resolve composite destinations to the primary concrete sink so
        // `a.log, /dev/stdout` still writes a file layer instead of being dropped.
        let effective_dest = match dest {
            crate::logging::LogDestination::Composite(ref list) => {
                let primary = dest
                    .primary_file_path()
                    .map(|p| crate::logging::LogDestination::File(p.to_path_buf()))
                    .or_else(|| {
                        list.iter().find_map(|d| match d {
                            crate::logging::LogDestination::DevStdout => {
                                Some(crate::logging::LogDestination::DevStdout)
                            }
                            crate::logging::LogDestination::DevStderr => {
                                Some(crate::logging::LogDestination::DevStderr)
                            }
                            _ => None,
                        })
                    });
                match primary {
                    Some(d) => d,
                    None => {
                        tracing::warn!(
                            "Daemon log destination '{}' has no writable file/stdio sink; console logging only",
                            path_str
                        );
                        crate::logging::LogDestination::Null
                    }
                }
            }
            crate::logging::LogDestination::Syslog(_) => {
                tracing::warn!(
                    "Daemon log destination '{}' uses syslog which is not supported for the daemon file layer; console logging only",
                    path_str
                );
                crate::logging::LogDestination::Null
            }
            crate::logging::LogDestination::Auto => crate::logging::LogDestination::Null,
            other => other,
        };

        match effective_dest {
            crate::logging::LogDestination::Null => None,
            crate::logging::LogDestination::DevStdout => Some(Box::new(std::io::stdout())),
            crate::logging::LogDestination::DevStderr => Some(Box::new(std::io::stderr())),
            crate::logging::LogDestination::File(ref file_path) => {
                let max_bytes = config
                    .logging
                    .max_bytes
                    .unwrap_or(crate::consts::DEFAULT_LOG_MAX_BYTES);
                let backups = config.logging.backups;
                let timestamp_suffix = config.logging.timestamp_suffix;
                match crate::logging::LogRotator::with_options(
                    file_path,
                    max_bytes,
                    backups,
                    timestamp_suffix,
                ) {
                    Ok(rot) => {
                        let arc_rot = std::sync::Arc::new(rot);
                        main_file_rotator = Some(arc_rot.clone());
                        Some(Box::new(ArcLogWriter(arc_rot)) as Box<dyn std::io::Write + Send>)
                    }
                    Err(e) => {
                        eprintln!(
                            "Failed to initialize daemon log rotator for {:?}: {}",
                            file_path, e
                        );
                        None
                    }
                }
            }
            _ => None,
        }
    } else {
        None
    };

    let file_writer_arc = writer_box.map(|w| std::sync::Arc::new(std::sync::Mutex::new(w)));

    macro_rules! try_with_file {
        ($reg:expr) => {
            if let Some(ref file_writer_arc) = file_writer_arc {
                let fw = file_writer_arc.clone();
                let make_writer = move || MutexWriter(fw.clone());
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

    main_file_rotator
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

struct MutexWriter(std::sync::Arc<std::sync::Mutex<Box<dyn std::io::Write + Send>>>);

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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_daemon_args_loglevel_none_by_default() {
        let args = DaemonArgs::try_parse_from(["supervisord"]).unwrap();
        assert!(args.loglevel.is_none());
    }

    #[test]
    fn test_daemon_args_explicit_loglevel_parsed() {
        let args = DaemonArgs::try_parse_from(["supervisord", "-l", "debug"]).unwrap();
        assert_eq!(args.loglevel.as_deref(), Some("debug"));

        let args = DaemonArgs::try_parse_from(["supervisord", "--loglevel", "warn"]).unwrap();
        assert_eq!(args.loglevel.as_deref(), Some("warn"));
    }

    #[test]
    fn test_apply_overrides_config_fields() {
        let args = DaemonArgs::try_parse_from([
            "supervisord",
            "-n",
            "--allow-unelevated",
            "--worker-threads",
            "4",
            "-l",
            "debug",
        ])
        .unwrap();

        let mut config = SupervisorConfig {
            nodaemon: false,
            worker_threads: Some(1),
            ..Default::default()
        };
        config.server.allow_unelevated = false;
        config.logging.level = "info".into();

        args.apply(&mut config);

        assert!(config.nodaemon);
        assert!(config.server.allow_unelevated);
        assert_eq!(config.worker_threads, Some(4));
        assert_eq!(config.logging.level, "debug");
    }

    #[test]
    fn test_apply_absent_flags_leave_config_untouched() {
        let args = DaemonArgs::try_parse_from(["supervisord"]).unwrap();

        let mut config = SupervisorConfig {
            nodaemon: false,
            worker_threads: Some(2),
            ..Default::default()
        };
        config.server.allow_unelevated = false;
        config.logging.level = "warn".into();

        args.apply(&mut config);

        assert!(!config.nodaemon);
        assert!(!config.server.allow_unelevated);
        assert_eq!(config.worker_threads, Some(2));
        assert_eq!(config.logging.level, "warn");
    }

    #[test]
    fn test_apply_is_idempotent() {
        let args = DaemonArgs::try_parse_from([
            "supervisord",
            "-n",
            "--worker-threads",
            "8",
            "-l",
            "error",
        ])
        .unwrap();

        let mut config = SupervisorConfig::default();
        args.apply(&mut config);
        let first_nodaemon = config.nodaemon;
        let first_threads = config.worker_threads;
        let first_level = config.logging.level.clone();

        args.apply(&mut config);

        assert_eq!(config.nodaemon, first_nodaemon);
        assert_eq!(config.worker_threads, first_threads);
        assert_eq!(config.logging.level, first_level);
    }
}
