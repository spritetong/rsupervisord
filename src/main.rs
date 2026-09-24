// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use clap::Parser;
use rsupervisord::daemon::{load_config, set_daemon_args};
use rsupervisord::{DaemonArgs, build_tokio_runtime, run_daemon};

fn main() -> anyhow::Result<()> {
    // Capture exe_path from argv[0] before any CLI argument parsing.
    let exe = rsupervisord::config::paths::exe_path();

    let mut args: Vec<String> = std::env::args().collect();

    let stem = exe.file_stem().and_then(|s| s.to_str()).unwrap_or_default();

    if stem.to_ascii_lowercase().ends_with("ctl") {
        // Binary renamed/symlinked as *ctl: dispatch to the shared CLI entry.
        return rsupervisord::cli::run();
    }

    if args.len() > 1 && args[1] == "ctl" {
        // "supervisord ctl ..." is an alias of supervisorctl; strip the token
        // and render usage/help as "<bin> ctl".
        let bin_name = format!(
            "{} ctl",
            exe.file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| "supervisord".to_string())
        );
        args.remove(1);
        return rsupervisord::cli::run_from(args, Some(&bin_name));
    }

    let daemon_args = DaemonArgs::parse_from(args);
    set_daemon_args(&daemon_args);

    // Service lifecycle: `supervisord service <install|uninstall|start|stop|restart>`
    if let Some(rsupervisord::daemon::DaemonAction::Service { op }) = &daemon_args.action {
        return rsupervisord::service::run_service_op(*op, daemon_args.config.as_deref());
    }

    let cmd_name = rsupervisord::config::paths::get_cmd_name();
    let config_path =
        rsupervisord::config::paths::resolve_config_path(&cmd_name, daemon_args.config.as_deref())?;

    // System service invocation via --service (e.g. Windows SCM dispatcher)
    if daemon_args.service {
        return rsupervisord::service::run_service(daemon_args, config_path, cmd_name);
    }

    let worker_threads = load_config(&config_path, Some(&daemon_args))
        .ok()
        .and_then(|c| c.worker_threads)
        .or(daemon_args.worker_threads)
        .map(|w| w as u32);

    let rt = build_tokio_runtime(worker_threads)?;
    rt.block_on(run_daemon(daemon_args, config_path, None))
}
