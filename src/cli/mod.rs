// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

pub mod args;
pub mod client;
pub mod commands;
pub mod transport;

pub use args::{CliArgs, CliCommand};
pub use client::{EndpointCandidate, SupervisorClient};
pub use transport::{Endpoint, StreamTransport};

use anyhow::Result;
use clap::{CommandFactory, FromArgMatches};
use std::ffi::OsString;

/// Builds an HTTP Basic credential pair from optional username/password fields,
/// returning `None` when both are absent or empty (open access).
fn basic_pair(u: Option<String>, p: Option<String>) -> Option<(String, String)> {
    match (u, p) {
        (Some(u), Some(p)) if !u.is_empty() || !p.is_empty() => Some((u, p)),
        (Some(u), None) if !u.is_empty() => Some((u, String::new())),
        (None, Some(p)) if !p.is_empty() => Some((String::new(), p)),
        _ => None,
    }
}

/// Endpoint candidate chain plus optional CLI/`[supervisorctl]` basic auth and token.
pub type ResolvedCandidates = (
    Vec<EndpointCandidate>,
    Option<(String, String)>,
    Option<String>,
);

/// Resolves the ordered endpoint candidate chain.
///
/// Endpoint priority:
/// 1. explicit `-s/--server`
/// 2. `[supervisorctl] serverurl` from config (OI-1)
/// 3. config-derived candidate chain (Windows: pipe → uds → TCP; Unix: TCP or UDS)
///
/// Basic-credential priority (second tuple element, applied as override):
/// 1. CLI `-u`/`-p`
/// 2. `[supervisorctl] username`/`password` (OI-1)
/// 3. `None` — each candidate carries its own server-section credentials
///
/// Config is always loaded when available so credentials/token apply even
/// with explicit `-s` (CLI flags still win at the call site).
///
/// Public for integration tests covering OI-1 (`[supervisorctl]` defaults).
pub fn resolve_endpoint_candidates(args: &CliArgs) -> Result<ResolvedCandidates> {
    let cmd_name = crate::config::paths::get_cmd_name();
    let cfg_path = args
        .config
        .clone()
        .or_else(|| crate::config::paths::find_default_config_path(&cmd_name));

    let explicit_config = args.config.is_some();
    let cfg = match cfg_path {
        Some(ref path) => match crate::config::SupervisorConfig::from_file(path) {
            Ok(c) => Some(c),
            Err(e) if explicit_config => {
                // Explicit `-c` must fail hard: silent fallback would connect
                // to the default local endpoint and issue commands against
                // the wrong instance.
                anyhow::bail!("Failed to load config {:?}: {}", path, e);
            }
            Err(_) => None,
        },
        None => None,
    };

    let cli_defaults = cfg.as_ref().and_then(|c| c.cli_defaults.as_ref());
    let token = cfg.as_ref().and_then(|c| c.server.auth_token.clone());

    let cli_basic = match (args.user.clone(), args.password.clone()) {
        (Some(u), Some(p)) => Some((u, p)),
        (Some(u), None) => Some((u, String::new())),
        (None, Some(p)) => Some((String::new(), p)),
        (None, None) => {
            cli_defaults.and_then(|d| basic_pair(d.username.clone(), d.password.clone()))
        }
    };

    // 1. Explicit -s
    if let Some(ref s) = args.server {
        return Ok((
            vec![EndpointCandidate {
                endpoint: Endpoint::parse(s),
                basic: None,
            }],
            cli_basic,
            token,
        ));
    }

    if let Some(cfg) = cfg.as_ref() {
        // 2. [supervisorctl] serverurl seeds the endpoint when -s is absent (OI-1).
        if let Some(url) = cli_defaults.and_then(|d| d.serverurl.as_ref()) {
            return Ok((
                vec![EndpointCandidate {
                    endpoint: Endpoint::parse(url),
                    basic: None,
                }],
                cli_basic,
                token,
            ));
        }

        let tcp_basic = basic_pair(cfg.server.username.clone(), cfg.server.password.clone());
        let uds_basic = basic_pair(
            cfg.server.uds_username.clone(),
            cfg.server.uds_password.clone(),
        );

        #[cfg(windows)]
        {
            // Candidate chain: default local pipe → configured uds_path → TCP.
            let mut candidates: Vec<EndpointCandidate> = Vec::new();
            let default_local = Endpoint::default_local();
            candidates.push(EndpointCandidate {
                endpoint: default_local.clone(),
                basic: uds_basic.clone(),
            });

            let uds_ep = Endpoint::parse(&cfg.server.uds_path.to_string_lossy());
            if uds_ep != default_local {
                candidates.push(EndpointCandidate {
                    endpoint: uds_ep,
                    basic: uds_basic.clone(),
                });
            }

            if let Some(ref http) = cfg.server.http_bind {
                candidates.push(EndpointCandidate {
                    endpoint: Endpoint::parse(http),
                    basic: tcp_basic.clone(),
                });
            }

            return Ok((candidates, cli_basic, token));
        }

        #[cfg(not(windows))]
        {
            // Preserve historical single-endpoint behavior on Unix:
            // TCP first when http_bind is set, else IPC path.
            let candidate = if let Some(ref http) = cfg.server.http_bind {
                EndpointCandidate {
                    endpoint: Endpoint::parse(http),
                    basic: tcp_basic.clone(),
                }
            } else {
                EndpointCandidate {
                    endpoint: Endpoint::parse(&cfg.server.uds_path.to_string_lossy()),
                    basic: uds_basic.clone(),
                }
            };
            return Ok((vec![candidate], cli_basic, token));
        }
    }

    // No config: single default local endpoint (pipe on Windows, UDS on Unix).
    Ok((
        vec![EndpointCandidate {
            endpoint: Endpoint::default_local(),
            basic: None,
        }],
        cli_basic,
        token,
    ))
}

