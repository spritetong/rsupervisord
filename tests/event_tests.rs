// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use parking_lot::Mutex;
use rsupervisord::cli::client::SupervisorClient;
use rsupervisord::cli::transport::Endpoint;
use rsupervisord::config::SupervisorConfig;
use rsupervisord::manager::{EventHub, LogEntry, SupervisorManager, SystemEvent};
use rsupervisord::program::state::ProgramState;
use rsupervisord::server::ServerEngine;
use std::net::TcpListener as StdTcpListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

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

#[tokio::test]
async fn test_event_hub_dual_track_isolation() {
    let hub = EventHub::new(16, 16);
    let mut sys_rx = hub.subscribe_system();
    let mut log_rx = hub.subscribe_logs();

    hub.publish_system(SystemEvent::DaemonLifecycle {
        action: "started".to_string(),
        timestamp_secs: 1000,
    });

    hub.publish_log(LogEntry::new("test_prog", "stdout", "hello event bus"));

    let sys_evt = tokio::time::timeout(Duration::from_millis(500), sys_rx.recv())
        .await
        .expect("sys_rx recv timeout")
        .expect("sys_rx recv error");

    match sys_evt {
        SystemEvent::DaemonLifecycle {
            action,
            timestamp_secs,
        } => {
            assert_eq!(action, "started");
            assert_eq!(timestamp_secs, 1000);
        }
        other => panic!("Unexpected event: {:?}", other),
    }

    let log_evt = tokio::time::timeout(Duration::from_millis(500), log_rx.recv())
        .await
        .expect("log_rx recv timeout")
        .expect("log_rx recv error");

    assert_eq!(log_evt.program, "test_prog");
    assert_eq!(log_evt.stream, "stdout");
    assert_eq!(log_evt.line, "hello event bus");
}

#[tokio::test]
async fn test_sse_system_events_stream_endpoint() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let config_path = temp_dir.path().join("rsupervisord.yaml");
    let port = get_ephemeral_port();
    let ipc_path = get_test_ipc_path("sse_events");

    let yaml = format!(
        r#"
server:
  uds_path: "{ipc_path}"
  http_bind: "127.0.0.1:{port}"

program_defaults:
  autostart: false
  start_secs: 0
  stop_wait_secs: 2

programs:
  sse_worker:
    command: |-
      {cmd}
    priority: 10
"#,
        ipc_path = ipc_path.to_string_lossy().replace('\\', "\\\\"),
        port = port,
        cmd = get_worker_command("sse_worker_started", 10),
    );

    std::fs::write(&config_path, &yaml).expect("write config");
    let config = SupervisorConfig::from_file(&config_path).expect("parse config");

    let mut manager = SupervisorManager::new(&config).expect("create manager");
    let manager_handle = manager.handle();

    let server_cancel = CancellationToken::new();
    let server = ServerEngine::new(
        manager_handle,
        Some(config_path.clone()),
        config.server.clone(),
    );
    let server_token = server_cancel.clone();
    let server_task = tokio::spawn(async move {
        let _ = server.run(server_token).await;
    });

    tokio::time::sleep(Duration::from_millis(200)).await;

    let endpoint = Endpoint::Tcp(format!("127.0.0.1:{}", port));
    let client = SupervisorClient::new(endpoint, None);

    // Spawn event listener
    let received_events = Arc::new(Mutex::new(Vec::<SystemEvent>::new()));
    let events_collector = received_events.clone();
    let client_for_events = client.clone();

    let listener_task = tokio::spawn(async move {
        let _ = client_for_events
            .stream_events(|line| {
                if let Ok(evt) = serde_json::from_str::<SystemEvent>(line) {
                    let mut list = events_collector.lock();
                    list.push(evt);
                }
            })
            .await;
    });

    // Give SSE connection time to establish
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Start program
    let _ = client
        .start("sse_worker", true, 5)
        .await
        .expect("start sse_worker");

    // Wait for event to propagate
    tokio::time::sleep(Duration::from_millis(500)).await;

    {
        let list = received_events.lock();
        assert!(
            list.iter().any(|e| match e {
                SystemEvent::StateChanged {
                    name, new_state, ..
                } => {
                    name == "sse_worker"
                        && (*new_state == ProgramState::Running
                            || *new_state == ProgramState::Starting)
                }
                _ => false,
            }),
            "Expected StateChanged event for sse_worker, got: {:?}",
            *list
        );
    }

    // Stop program
    let _ = client
        .stop("sse_worker", true, 5)
        .await
        .expect("stop sse_worker");
    tokio::time::sleep(Duration::from_millis(500)).await;

    {
        let list = received_events.lock();
        assert!(
            list.iter().any(|e| match e {
                SystemEvent::StateChanged {
                    name, new_state, ..
                } => {
                    name == "sse_worker" && *new_state == ProgramState::Stopped
                }
                _ => false,
            }),
            "Expected StateChanged Stopped event for sse_worker, got: {:?}",
            *list
        );
    }

    listener_task.abort();
    server_cancel.cancel();
    let _ = server_task.await;
    manager.shutdown().await.expect("shutdown manager");
}

