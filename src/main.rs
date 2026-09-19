use clap::Parser;
use rsupervisord::config::SupervisorConfig;
use rsupervisord::manager::SupervisorManager;
use rsupervisord::server::ServerEngine;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

/// Command-line arguments for the rsupervisord daemon.
#[derive(Parser, Debug)]
#[command(
    name = "rsupervisord",
    version,
    about = "Next-generation process supervision daemon"
)]
pub struct DaemonArgs {
    /// Path to YAML configuration file
    #[arg(short = 'c', long = "config", default_value = "rsupervisord.yaml")]
    pub config: PathBuf,

    /// Run daemon in the foreground (default: true)
    #[arg(short = 'n', long = "nodaemon")]
    pub nodaemon: bool,

    /// Log level filter (trace, debug, info, warn, error)
    #[arg(short = 'l', long = "loglevel", default_value = "info")]
    pub loglevel: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().collect();
    let bin_name = args.first().cloned().unwrap_or_default();

    if bin_name.ends_with("rsupervisorctl") || bin_name.ends_with("rsupervisorctl.exe") {
        return rsupervisord::cli::run().await;
    }

    if args.len() > 1 && args[1] == "ctl" {
        // Dispatch "rsupervisord ctl ..." to CLI
        args.remove(1);
        let parsed = rsupervisord::cli::CliArgs::parse_from(args);
        return rsupervisord::cli::run_with_args(parsed).await;
    }

    let daemon_args = DaemonArgs::parse_from(args);
    run_daemon(daemon_args).await
}

async fn run_daemon(args: DaemonArgs) -> anyhow::Result<()> {
    // Initialize logging subscriber
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&args.loglevel));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();

    if !args.config.exists() {
        anyhow::bail!(
            "Configuration file not found: {:?}. Please specify a valid file using -c/--config.",
            args.config
        );
    }

    tracing::info!(
        "Starting rsupervisord v{} (elevated: {})",
        env!("CARGO_PKG_VERSION"),
        rsupervisord::platform::native_platform().is_elevated()
    );

    let config = match SupervisorConfig::from_file(&args.config) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to load configuration from {:?}: {}", args.config, e);
            return Err(e.into());
        }
    };

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
    let server = ServerEngine::new(manager_handle, Some(args.config.clone()), config.server);
    let server_token = cancel_token.clone();
    let server_handle = tokio::spawn(async move {
        if let Err(e) = server.run(server_token).await {
            tracing::error!("ServerEngine terminated with error: {}", e);
        }
    });

    // Wait cooperatively for OS shutdown signal (Ctrl+C on Windows, SIGTERM/SIGINT on Unix)
    tokio::select! {
        _ = rsupervisord::platform::wait_for_shutdown_signal() => {
            tracing::info!("Shutdown signal received, initiating graceful shutdown...");
        }
        _ = cancel_token.cancelled() => {
            tracing::info!("Daemon cancellation triggered");
        }
    }

    // Graceful teardown
    cancel_token.cancel();
    let _ = server_handle.await;

    tracing::info!("Stopping all supervised processes...");
    if let Err(e) = manager.shutdown().await {
        tracing::error!("Error shutting down manager: {}", e);
    }

    tracing::info!("rsupervisord shutdown cleanly");
    Ok(())
}
