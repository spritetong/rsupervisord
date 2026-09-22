// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::daemon::DaemonArgs;
use clap::Subcommand;
use std::path::{Path, PathBuf};

/// Service lifecycle operations shared by `rsupervisord service ...` and
/// `rsupervisorctl service ...`.
#[derive(Subcommand, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceOp {
    /// Install as a system service
    Install,
    /// Uninstall the system service
    Uninstall,
    /// Start the system service
    Start,
    /// Stop the system service
    Stop,
    /// Restart the system service
    Restart,
}

/// Executes a service lifecycle operation.
///
/// Single entry point shared by the daemon binary (`rsupervisord service ...`)
/// and the control binary (`rsupervisorctl service ...`). Resolves the command
/// name, configuration path, and daemon executable through the existing
/// `config::paths` helpers, then dispatches to the platform service backend.
pub fn run_service_op(op: ServiceOp, explicit_config: Option<&Path>) -> anyhow::Result<()> {
    let cmd_name = crate::config::paths::get_cmd_name();
    let service = crate::platform::native_platform().service();

    match op {
        ServiceOp::Install => {
            let exe_path = crate::config::paths::find_daemon_exe(&cmd_name);
            if !exe_path.is_file() {
                anyhow::bail!(
                    "Daemon executable not found at {:?}. Deploy {} next to this binary.",
                    exe_path,
                    cmd_name
                );
            }
            let config_path =
                crate::config::paths::resolve_config_path(&cmd_name, explicit_config)?;
            if !config_path.exists() {
                eprintln!(
                    "Warning: Configuration file {:?} does not exist yet. Please ensure it is present before starting the service.",
                    config_path
                );
            }
            service.install(&cmd_name, &exe_path, &config_path)
        }
        ServiceOp::Uninstall => service.uninstall(&cmd_name),
        ServiceOp::Start => service.start(&cmd_name),
        ServiceOp::Stop => service.stop(&cmd_name),
        ServiceOp::Restart => service.restart(&cmd_name),
    }
}

/// Runs the daemon as a system service.
pub fn run_service(args: DaemonArgs, config_path: PathBuf, cmd_name: String) -> anyhow::Result<()> {
    crate::platform::native_platform()
        .service()
        .run_service(args, config_path, cmd_name)
}