#[tokio::test]
async fn test_sse_all_logs_stream_endpoint() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let config_path = temp_dir.path().join("rsupervisord.yaml");
    let port = get_ephemeral_port();
    let ipc_path = get_test_ipc_path("sse_all_logs");

    let yaml = format!(
        r#"
server:
  uds_path: "{ipc_path}"
  http_bind: "127.0.0.1:{port}"

program_defaults:
  autostart: false
  start_secs: 0
  stop_wait_secs: 2

programs:
  logger_a:
    command: |-
      {cmd}
    priority: 10
"#,
        ipc_path = ipc_path.to_string_lossy().replace('\\', "\\\\"),
        port = port,
        cmd = get_worker_command("agg_log_line_alpha", 5),
    );

    std::fs::write(&config_path, &yaml).expect("write config");
    let config = SupervisorConfig::from_file(&config_path).expect("parse config");

    let mut manager = SupervisorManager::new(&config).expect("create manager");
    let manager_handle = manager.handle();

    let server_cancel = CancellationToken::new();
    let server = ServerEngine::new(
        manager_handle,
        Some(config_path.clone()),
        config.server.clone(),
    );
    let server_token = server_cancel.clone();
    let server_task = tokio::spawn(async move {
        let _ = server.run(server_token).await;
    });

    tokio::time::sleep(Duration::from_millis(200)).await;

    let endpoint = Endpoint::Tcp(format!("127.0.0.1:{}", port));
    let client = SupervisorClient::new(endpoint, None);

    let received_logs = Arc::new(Mutex::new(Vec::<LogEntry>::new()));
    let logs_collector = received_logs.clone();
    let client_for_logs = client.clone();

    let listener_task = tokio::spawn(async move {
        let _ = client_for_logs
            .stream_all_logs(|line| {
                if let Ok(entry) = serde_json::from_str::<LogEntry>(line) {
                    let mut list = logs_collector.lock();
                    list.push(entry);
                }
            })
            .await;
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

    // Start program to emit logs
    let _ = client
        .start("logger_a", true, 5)
        .await
        .expect("start logger_a");

    // Allow logs to flush and pump to central hub
    tokio::time::sleep(Duration::from_millis(1500)).await;

    {
        let list = received_logs.lock();
        assert!(
            list.iter()
                .any(|l| l.program == "logger_a" && l.line.contains("agg_log_line_alpha")),
            "Expected aggregated log entry from logger_a, got: {:?}",
            *list
        );
    }

    listener_task.abort();
    server_cancel.cancel();
    let _ = server_task.await;
    manager.shutdown().await.expect("shutdown manager");
}

#[tokio::test]
async fn test_sse_auth_query_token_protection() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let config_path = temp_dir.path().join("rsupervisord.yaml");
    let port = get_ephemeral_port();
    let ipc_path = get_test_ipc_path("sse_auth");

    let yaml = format!(
        r#"
server:
  uds_path: "{ipc_path}"
  http_bind: "127.0.0.1:{port}"
  auth_token: "secret_bus_token"

program_defaults:
  autostart: false

programs:
  dummy:
    command: |-
      {cmd}
"#,
        ipc_path = ipc_path.to_string_lossy().replace('\\', "\\\\"),
        port = port,
        cmd = get_worker_command("dummy", 1),
    );

    std::fs::write(&config_path, &yaml).expect("write config");
    let config = SupervisorConfig::from_file(&config_path).expect("parse config");

    let mut manager = SupervisorManager::new(&config).expect("create manager");
    let manager_handle = manager.handle();

    let server_cancel = CancellationToken::new();
    let server = ServerEngine::new(
        manager_handle,
        Some(config_path.clone()),
        config.server.clone(),
    );
    let server_token = server_cancel.clone();
    let server_task = tokio::spawn(async move {
        let _ = server.run(server_token).await;
    });

    tokio::time::sleep(Duration::from_millis(200)).await;

    // 1. Connect without token -> should receive 401 Unauthorized
    {
        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .expect("connect tcp");
        stream
            .write_all(b"GET /api/v1/events HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .expect("write req");
        let mut reader = BufReader::new(stream).lines();
        let status_line = reader
            .next_line()
            .await
            .expect("read status")
            .expect("has status");
        assert!(
            status_line.contains("401"),
            "Expected 401 Unauthorized, got: {}",
            status_line
        );
    }

    // 2. Connect with query param token -> should receive 200 OK
    {
        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .expect("connect tcp");
        stream
            .write_all(
                b"GET /api/v1/events?token=secret_bus_token HTTP/1.1\r\nHost: localhost\r\n\r\n",
            )
            .await
            .expect("write req");
        let mut reader = BufReader::new(stream).lines();
        let status_line = reader
            .next_line()
            .await
            .expect("read status")
            .expect("has status");
        assert!(
            status_line.contains("200"),
            "Expected 200 OK with query token, got: {}",
            status_line
        );
    }

    server_cancel.cancel();
    let _ = server_task.await;
    manager.shutdown().await.expect("shutdown manager");
}
