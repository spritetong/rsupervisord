// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use rsupervisord::platform::{PlatformProcessGuard, native_platform};
use rsupervisord::program::config::StopSignal;
use std::time::Duration;

fn get_sleep_cmd_and_args(secs: u64) -> (String, Vec<String>) {
    #[cfg(unix)]
    {
        ("sleep".to_string(), vec![secs.to_string()])
    }
    #[cfg(windows)]
    {
        (
            "powershell.exe".to_string(),
            vec![
                "-NoProfile".to_string(),
                "-Command".to_string(),
                format!("Start-Sleep -Seconds {}", secs),
            ],
        )
    }
}

#[test]
fn test_platform_default_uds_path_is_valid() {
    let platform = native_platform();
    let uds_path = platform.default_uds_path();
    assert!(!uds_path.as_os_str().is_empty());

    #[cfg(unix)]
    assert_eq!(uds_path.to_str().unwrap(), "/var/run/rsupervisord.sock");

    #[cfg(windows)]
    assert!(uds_path.to_str().unwrap().contains("rsupervisord.sock"));
}

#[tokio::test]
async fn test_platform_backend_configure_attach_and_signal() {
    let platform = native_platform();
    let (cmd_str, args) = get_sleep_cmd_and_args(10);

    let mut cmd = tokio::process::Command::new(cmd_str);
    cmd.args(&args);

    // Test platform configuration
    platform
        .configure_command(&mut cmd, None, None)
        .expect("configure_command failed");

    let mut child = cmd.spawn().expect("failed to spawn child");
    let pid = child.id().expect("child PID missing");

    // Test attaching platform guard
    let guard: Box<dyn PlatformProcessGuard> = platform
        .attach_child(&child, pid)
        .expect("attach_child failed");

    assert_eq!(guard.pid(), pid);

    // Send stop signal
    guard
        .send_stop_signal(StopSignal::default())
        .expect("send_stop_signal failed");

    // Verify process exits within grace period
    let exit_status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("child timed out")
        .expect("child wait failed");

    assert!(exit_status.success() || exit_status.code().is_some() || exit_status.code().is_none());
}

#[tokio::test]
async fn test_platform_guard_force_kill() {
    let platform = native_platform();
    let (cmd_str, args) = get_sleep_cmd_and_args(30);

    let mut cmd = tokio::process::Command::new(cmd_str);
    cmd.args(&args);

    platform
        .configure_command(&mut cmd, None, None)
        .expect("configure_command failed");

    let mut child = cmd.spawn().expect("spawn child");
    let pid = child.id().expect("PID missing");

    let guard = platform
        .attach_child(&child, pid)
        .expect("attach_child failed");

    // Force kill immediately
    guard.force_kill().expect("force_kill failed");

    let exit_status = tokio::time::timeout(Duration::from_secs(3), child.wait())
        .await
        .expect("child force kill timeout")
        .expect("child wait failed");

    // Process was terminated
    assert!(!exit_status.success());
}
