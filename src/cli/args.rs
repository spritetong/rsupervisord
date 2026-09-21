// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Command-line arguments for rsupervisorctl.
#[derive(Parser, Debug)]
#[command(
    name = "rsupervisorctl",
    version,
    about = "Control and monitor processes managed by rsupervisord"
)]
pub struct CliArgs {
    /// Endpoint to connect to (Unix Domain Socket, Windows Named Pipe, or TCP URL)
    #[arg(short = 's', long = "server", global = true)]
    pub server: Option<String>,

    /// Path to rsupervisord.yaml configuration file
    #[arg(short = 'c', long = "config", global = true)]
    pub config: Option<PathBuf>,

    /// Authentication token for protected TCP connections
    #[arg(short = 'k', long = "key", global = true)]
    pub auth_token: Option<String>,

    /// Username for HTTP Basic Authentication (Supervisord compatible)
    #[arg(short = 'u', long = "user", global = true)]
    pub user: Option<String>,

    /// Password for HTTP Basic Authentication (Supervisord compatible)
    #[arg(short = 'P', long = "password", global = true)]
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
        #[arg(required = true)]
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
        #[arg(required = true)]
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
        #[arg(required = true)]
        names: Vec<String>,

        /// Asynchronous fire-and-forget mode (returns immediately)
        #[arg(short = 'a', long = "async")]
        r#async: bool,

        /// Timeout in seconds when waiting in synchronous mode
        #[arg(short = 't', long = "timeout", default_value = "30")]
        timeout: u64,
    },
    /// Perform zero-downtime hot reload of configuration file
    Reload,
    /// Tail console output for a specific program, or 'all' for aggregated stream
    Tail {
        /// Program name or 'all'
        name: String,

        /// Continuously stream live log lines
        #[arg(short = 'f', long = "follow")]
        follow: bool,

        /// Number of historical lines to show
        #[arg(short = 'n', long = "lines", default_value = "100")]
        lines: usize,
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
