// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use clap::Parser;
use rsupervisord::config::SupervisorConfig;
use rsupervisord::{DaemonArgs, build_tokio_runtime, run_daemon};

fn main() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().collect();
    let bin_name = args.first().cloned().unwrap_or_default();

    let stem = std::path::Path::new(&bin_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();

    if stem.to_ascii_lowercase().ends_with("ctl") {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        return rt.block_on(rsupervisord::cli::run());
    }

    if args.len() > 1 && args[1] == "ctl" {
        // Dispatch "rsupervisord ctl ..." to CLI
        args.remove(1);
        let parsed = rsupervisord::cli::CliArgs::parse_from(args);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        return rt.block_on(rsupervisord::cli::run_with_args(parsed));
    }

    let daemon_args = DaemonArgs::parse_from(args);

    let cmd_name = rsupervisord::config::paths::get_cmd_name();
    let config_path = daemon_args.config.clone().unwrap_or_else(|| {
        rsupervisord::config::paths::find_default_config_path(&cmd_name).unwrap_or_else(|| {
            rsupervisord::config::paths::get_default_config_path_fallback(&cmd_name)
        })
    });

    // Check if a service management action (--install, --uninstall, --start, --stop, --restart) was requested
    if rsupervisord::service::handle_service_command(
        &daemon_args,
        &cmd_name,
        daemon_args.config.as_deref(),
    )? {
        return Ok(());
    }

    // System service invocation via --service (e.g. Windows SCM dispatcher)
    if daemon_args.service {
        return rsupervisord::service::run_service(daemon_args, config_path, cmd_name);
    }

    let file_threads = if config_path.exists() {
        SupervisorConfig::from_file(&config_path)
            .ok()
            .and_then(|c| c.worker_threads.map(|w| w as u32))
    } else {
        None
    };

    let worker_threads = daemon_args
        .worker_threads
        .map(|w| w as u32)
        .or(file_threads);

    let rt = build_tokio_runtime(worker_threads)?;
    rt.block_on(run_daemon(daemon_args, config_path, None))
}
