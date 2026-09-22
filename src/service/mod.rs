// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::daemon::DaemonArgs;
use std::path::{Path, PathBuf};

/// Inspects command-line service flags and executes service management actions if specified.
///
/// Returns `Ok(true)` if a service management flag was present and processed, or `Ok(false)`
/// if no service management flags were specified (meaning normal daemon startup should proceed).
pub fn handle_service_command(
    args: &DaemonArgs,
    cmd_name: &str,
    config_path: Option<&Path>,
) -> anyhow::Result<bool> {
    let actions = [
        args.install,
        args.uninstall,
        args.start,
        args.stop,
        args.restart,
    ];
    let count = actions.iter().filter(|&&b| b).count();
    if count > 1 {
        anyhow::bail!(
            "Only one service management flag (--install, --uninstall, --start, --stop, --restart) may be specified at a time."
        );
    }
    if count == 0 {
        return Ok(false);
    }

    let service = crate::platform::native_platform().service();
    if args.install {
        service.install(cmd_name, config_path)?;
    } else if args.uninstall {
        service.uninstall(cmd_name)?;
    } else if args.start {
        service.start(cmd_name)?;
    } else if args.stop {
        service.stop(cmd_name)?;
    } else if args.restart {
        service.restart(cmd_name)?;
    }

    Ok(true)
}

/// Runs the daemon as a system service.
pub fn run_service(args: DaemonArgs, config_path: PathBuf, cmd_name: String) -> anyhow::Result<()> {
    crate::platform::native_platform()
        .service()
        .run_service(args, config_path, cmd_name)
}
