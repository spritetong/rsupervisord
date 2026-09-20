// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

#[cfg(unix)]
pub mod linux;

#[cfg(windows)]
pub mod windows;

use crate::daemon::DaemonArgs;
use std::path::Path;

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

    #[cfg(windows)]
    {
        if args.install {
            windows::install_service(cmd_name, config_path)?;
        } else if args.uninstall {
            windows::uninstall_service(cmd_name)?;
        } else if args.start {
            windows::start_service(cmd_name)?;
        } else if args.stop {
            windows::stop_service(cmd_name)?;
        } else if args.restart {
            windows::restart_service(cmd_name)?;
        }
        Ok(true)
    }

    #[cfg(unix)]
    {
        if args.install {
            linux::install_service(cmd_name, config_path)?;
        } else if args.uninstall {
            linux::uninstall_service(cmd_name)?;
        } else if args.start {
            linux::start_service(cmd_name)?;
        } else if args.stop {
            linux::stop_service(cmd_name)?;
        } else if args.restart {
            linux::restart_service(cmd_name)?;
        }
        Ok(true)
    }

    #[cfg(not(any(windows, unix)))]
    {
        let _ = (cmd_name, config_path);
        anyhow::bail!("Service management is not supported on this platform.");
    }
}
