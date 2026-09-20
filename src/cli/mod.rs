// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

pub mod args;
pub mod client;
pub mod commands;
pub mod security;
pub mod transport;

pub use args::{CliArgs, CliCommand};
pub use client::SupervisorClient;
pub use transport::{Endpoint, StreamTransport};

use anyhow::Result;
use clap::Parser;

/// Main entry point for the rsupervisorctl CLI utility.
pub async fn run() -> Result<()> {
    let args = CliArgs::parse();
    run_with_args(args).await
}

/// Executes rsupervisorctl with parsed arguments.
pub async fn run_with_args(args: CliArgs) -> Result<()> {
    // Check caller privileges
    security::validate_caller_privileges(args.allow_unelevated)?;

    // Determine target daemon endpoint
    let endpoint = if let Some(ref s) = args.server {
        Endpoint::parse(s)
    } else if let Some(ref cfg_path) = args.config {
        if let Ok(content) = std::fs::read_to_string(cfg_path)
            && let Ok(cfg) = crate::config::SupervisorConfig::from_yaml_str(&content)
        {
            if let Some(ref http) = cfg.server.http_bind {
                Endpoint::parse(http)
            } else {
                Endpoint::parse(&cfg.server.uds_path.to_string_lossy())
            }
        } else {
            Endpoint::default_local()
        }
    } else {
        Endpoint::default_local()
    };

    let client = SupervisorClient::new(endpoint, args.auth_token);

    // Default to 'status' if no subcommand was explicitly provided
    let command = args
        .command
        .unwrap_or(CliCommand::Status { names: Vec::new() });

    match command {
        CliCommand::Status { names } => commands::handle_status(&client, &names).await?,
        CliCommand::Start {
            names,
            r#async,
            timeout,
        } => commands::handle_start(&client, &names, r#async, timeout).await?,
        CliCommand::Stop {
            names,
            r#async,
            timeout,
        } => commands::handle_stop(&client, &names, r#async, timeout).await?,
        CliCommand::Restart {
            names,
            r#async,
            timeout,
        } => commands::handle_restart(&client, &names, r#async, timeout).await?,
        CliCommand::Reload => commands::handle_reload(&client).await?,
        CliCommand::Tail {
            name,
            follow,
            lines,
        } => commands::handle_tail(&client, &name, follow, lines).await?,
        CliCommand::Events => commands::handle_events(&client).await?,
    }

    Ok(())
}
