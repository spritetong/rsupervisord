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

use crate::config::{CtlConfig, SupervisorConfig};
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

/// Default endpoint when a ctl section exists but omits `serverurl` (Python parity:
/// `options.py` uses `http://localhost:9001`).
const PYTHON_DEFAULT_SERVERURL: &str = "http://localhost:9001";

/// Endpoint candidate chain plus optional basic auth and token from ctl config.
///
/// Per-candidate `basic` comes from each `CtlConfig`; the tuple's second element
/// is only a CLI-level `-u`/`-p` override (when those flags were given).
pub type ResolvedCandidates = (
    Vec<EndpointCandidate>,
    Option<(String, String)>,
    Option<String>,
);

/// System default local endpoint as a `CtlConfig.serverurl` string
/// (named pipe path on Windows, UDS path on Unix; TCP URL if ever selected).
fn default_local_url() -> String {
    match Endpoint::default_local() {
        Endpoint::Tcp(addr) => format!("http://{}", addr),
        other => other.to_string(),
    }
}

/// Converts one `CtlConfig` into an endpoint candidate (per-candidate basic).
fn ctl_to_candidate(ctl: &CtlConfig) -> EndpointCandidate {
    let url = ctl
        .serverurl
        .clone()
        .unwrap_or_else(|| PYTHON_DEFAULT_SERVERURL.to_string());
    EndpointCandidate {
        endpoint: Endpoint::parse(&url),
        basic: basic_pair(ctl.username.clone(), ctl.password.clone()),
    }
}

/// CLI `-u`/`-p` as a global basic override (applies to every candidate).
fn cli_basic_override(args: &CliArgs) -> Option<(String, String)> {
    match (&args.user, &args.password) {
        (Some(u), Some(p)) => Some((u.clone(), p.clone())),
        (Some(u), None) => Some((u.clone(), String::new())),
        (None, Some(p)) => Some((String::new(), p.clone())),
        (None, None) => None,
    }
}

/// Builds the ordered `CtlConfig` chain from an optional loaded config + CLI flags.
///
/// Chain rules (confirmed):
/// 1. **No config file** (`cfg = None`): `[default local UDS/pipe, http://localhost:9001]`
///    (IPC first). Explicit `-c` load failures never reach here (hard error above).
/// 2. **Section present** (`ctl = Some`): single entry — strict Python when a
///    section exists (no server fallbacks). Partial fields already filled at load
///    when `server.ctl_defaults`.
/// 3. **No section + `ctl_defaults`**: full server backfill via
///    [`CtlConfig::vec_from_server`] (IPC then TCP), every connection field set.
/// 4. **No section + `!ctl_defaults`** (INI): hard error — Python requires
///    `[supervisorctl]`.
///
/// CLI: `-s` collapses a multi-entry chain to one seed whose endpoint type matches
/// `-s` (credentials seed); then `CliArgs::apply` runs on every entry
/// (`-s`/`-k` field-independent; `-u`/`-p` pair override).
///
/// Public for integration tests.
pub fn resolve_ctl_chain(cfg: Option<&SupervisorConfig>, args: &CliArgs) -> Result<Vec<CtlConfig>> {
    let mut ctls = match cfg {
        None => vec![
            CtlConfig {
                serverurl: Some(default_local_url()),
                ..Default::default()
            },
            CtlConfig {
                serverurl: Some(PYTHON_DEFAULT_SERVERURL.to_string()),
                ..Default::default()
            },
        ],
        Some(cfg) => {
            if let Some(ctl) = cfg.ctl.clone() {
                vec![ctl]
            } else if cfg.server.ctl_defaults {
                CtlConfig::vec_from_server(&cfg.server)
            } else {
                anyhow::bail!(
                    "configuration does not include a [supervisorctl] / ctl section \
                     (server.ctl_defaults is false and no section was provided)"
                );
            }
        }
    };

    // `-s` replaces the whole chain with a single endpoint; seed credentials from
    // the config entry whose endpoint type matches `-s` (IPC vs TCP).
    if args.server.is_some() && ctls.len() > 1 {
        let want_tcp = Endpoint::parse(args.server.as_deref().unwrap_or_default()).is_tcp();
        let idx = ctls
            .iter()
            .position(|c| {
                Endpoint::parse(c.serverurl.as_deref().unwrap_or_default()).is_tcp() == want_tcp
            })
            .unwrap_or(0);
        let seed = ctls.swap_remove(idx);
        ctls.clear();
        ctls.push(seed);
    }

    for ctl in &mut ctls {
        args.apply(ctl);
    }
    Ok(ctls)
}

/// Resolves endpoint candidates plus credentials from ctl config.
///
/// Loads the config file (explicit `-c` failures hard-error), builds the chain
/// via [`resolve_ctl_chain`], then converts each `CtlConfig` to an
/// [`EndpointCandidate`] with its own basic credentials. Token is taken from the
/// first entry (shared; `-k` / server backfill apply the same value chain-wide).
///
/// Public for integration tests covering OI-1 / Python parity.
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

    let ctls = resolve_ctl_chain(cfg.as_ref(), args)?;
    let candidates: Vec<EndpointCandidate> = ctls.iter().map(ctl_to_candidate).collect();
    let token = ctls.first().and_then(|c| c.auth_token.clone());
    Ok((candidates, cli_basic_override(args), token))
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

    // Determine target daemon endpoint + credentials from ctl config.
    // Per-candidate basic comes from each CtlConfig; basic_override is CLI -u/-p.
    let (candidates, basic_auth, auth_token) = resolve_endpoint_candidates(&args)?;

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
