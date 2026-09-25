// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

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

static IPC_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
static PORT_OFFSET: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);

fn get_ephemeral_port() -> u16 {
    static BASE_PORT: std::sync::LazyLock<u16> = std::sync::LazyLock::new(|| {
        let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind ephemeral std port");
        listener.local_addr().expect("local addr").port()
    });
    for _ in 0..200 {
        let offset = PORT_OFFSET.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let candidate = BASE_PORT.wrapping_add(offset);
        if candidate >= 1024 && StdTcpListener::bind(("127.0.0.1", candidate)).is_ok() {
            return candidate;
        }
    }
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind ephemeral std port");
    listener.local_addr().expect("local addr").port()
}

fn get_test_ipc_path(prefix: &str) -> PathBuf {
    let id = IPC_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    #[cfg(windows)]
    {
        PathBuf::from(format!(
            r"\\.\pipe\rsupervisord_test_{}_{}_{}",
            prefix,
            std::process::id(),
            id
        ))
    }
    #[cfg(unix)]
    {
        let temp = std::env::temp_dir();
        temp.join(format!(
            "rsupervisord_test_{}_{}_{}.sock",
            prefix,
            std::process::id(),
            id
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
    let mut started_found = false;
    for _ in 0..60 {
        {
            let list = received_events.lock();
            if list.iter().any(|e| match e {
                SystemEvent::StateChanged {
                    name, new_state, ..
                } => {
                    name == "sse_worker"
                        && (*new_state == ProgramState::Running
                            || *new_state == ProgramState::Starting)
                }
                _ => false,
            }) {
                started_found = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        started_found,
        "Expected StateChanged event for sse_worker, got: {:?}",
        *received_events.lock()
    );

    // Stop program
    let _ = client
        .stop("sse_worker", true, 5)
        .await
        .expect("stop sse_worker");

    let mut stopped_found = false;
    for _ in 0..60 {
        {
            let list = received_events.lock();
            if list.iter().any(|e| match e {
                SystemEvent::StateChanged {
                    name, new_state, ..
                } => name == "sse_worker" && *new_state == ProgramState::Stopped,
                _ => false,
            }) {
                stopped_found = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        stopped_found,
        "Expected StateChanged Stopped event for sse_worker, got: {:?}",
        *received_events.lock()
    );

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
    let mut log_found = false;
    for _ in 0..100 {
        {
            let list = received_logs.lock();
            if list
                .iter()
                .any(|l| l.program == "logger_a" && l.line.contains("agg_log_line_alpha"))
            {
                log_found = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        log_found,
        "Expected aggregated log entry from logger_a, got: {:?}",
        *received_logs.lock()
    );

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

    tokio::time::sleep(Duration::from_millis(150)).await;

    let connect_stream = || async {
        for _ in 0..40 {
            if let Ok(s) = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port)).await {
                return Ok(s);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port)).await
    };

    // 1. Connect without token -> should receive 401 Unauthorized
    {
        let mut stream = connect_stream().await.expect("connect tcp");
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
        let mut stream = connect_stream().await.expect("connect tcp");
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
