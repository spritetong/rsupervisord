// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use rsupervisord::config::schema::SupervisorConfig;
use rsupervisord::manager::SupervisorManager;
use std::fs;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn test_watch_binary_changed_triggers_restart() {
    let temp_dir = tempfile::tempdir().unwrap();
    let bin_path = if cfg!(windows) {
        temp_dir.path().join("mock_worker.bat")
    } else {
        temp_dir.path().join("mock_worker.sh")
    };

    let content = if cfg!(windows) {
        "@echo off\r\n:loop\r\necho running\r\nping -n 2 127.0.0.1 >nul\r\ngoto loop\r\n"
    } else {
        "#!/bin/sh\nwhile true; do echo running; sleep 1; done\n"
    };

    fs::write(&bin_path, content).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&bin_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&bin_path, perms).unwrap();
    }

    let bin_path_str = bin_path.to_str().unwrap().replace('\\', "/");

    let yaml = format!(
        r#"
programs:
  monitored_worker:
    command: "{}"
    restart_when_binary_changed: true
    restart_debounce_secs: 1
    start_secs: 0
"#,
        bin_path_str
    );

    let config = SupervisorConfig::from_yaml_str(&yaml).unwrap();
    let mut manager = SupervisorManager::new(&config).unwrap();
    let handle = manager.handle();

    handle.start_program("monitored_worker").await.unwrap();

    let initial_status = handle.get_status("monitored_worker").await.unwrap();
    assert!(initial_status.state.is_running() || initial_status.state.is_active());
    let initial_pid = initial_status.pid;

    // Small delay to ensure watcher registration is active
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Simulate compiler writing chunks: touch multiple times rapidly
    let updated_content = if cfg!(windows) {
        "@echo off\r\n:loop\r\necho updated\r\nping -n 2 127.0.0.1 >nul\r\ngoto loop\r\n"
    } else {
        "#!/bin/sh\nwhile true; do echo updated; sleep 1; done\n"
    };

    fs::write(&bin_path, updated_content).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    fs::write(&bin_path, updated_content).unwrap();

    // Wait for debounce window (1s) + execution margin
    let mut restarted = false;
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(300)).await;
        if let Ok(st) = handle.get_status("monitored_worker").await
            && st.pid.is_some()
            && st.pid != initial_pid
        {
            restarted = true;
            break;
        }
    }

    assert!(
        restarted,
        "Program should have restarted with a new PID after binary modification"
    );

    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_watch_directory_monitor_triggers_signal() {
    let temp_dir = tempfile::tempdir().unwrap();
    let conf_dir = temp_dir.path().join("configs");
    fs::create_dir_all(&conf_dir).unwrap();

    let conf_file = conf_dir.join("app.conf");
    fs::write(&conf_file, "key=value1\n").unwrap();

    let bin_path = if cfg!(windows) {
        temp_dir.path().join("server.bat")
    } else {
        temp_dir.path().join("server.sh")
    };

    let content = if cfg!(windows) {
        "@echo off\r\n:loop\r\nping -n 2 127.0.0.1 >nul\r\ngoto loop\r\n"
    } else {
        "#!/bin/sh\nwhile true; do sleep 1; done\n"
    };

    fs::write(&bin_path, content).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&bin_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&bin_path, perms).unwrap();
    }

    let bin_path_str = bin_path.to_str().unwrap().replace('\\', "/");
    let conf_dir_str = conf_dir.to_str().unwrap().replace('\\', "/");

    let yaml = format!(
        r#"
programs:
  server_app:
    command: "{}"
    restart_directory_monitor: "{}"
    restart_file_pattern: "*.conf"
    restart_debounce_secs: 1
    restart_signal_when_file_changed: "TERM"
    start_secs: 0
"#,
        bin_path_str, conf_dir_str
    );

    let config = SupervisorConfig::from_yaml_str(&yaml).unwrap();
    let mut manager = SupervisorManager::new(&config).unwrap();
    let handle = manager.handle();

    handle.start_program("server_app").await.unwrap();
    let initial_status = handle.get_status("server_app").await.unwrap();
    assert!(initial_status.state.is_active());

    tokio::time::sleep(Duration::from_millis(500)).await;

    // Modify matching config file
    fs::write(&conf_file, "key=value2\n").unwrap();

    // Since restart_signal_when_file_changed is TERM, server_app receives TERM and terminates/exits
    let mut signaled = false;
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(300)).await;
        if let Ok(st) = handle.get_status("server_app").await
            && (st.pid != initial_status.pid || !st.state.is_running())
        {
            signaled = true;
            break;
        }
    }

    assert!(
        signaled,
        "Signal should have affected program after config file modification"
    );

    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_watch_service_cancellation() {
    let cancel_token = CancellationToken::new();
    let configs = std::collections::HashMap::new();

    // Spawn watch service directly with mock handle
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    let handle = rsupervisord::manager::ManagerHandle::new_mock_for_test(tx, cancel_token.clone());

    let watch_handle =
        rsupervisord::manager::WatchService::spawn(handle, configs, cancel_token.clone());

    // Update configs should not block or panic
    watch_handle
        .update_configs(std::collections::HashMap::new())
        .await;

    // Cancel token
    cancel_token.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}
