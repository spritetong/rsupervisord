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
use clap::{CommandFactory, FromArgMatches};
use std::ffi::OsString;
use std::path::Path;

/// Entry point for the standalone `rsupervisorctl` binary.
///
/// The usage/help name is derived from `argv[0]`, mirroring clap's default.
pub fn run() -> Result<()> {
    let argv: Vec<OsString> = std::env::args_os().collect();
    let bin_name = argv
        .first()
        .and_then(|a| Path::new(a).file_name())
        .map(|f| f.to_string_lossy().into_owned());
    run_from(argv, bin_name.as_deref())
}

/// Single shared entry used by both `rsupervisorctl` and `rsupervisord ctl ...`.
///
/// Argument parsing, `--help` rendering, and the Tokio runtime construction are
/// defined exactly once here. `bin_name` overrides the usage/help program name
/// (e.g. `"rsupervisord ctl"`) so help text reflects the actual invocation.
pub fn run_from<I, T>(argv: I, bin_name: Option<&str>) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let mut cmd = CliArgs::command();
    if let Some(b) = bin_name {
        cmd = cmd.bin_name(b);
    }
    let matches = cmd.get_matches_from(argv);
    let args = match CliArgs::from_arg_matches(&matches) {
        Ok(args) => args,
        Err(e) => e.exit(),
    };

    // Short-lived client process: a lightweight current-thread runtime suffices.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(run_with_args(args, bin_name))
}

/// Executes the CLI with parsed arguments; `bin_name` is only forwarded to the
/// `help` subcommand so it renders consistently with clap's `--help`.
pub async fn run_with_args(args: CliArgs, bin_name: Option<&str>) -> Result<()> {
    // Check caller privileges
    security::validate_caller_privileges(args.allow_unelevated)?;

    // Keep a copy of the explicit config: endpoint resolution below consumes
    // `args.config`, while local service operations need it too.
    let service_config = args.config.clone();

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

    let exit_code = match command {
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
        CliCommand::Reread => commands::handle_reread(&client).await?,
        CliCommand::Update { names } => commands::handle_update(&client, &names).await?,
        CliCommand::Pid { names } => commands::handle_pid(&client, &names).await?,
        CliCommand::Shutdown => commands::handle_shutdown(&client).await?,
        CliCommand::Version => commands::handle_version().await?,
        CliCommand::Help { command } => commands::handle_help(command.as_deref(), bin_name).await?,
        CliCommand::Signal { signal, names } => {
            commands::handle_signal(&client, &signal, &names).await?
        }
        CliCommand::Avail => commands::handle_avail(&client).await?,
        CliCommand::Tail {
            name,
            channel,
            follow,
            bytes,
            lines,
        } => {
            commands::handle_tail(&client, &name, channel.as_deref(), follow, bytes, lines).await?
        }
        CliCommand::Maintail {
            follow,
            bytes,
            lines,
        } => commands::handle_maintail(&client, follow, bytes, lines).await?,
        CliCommand::Clear { names } => commands::handle_clear(&client, &names).await?,
        CliCommand::Add { names } => commands::handle_add(&client, &names).await?,
        CliCommand::Remove { names } => commands::handle_remove(&client, &names).await?,
        CliCommand::Open { url } => commands::handle_open(&client, &url).await?,
        CliCommand::Fg { name } => commands::handle_fg(&client, &name).await?,
        CliCommand::Events => commands::handle_events(&client).await?,
        CliCommand::Stdin { name, chars } => commands::handle_stdin(&client, &name, &chars).await?,
        CliCommand::Service { op } => {
            crate::service::run_service_op(op, service_config.as_deref())?;
            0
        }
    };

    if exit_code != 0 {
        std::process::exit(exit_code);
    }

    Ok(())
}
