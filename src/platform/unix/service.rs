// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::daemon::DaemonArgs;
use crate::platform::traits::PlatformService;

pub struct UnixService;

/// Generates systemd service unit file content.
pub fn generate_systemd_unit(cmd_name: &str, exe_path: &Path, config_path: &Path) -> String {
    format!(
        r#"[Unit]
Description={cmd_name} Process Supervision Daemon
After=network.target

[Service]
Type=simple
ExecStart="{exe}" -c "{cfg}"
ExecReload=/bin/kill -HUP $MAINPID
Restart=on-failure
RestartSec=5s
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
"#,
        cmd_name = cmd_name,
        exe = exe_path.display(),
        cfg = config_path.display(),
    )
}

/// Executes a systemctl subcommand.
fn run_systemctl(args: &[&str], ignore_error: bool) -> anyhow::Result<()> {
    let output = Command::new("systemctl")
        .args(args)
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to execute 'systemctl {:?}': {}", args, e))?;

    if !output.status.success() && !ignore_error {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "systemctl {:?} failed with status {}: {}",
            args,
            output.status,
            stderr.trim()
        );
    }

    Ok(())
}

/// Installs the binary as an auto-start systemd service on Linux.
pub fn install_service(cmd_name: &str, config_path: Option<&Path>) -> anyhow::Result<()> {
    if !crate::platform::native_platform().is_elevated() {
        anyhow::bail!(
            "Root privileges are required to install systemd service. Please run with sudo."
        );
    }

    let exe_path = std::env::current_exe()?;
    let cfg_path: PathBuf = if let Some(p) = config_path {
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            std::env::current_dir()?.join(p)
        }
    } else {
        crate::config::paths::find_default_config_path(cmd_name)
            .unwrap_or_else(|| crate::config::paths::get_default_config_path_fallback(cmd_name))
    };

    if !cfg_path.exists() {
        eprintln!(
            "Warning: Configuration file {:?} does not exist yet. Please ensure it is present before starting the service.",
            cfg_path
        );
    }

    let unit_content = generate_systemd_unit(cmd_name, &exe_path, &cfg_path);
    let unit_file = PathBuf::from(format!("/etc/systemd/system/{}.service", cmd_name));

    std::fs::write(&unit_file, unit_content)
        .map_err(|e| anyhow::anyhow!("Failed to write unit file {:?}: {}", unit_file, e))?;

    run_systemctl(&["daemon-reload"], false)?;
    run_systemctl(&["enable", &format!("{}.service", cmd_name)], false)?;

    println!(
        "Successfully installed and enabled systemd service: {}.service",
        cmd_name
    );
    Ok(())
}

/// Uninstalls the systemd service.
pub fn uninstall_service(cmd_name: &str) -> anyhow::Result<()> {
    if !crate::platform::native_platform().is_elevated() {
        anyhow::bail!(
            "Root privileges are required to uninstall systemd service. Please run with sudo."
        );
    }

    let service_file_name = format!("{}.service", cmd_name);
    let unit_file = PathBuf::from(format!("/etc/systemd/system/{}", service_file_name));

    let _ = run_systemctl(&["stop", &service_file_name], true);
    let _ = run_systemctl(&["disable", &service_file_name], true);

    if unit_file.exists() {
        std::fs::remove_file(&unit_file)
            .map_err(|e| anyhow::anyhow!("Failed to remove unit file {:?}: {}", unit_file, e))?;
    }

    let _ = run_systemctl(&["daemon-reload"], true);
    let _ = run_systemctl(&["reset-failed"], true);

    println!(
        "Successfully uninstalled systemd service: {}.service",
        cmd_name
    );
    Ok(())
}

/// Starts the systemd service.
pub fn start_service(cmd_name: &str) -> anyhow::Result<()> {
    let service_name = format!("{}.service", cmd_name);
    println!("Starting systemd service '{}'...", service_name);
    run_systemctl(&["start", &service_name], false)?;
    println!("Service '{}' started successfully.", service_name);
    Ok(())
}

/// Stops the systemd service.
pub fn stop_service(cmd_name: &str) -> anyhow::Result<()> {
    let service_name = format!("{}.service", cmd_name);
    println!("Stopping systemd service '{}'...", service_name);
    run_systemctl(&["stop", &service_name], false)?;
    println!("Service '{}' stopped successfully.", service_name);
    Ok(())
}

/// Restarts the systemd service.
pub fn restart_service(cmd_name: &str) -> anyhow::Result<()> {
    let service_name = format!("{}.service", cmd_name);
    println!("Restarting systemd service '{}'...", service_name);
    run_systemctl(&["restart", &service_name], false)?;
    println!("Service '{}' restarted successfully.", service_name);
    Ok(())
}

impl PlatformService for UnixService {
    fn install(&self, cmd_name: &str, config_path: Option<&Path>) -> anyhow::Result<()> {
        install_service(cmd_name, config_path)
    }

    fn uninstall(&self, cmd_name: &str) -> anyhow::Result<()> {
        uninstall_service(cmd_name)
    }

    fn start(&self, cmd_name: &str) -> anyhow::Result<()> {
        start_service(cmd_name)
    }

    fn stop(&self, cmd_name: &str) -> anyhow::Result<()> {
        stop_service(cmd_name)
    }

    fn restart(&self, cmd_name: &str) -> anyhow::Result<()> {
        restart_service(cmd_name)
    }

    fn run_service(
        &self,
        _daemon_args: DaemonArgs,
        _config_path: PathBuf,
        _cmd_name: String,
    ) -> anyhow::Result<()> {
        anyhow::bail!(
            "Running via '--service' is only supported on Windows (Service Control Manager). On Linux/Unix, systemd manages standard processes directly."
        )
    }
}
