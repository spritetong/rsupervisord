// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

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

    // Determine target daemon endpoint & configuration credentials
    let (endpoint, cfg_basic_auth, cfg_token) = if let Some(ref s) = args.server {
        (Endpoint::parse(s), None, None)
    } else {
        let cmd_name = crate::config::paths::get_cmd_name();
        let cfg_path = args
            .config
            .or_else(|| crate::config::paths::find_default_config_path(&cmd_name));
        if let Some(ref path) = cfg_path
            && let Ok(cfg) = crate::config::SupervisorConfig::from_file(path)
        {
            let (ep, basic) = if let Some(ref http) = cfg.server.http_bind {
                let ep = Endpoint::parse(http);
                let basic = match (cfg.server.username, cfg.server.password) {
                    (Some(u), Some(p)) if !u.is_empty() || !p.is_empty() => Some((u, p)),
                    _ => None,
                };
                (ep, basic)
            } else {
                let ep = Endpoint::parse(&cfg.server.uds_path.to_string_lossy());
                let basic = match (cfg.server.uds_username, cfg.server.uds_password) {
                    (Some(u), Some(p)) if !u.is_empty() || !p.is_empty() => Some((u, p)),
                    _ => None,
                };
                (ep, basic)
            };
            (ep, basic, cfg.server.auth_token)
        } else {
            (Endpoint::default_local(), None, None)
        }
    };

    let basic_auth = match (args.user, args.password) {
        (Some(u), Some(p)) => Some((u, p)),
        (Some(u), None) => Some((u, String::new())),
        (None, Some(p)) => Some((String::new(), p)),
        (None, None) => cfg_basic_auth,
    };

    let auth_token = args.auth_token.or(cfg_token);

    let client = SupervisorClient::new_with_auth(endpoint, auth_token, basic_auth);

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
        CliCommand::Config {
            action: args::ConfigSubcommand::Reload,
        } => commands::handle_config_reload(&client).await?,
        CliCommand::ConfigReload => commands::handle_config_reload(&client).await?,
        CliCommand::Reload => commands::handle_daemon_reload(&client).await?,
        CliCommand::Tail {
            name,
            follow,
            lines,
        } => commands::handle_tail(&client, &name, follow, lines).await?,
        CliCommand::Events => commands::handle_events(&client).await?,
        CliCommand::Stdin { name, chars } => commands::handle_stdin(&client, &name, &chars).await?,
    }

    Ok(())
}
