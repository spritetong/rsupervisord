// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::service::ServiceOp;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Command-line arguments for rsupervisorctl.
#[derive(Parser, Debug)]
#[command(
    name = "rsupervisorctl",
    version,
    about = "Control and monitor processes managed by rsupervisord",
    disable_help_subcommand = true
)]
pub struct CliArgs {
    /// Endpoint to connect to (Unix Domain Socket, Windows Named Pipe, or TCP URL)
    #[arg(short = 's', long = "server", alias = "serverurl", global = true)]
    pub server: Option<String>,

    /// Path to rsupervisord.yaml configuration file
    #[arg(short = 'c', long = "config", alias = "configuration", global = true)]
    pub config: Option<PathBuf>,

    /// Authentication token for protected TCP connections
    #[arg(short = 'k', long = "key", global = true)]
    pub auth_token: Option<String>,

    /// Username for HTTP Basic Authentication (Supervisord compatible)
    #[arg(short = 'u', long = "user", alias = "username", global = true)]
    pub user: Option<String>,

    /// Password for HTTP Basic Authentication (Supervisord compatible)
    #[arg(short = 'p', short_alias = 'P', long = "password", global = true)]
    pub password: Option<String>,

    /// Bypass caller elevation verification
    #[arg(long = "allow-unelevated", global = true)]
    pub allow_unelevated: bool,

    #[command(subcommand)]
    pub command: Option<CliCommand>,
}

/// Available subcommands for rsupervisorctl.
#[derive(Subcommand, Debug, Clone)]
pub enum CliCommand {
    /// Show current status of managed programs
    Status {
        /// Optional specific program names to inspect
        names: Vec<String>,
    },
    /// Start specified program(s) or 'all'
    Start {
        /// Target program name(s) or 'all'
        names: Vec<String>,

        /// Asynchronous fire-and-forget mode (returns immediately)
        #[arg(short = 'a', long = "async")]
        r#async: bool,

        /// Timeout in seconds when waiting in synchronous mode
        #[arg(short = 't', long = "timeout", default_value = "30")]
        timeout: u64,
    },
    /// Stop specified program(s) or 'all'
    Stop {
        /// Target program name(s) or 'all'
        names: Vec<String>,

        /// Asynchronous fire-and-forget mode (returns immediately)
        #[arg(short = 'a', long = "async")]
        r#async: bool,

        /// Timeout in seconds when waiting in synchronous mode
        #[arg(short = 't', long = "timeout", default_value = "10")]
        timeout: u64,
    },
    /// Restart specified program(s) or 'all'
    Restart {
        /// Target program name(s) or 'all'
        names: Vec<String>,

        /// Asynchronous fire-and-forget mode (returns immediately)
        #[arg(short = 'a', long = "async")]
        r#async: bool,

        /// Timeout in seconds when waiting in synchronous mode
        #[arg(short = 't', long = "timeout", default_value = "30")]
        timeout: u64,
    },
    /// Configuration management subcommands (e.g. 'config reload')
    Config {
        #[command(subcommand)]
        action: ConfigSubcommand,
    },
    /// Perform zero-downtime hot reload of configuration file
    #[command(alias = "reload-config")]
    ConfigReload,
    /// Restart daemon: gracefully stop all programs and reload configuration (Python compatible)
    Reload,
    /// Reread configuration files and output diff without starting/stopping processes
    Reread,
    /// Reread configuration and apply changes (restart/add/remove affected process groups)
    Update {
        /// Target group name(s) or 'all'
        names: Vec<String>,
    },
    /// Get process ID for a program or all programs
    Pid {
        /// Target program name(s) or 'all'
        names: Vec<String>,
    },
    /// Shut down the remote rsupervisord daemon
    Shutdown,
    /// Manage the local system service (install/uninstall/start/stop/restart)
    Service {
        #[command(subcommand)]
        op: ServiceOp,
    },
    /// Display rsupervisorctl and protocol version
    Version,
    /// Display help information for commands
    Help {
        /// Optional command to display help for
        command: Option<String>,
    },
    /// Send a signal to a process, group, or all processes
    Signal {
        /// Signal name or number (e.g. HUP, TERM, KILL, QUIT, INT)
        signal: String,

        /// Target process or group name(s) or 'all'
        names: Vec<String>,
    },
    /// Display available process configuration information
    Avail,
    /// Tail console output for a specific program
    Tail {
        /// Program name or 'all'
        name: String,

        /// Log channel: stdout or stderr
        channel: Option<String>,

        /// Continuously stream live log lines
        #[arg(short = 'f', long = "follow")]
        follow: bool,

        /// Number of historical bytes to show (Python -N compatibility)
        #[arg(short = 'B', long = "bytes")]
        bytes: Option<usize>,

        /// Number of historical lines to show
        #[arg(short = 'n', long = "lines")]
        lines: Option<usize>,
    },
    /// Tail daemon main log
    Maintail {
        /// Continuously stream live log lines
        #[arg(short = 'f', long = "follow")]
        follow: bool,

        /// Number of historical bytes to show
        #[arg(short = 'B', long = "bytes")]
        bytes: Option<usize>,

        /// Number of historical lines to show
        #[arg(short = 'n', long = "lines")]
        lines: Option<usize>,
    },
    /// Clear log files and ring buffer for process(es)
    Clear {
        /// Target program name(s) or 'all'
        names: Vec<String>,
    },
    /// Activates a process group from pending configuration at runtime
    Add {
        /// Target group name(s)
        names: Vec<String>,
    },
    /// Deactivates a process group from active runtime set
    Remove {
        /// Target group name(s)
        names: Vec<String>,
    },
    /// Connect to a different rsupervisord server URL for current session
    Open {
        /// Server URL (http:// or unix://)
        url: String,
    },
    /// Foreground mode: stream logs and forward stdin to process
    Fg {
        /// Target program name
        name: String,
    },
    /// Stream real-time system lifecycle and state events
    Events,
    /// Send input characters to the stdin of a managed program
    #[command(alias = "send-stdin")]
    Stdin {
        /// Target program name
        name: String,
        /// Input characters to send to the process
        chars: String,
    },
}

/// Actions available under the 'config' subcommand.
#[derive(Subcommand, Debug, Clone)]
pub enum ConfigSubcommand {
    /// Perform zero-downtime hot reload of configuration file
    Reload,
}
