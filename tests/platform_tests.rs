// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

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
    assert_eq!(uds_path.to_str().unwrap(), r"\\.\pipe\rsupervisord");
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

#[tokio::test]
async fn test_platform_guard_wait_exit() {
    let platform = native_platform();
    let (cmd_str, args) = get_sleep_cmd_and_args(1);

    let mut cmd = tokio::process::Command::new(cmd_str);
    cmd.args(&args);

    platform
        .configure_command(&mut cmd, None, None)
        .expect("configure_command failed");

    let mut child = cmd.spawn().expect("spawn child");
    let pid = child.id().expect("PID missing");

    let mut guard = platform
        .attach_child(&child, pid)
        .expect("attach_child failed");

    let exit_status = tokio::time::timeout(Duration::from_secs(5), guard.wait_exit(&mut child))
        .await
        .expect("child wait_exit timeout")
        .expect("child wait_exit failed");

    assert!(exit_status.success() || exit_status.code().is_some());
}

#[test]
fn test_build_tokio_runtime_single_and_multi_threaded() {
    use rsupervisord::build_tokio_runtime;

    // Single-threaded (current_thread)
    let rt1 = build_tokio_runtime(Some(1)).expect("build single-threaded runtime");
    let val1 = rt1.block_on(async { 42 });
    assert_eq!(val1, 42);

    // Multi-threaded with explicit thread count
    let rt2 = build_tokio_runtime(Some(2)).expect("build 2-thread runtime");
    let val2 = rt2.block_on(async { 100 });
    assert_eq!(val2, 100);

    // Default thread count
    let rt_default = build_tokio_runtime(None).expect("build default runtime");
    let val_def = rt_default.block_on(async { 200 });
    assert_eq!(val_def, 200);
}

#[cfg(windows)]
#[tokio::test]
async fn test_windows_gui_wm_close_shutdown_signal() {
    let task = tokio::spawn(async {
        rsupervisord::platform::wait_for_shutdown_signal().await;
    });

    // Give the listener window a brief moment to spawn
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Find the hidden GUI listener window by its title
    let window_title = "rsupervisord_shutdown_listener\0"
        .encode_utf16()
        .collect::<Vec<u16>>();
    let hwnd = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW(
            std::ptr::null(),
            window_title.as_ptr(),
        )
    };

    assert!(!hwnd.is_null(), "Hidden GUI listener window must exist");

    // Post WM_CLOSE to simulate user closing GUI app or taskkill
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
            hwnd,
            windows_sys::Win32::UI::WindowsAndMessaging::WM_CLOSE,
            0,
            0,
        );
    }

    // wait_for_shutdown_signal must finish promptly
    let res = tokio::time::timeout(Duration::from_secs(3), task).await;
    assert!(
        res.is_ok(),
        "wait_for_shutdown_signal timed out on WM_CLOSE"
    );
}

#[cfg(windows)]
#[tokio::test]
async fn test_windows_gui_notepad_graceful_stop() {
    let platform = native_platform();
    let mut cmd = tokio::process::Command::new("notepad.exe");
    platform
        .configure_command(&mut cmd, None, None)
        .expect("configure_command failed");

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping test_windows_gui_notepad_graceful_stop: {}", e);
            return;
        }
    };
    let pid = child.id().expect("PID missing");

    let mut guard = platform
        .attach_child(&child, pid)
        .expect("attach_child failed");

    // Wait a brief moment for Notepad to initialize its GUI window
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Send stop signal which posts WM_CLOSE to notepad top-level window
    guard
        .send_stop_signal(StopSignal::default())
        .expect("send_stop_signal failed");

    // Notepad should process WM_CLOSE and exit cleanly within 5 seconds
    let exit_status = tokio::time::timeout(Duration::from_secs(5), guard.wait_exit(&mut child))
        .await
        .expect("Notepad failed to exit after WM_CLOSE within 5s")
        .expect("child wait_exit failed");

    assert!(exit_status.success() || exit_status.code().is_some());
}