/// Entry point for the standalone `supervisorctl` binary.
///
/// The usage/help name is derived from `exe_path()`, mirroring clap's default.
pub fn run() -> Result<()> {
    // Capture exe_path from argv[0] before clap argument parsing.
    let exe = crate::config::paths::exe_path();

    let argv: Vec<OsString> = std::env::args_os().collect();
    let bin_name = exe.file_name().map(|f| f.to_string_lossy().into_owned());
    run_from(argv, bin_name.as_deref())
}

/// Single shared entry used by both `supervisorctl` and `supervisord ctl ...`.
///
/// Argument parsing, `--help` rendering, and the Tokio runtime construction are
/// defined exactly once here. `bin_name` overrides the usage/help program name
/// (e.g. `"supervisord ctl"`) so help text reflects the actual invocation.
pub fn run_from<I, T>(argv: I, bin_name: Option<&str>) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    // Capture exe_path before clap argument parsing.
    let _ = crate::config::paths::exe_path();

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
    // Keep a copy of the explicit config: endpoint resolution below consumes
    // `args.config`, while local service operations need it too.
    let service_config = args.config.clone();

    // Determine target daemon endpoint candidates & configuration credentials.
    // Explicit `-s` still loads config so `-c` + `[supervisorctl]` defaults apply;
    // CLI `-u`/`-p` always override config-derived credentials (OI-1).
    let (candidates, cfg_basic_auth, cfg_token) = resolve_endpoint_candidates(&args)?;

    // CLI -u/-p override config-derived basic auth for all candidates.
    // resolve_endpoint_candidates already folds CLI flags / [supervisorctl] into
    // cfg_basic_auth; re-apply CLI flags here so they always win if something
    // upstream changed.
    let basic_auth = match (args.user, args.password) {
        (Some(u), Some(p)) => Some((u, p)),
        (Some(u), None) => Some((u, String::new())),
        (None, Some(p)) => Some((String::new(), p)),
        (None, None) => cfg_basic_auth,
    };

    let auth_token = args.auth_token.or(cfg_token);

    let client = SupervisorClient::new_with_candidates(candidates, auth_token, basic_auth);

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
        CliCommand::Version => commands::handle_version()?,
        CliCommand::Help { command } => commands::handle_help(command.as_deref(), bin_name)?,
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
        CliCommand::Open { url } => commands::handle_open(&client, &url)?,
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
