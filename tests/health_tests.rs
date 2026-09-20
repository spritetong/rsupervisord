// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use rsupervisord::config::SupervisorConfig;
use rsupervisord::manager::SupervisorManager;
use rsupervisord::program::state::{HealthStatus, ProgramState};
use std::net::TcpListener as StdTcpListener;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn get_worker_command(msg: &str, secs: u64) -> String {
    #[cfg(unix)]
    {
        format!("sh -c 'echo \"{}\"; sleep {}'", msg, secs)
    }
    #[cfg(windows)]
    {
        format!(
            "powershell.exe -NoProfile -Command \"Write-Output '{}'; Start-Sleep -Seconds {}\"",
            msg, secs
        )
    }
}

fn get_ephemeral_port() -> u16 {
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind ephemeral std port");
    listener.local_addr().expect("local addr").port()
}

fn get_test_ipc_path(prefix: &str) -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(format!(
            r"\\.\pipe\rsupervisord_test_{}_{}_{}",
            prefix,
            std::process::id(),
            get_ephemeral_port()
        ))
    }
    #[cfg(unix)]
    {
        let temp = std::env::temp_dir();
        temp.join(format!(
            "rsupervisord_test_{}_{}_{}.sock",
            prefix,
            std::process::id(),
            get_ephemeral_port()
        ))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_tcp_health_check_success_and_failure_restart() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let config_path = temp_dir.path().join("rsupervisord.yaml");
    let ipc_path = get_test_ipc_path("health_tcp");
    let target_port = get_ephemeral_port();

    // Spawn a dummy TCP server that will listen on target_port
    let tcp_listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", target_port))
        .await
        .expect("bind dummy tcp listener");
    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut stop_rx => break,
                Ok((mut socket, _)) = tcp_listener.accept() => {
                    let mut buf = [0u8; 16];
                    let _ = socket.read(&mut buf).await;
                }
            }
        }
    });

    let yaml = format!(
        r#"
server:
  uds_path: "{ipc_path}"

programs:
  monitored_app:
    command: |-
      {cmd}
    autostart: true
    start_secs: 0
    autorestart: unexpected
    health_check:
      type: tcp
      endpoint: "127.0.0.1:{target_port}"
      interval_secs: 1
      timeout_secs: 1
      failure_threshold: 2
      initial_delay_secs: 0
"#,
        ipc_path = ipc_path.to_string_lossy().replace('\\', "\\\\"),
        cmd = get_worker_command("tcp_monitored", 30),
        target_port = target_port,
    );

    std::fs::write(&config_path, &yaml).expect("write config");
    let config = SupervisorConfig::from_file(&config_path).expect("parse config");

    let mut manager = SupervisorManager::new(&config).expect("create manager");
    let handle = manager.handle();
    handle.start_all().await.expect("start all");

    // Wait for initial health check to succeed
    let mut healthy = false;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let status = handle.get_status("monitored_app").await.expect("status");
        if status.health == HealthStatus::Healthy {
            healthy = true;
            assert_eq!(status.state, ProgramState::Running);
            break;
        }
    }
    assert!(healthy, "Expected monitored_app to become Healthy");

    // Capture initial PID
    let initial_pid = handle
        .get_status("monitored_app")
        .await
        .expect("status")
        .pid;
    assert!(initial_pid.is_some());

    // Close the target port to force health check failure
    let _ = stop_tx.send(());

    // Wait for failure threshold (2 failures @ 1s interval) and restart
    let mut restarted = false;
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let status = handle.get_status("monitored_app").await.expect("status");
        if status.pid.is_some() && status.pid != initial_pid {
            restarted = true;
            break;
        }
    }

    assert!(
        restarted,
        "Expected monitored_app to restart after health check failures"
    );

    manager.shutdown().await.expect("shutdown manager");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_http_health_check_success() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let config_path = temp_dir.path().join("rsupervisord.yaml");
    let ipc_path = get_test_ipc_path("health_http");
    let http_port = get_ephemeral_port();

    // Spawn a dummy HTTP responder on http_port
    let tcp_listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", http_port))
        .await
        .expect("bind dummy http listener");

    tokio::spawn(async move {
        while let Ok((mut socket, _)) = tcp_listener.accept().await {
            let mut buf = [0u8; 1024];
            if let Ok(n) = socket.read(&mut buf).await
                && n > 0
            {
                let resp = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK";
                let _ = socket.write_all(resp.as_bytes()).await;
            }
        }
    });

    let yaml = format!(
        r#"
server:
  uds_path: "{ipc_path}"

programs:
  http_service:
    command: |-
      {cmd}
    autostart: true
    start_secs: 0
    health_check:
      type: http
      url: "http://127.0.0.1:{http_port}/health"
      interval_secs: 1
      timeout_secs: 1
      failure_threshold: 1
"#,
        ipc_path = ipc_path.to_string_lossy().replace('\\', "\\\\"),
        cmd = get_worker_command("http_service", 20),
        http_port = http_port,
    );

    std::fs::write(&config_path, &yaml).expect("write config");
    let config = SupervisorConfig::from_file(&config_path).expect("parse config");

    let mut manager = SupervisorManager::new(&config).expect("create manager");
    let handle = manager.handle();
    handle.start_all().await.expect("start all");

    let mut healthy = false;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let status = handle.get_status("http_service").await.expect("status");
        if status.health == HealthStatus::Healthy {
            healthy = true;
            assert!(status.is_healthy);
            break;
        }
    }

    assert!(healthy, "Expected http_service to become Healthy");
    manager.shutdown().await.expect("shutdown manager");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_exec_health_check_success() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let config_path = temp_dir.path().join("rsupervisord.yaml");
    let ipc_path = get_test_ipc_path("health_exec");

    #[cfg(windows)]
    let exec_cmd = "echo ok";
    #[cfg(not(windows))]
    let exec_cmd = "exit 0";

    let yaml = format!(
        r#"
server:
  uds_path: "{ipc_path}"

programs:
  exec_service:
    command: |-
      {cmd}
    autostart: true
    start_secs: 0
    health_check:
      type: exec
      command: "{exec_cmd}"
      interval_secs: 1
      timeout_secs: 2
      failure_threshold: 1
"#,
        ipc_path = ipc_path.to_string_lossy().replace('\\', "\\\\"),
        cmd = get_worker_command("exec_service", 20),
        exec_cmd = exec_cmd,
    );

    std::fs::write(&config_path, &yaml).expect("write config");
    let config = SupervisorConfig::from_file(&config_path).expect("parse config");

    let mut manager = SupervisorManager::new(&config).expect("create manager");
    let handle = manager.handle();
    handle.start_all().await.expect("start all");

    let mut healthy = false;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let status = handle.get_status("exec_service").await.expect("status");
        if status.health == HealthStatus::Healthy {
            healthy = true;
            break;
        }
    }

    assert!(healthy, "Expected exec_service to become Healthy");
    manager.shutdown().await.expect("shutdown manager");
}
