// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use rsupervisord::cli::client::SupervisorClient;
use rsupervisord::cli::transport::Endpoint;
use rsupervisord::config::SupervisorConfig;
use rsupervisord::manager::SupervisorManager;
use rsupervisord::program::state::ProgramState;
use rsupervisord::server::ServerEngine;
use std::net::TcpListener as StdTcpListener;
use std::path::PathBuf;
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
async fn test_server_engine_rest_api_lifecycle() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let config_path = temp_dir.path().join("rsupervisord.yaml");
    let port = get_ephemeral_port();
    let ipc_path = get_test_ipc_path("lifecycle");

    let yaml = format!(
        r#"
server:
  uds_path: "{ipc_path}"
  http_bind: "127.0.0.1:{port}"
  auth_token: "test_secret_123"

program_defaults:
  autostart: false
  start_secs: 0
  stop_wait_secs: 2

programs:
  worker:
    command: |-
      {cmd}
    priority: 10
"#,
        ipc_path = ipc_path.to_string_lossy().replace('\\', "\\\\"),
        port = port,
        cmd = get_worker_command("worker_online_marker", 10),
    );

    std::fs::write(&config_path, &yaml).expect("write config file");
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

    // Allow listeners to initialize
    tokio::time::sleep(Duration::from_millis(200)).await;

    let tcp_endpoint = Endpoint::Tcp(format!("127.0.0.1:{}", port));
    let client = SupervisorClient::new(tcp_endpoint, Some("test_secret_123".to_string()));

    // 1. Initial status: worker should be STOPPED
    let statuses = client.status(&[]).await.expect("query initial status");
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].name, "worker");
    assert_eq!(statuses[0].state, "STOPPED");

    // 2. Start worker
    let start_res = client
        .start("worker", true, 10)
        .await
        .expect("start worker");
    assert_eq!(start_res.len(), 1);
    assert_eq!(start_res[0].name, "worker");
    assert_eq!(start_res[0].state, ProgramState::Running);
    assert!(start_res[0].pid.is_some());

    // 3. Verify status after start
    let statuses = client
        .status(&["worker".to_string()])
        .await
        .expect("query status after start");
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].state, "RUNNING");

    // 4. Read logs
    tokio::time::sleep(Duration::from_millis(800)).await;
    let logs = client.read_logs("worker", 50).await.expect("read logs");
    assert!(
        logs.iter()
            .any(|line| line.contains("worker_online_marker")),
        "Expected log to contain 'worker_online_marker', got: {:?}",
        logs
    );

    // 5. Restart worker
    let restart_res = client
        .restart("worker", true, 10)
        .await
        .expect("restart worker");
    assert_eq!(restart_res.len(), 1);
    assert_eq!(restart_res[0].name, "worker");
    assert_eq!(restart_res[0].state, ProgramState::Running);

    // 6. Stop worker
    let stop_res = client.stop("worker", true, 10).await.expect("stop worker");
    assert_eq!(stop_res.len(), 1);
    assert_eq!(stop_res[0].name, "worker");
    assert_eq!(stop_res[0].state, ProgramState::Stopped);

    // 7. Verify status after stop
    let statuses = client
        .status(&["worker".to_string()])
        .await
        .expect("query status after stop");
    assert_eq!(statuses[0].state, "STOPPED");

    // Clean teardown
    server_cancel.cancel();
    let _ = server_task.await;
    manager.shutdown().await.expect("shutdown manager");
}

#[tokio::test]
async fn test_server_engine_auth_protection() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let config_path = temp_dir.path().join("rsupervisord.yaml");
    let port = get_ephemeral_port();
    let ipc_path = get_test_ipc_path("auth");

    let yaml = format!(
        r#"
server:
  uds_path: "{ipc_path}"
  http_bind: "127.0.0.1:{port}"
  auth_token: "super_secure_token"

program_defaults:
  autostart: false

programs:
  dummy:
    command: |-
      {cmd}
"#,
        ipc_path = ipc_path.to_string_lossy().replace('\\', "\\\\"),
        port = port,
        cmd = get_worker_command("auth_test", 10),
    );

    std::fs::write(&config_path, &yaml).expect("write config file");
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

    let tcp_endpoint = Endpoint::Tcp(format!("127.0.0.1:{}", port));

    // Request without token should fail with 401 Unauthorized
    let unauth_client = SupervisorClient::new(tcp_endpoint.clone(), None);
    let err_res = unauth_client.status(&[]).await;
    assert!(err_res.is_err());
    let err_str = err_res.unwrap_err().to_string();
    assert!(
        err_str.contains("401"),
        "Expected 401 Unauthorized, got: {}",
        err_str
    );

    // Request with invalid token should also fail with 401 Unauthorized
    let wrong_client =
        SupervisorClient::new(tcp_endpoint.clone(), Some("incorrect_token".to_string()));
    let err_res = wrong_client.status(&[]).await;
    assert!(err_res.is_err());
    let err_str = err_res.unwrap_err().to_string();
    assert!(
        err_str.contains("401"),
        "Expected 401 Unauthorized, got: {}",
        err_str
    );

    // Request with correct token should succeed
    let valid_client =
        SupervisorClient::new(tcp_endpoint.clone(), Some("super_secure_token".to_string()));
    let ok_res = valid_client.status(&[]).await;
    assert!(ok_res.is_ok());

    server_cancel.cancel();
    let _ = server_task.await;
    manager.shutdown().await.expect("shutdown manager");
}

#[tokio::test]
async fn test_server_engine_hot_reload_api() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let config_path = temp_dir.path().join("rsupervisord.yaml");
    let port = get_ephemeral_port();
    let ipc_path = get_test_ipc_path("reload");

    let yaml_v1 = format!(
        r#"
server:
  uds_path: "{ipc_path}"
  http_bind: "127.0.0.1:{port}"

program_defaults:
  autostart: false

programs:
  service_alpha:
    command: |-
      {cmd_alpha}
"#,
        ipc_path = ipc_path.to_string_lossy().replace('\\', "\\\\"),
        port = port,
        cmd_alpha = get_worker_command("alpha", 15),
    );

    std::fs::write(&config_path, &yaml_v1).expect("write v1 config");
    let config_v1 = SupervisorConfig::from_file(&config_path).expect("parse v1");

    let mut manager = SupervisorManager::new(&config_v1).expect("create manager");
    let manager_handle = manager.handle();

    let server_cancel = CancellationToken::new();
    let server = ServerEngine::new(
        manager_handle,
        Some(config_path.clone()),
        config_v1.server.clone(),
    );
    let server_token = server_cancel.clone();
    let server_task = tokio::spawn(async move {
        let _ = server.run(server_token).await;
    });

    tokio::time::sleep(Duration::from_millis(200)).await;

    let client = SupervisorClient::new(Endpoint::Tcp(format!("127.0.0.1:{}", port)), None);

    let initial = client.status(&[]).await.expect("initial status");
    assert_eq!(initial.len(), 1);
    assert_eq!(initial[0].name, "service_alpha");

    // Write v2 config adding service_beta
    let yaml_v2 = format!(
        r#"
server:
  uds_path: "{ipc_path}"
  http_bind: "127.0.0.1:{port}"

program_defaults:
  autostart: false

programs:
  service_alpha:
    command: |-
      {cmd_alpha}
  service_beta:
    command: |-
      {cmd_beta}
"#,
        ipc_path = ipc_path.to_string_lossy().replace('\\', "\\\\"),
        port = port,
        cmd_alpha = get_worker_command("alpha", 15),
        cmd_beta = get_worker_command("beta", 15),
    );
    std::fs::write(&config_path, &yaml_v2).expect("write v2 config");

    // Trigger reload via API
    let reload_resp = client.reload().await.expect("execute reload via client");
    assert_eq!(reload_resp.unchanged, vec!["service_alpha"]);
    assert_eq!(reload_resp.added, vec!["service_beta"]);

    // Verify status now shows both programs
    let updated = client.status(&[]).await.expect("updated status");
    assert_eq!(updated.len(), 2);
    let names: Vec<String> = updated.into_iter().map(|s| s.name).collect();
    assert!(names.contains(&"service_alpha".to_string()));
    assert!(names.contains(&"service_beta".to_string()));

    server_cancel.cancel();
    let _ = server_task.await;
    manager.shutdown().await.expect("shutdown manager");
}

#[tokio::test]
async fn test_server_engine_native_ipc_transport() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let config_path = temp_dir.path().join("rsupervisord.yaml");
    let ipc_path = get_test_ipc_path("ipc_transport");

    let yaml = format!(
        r#"
server:
  uds_path: "{ipc_path}"

program_defaults:
  autostart: false

programs:
  ipc_worker:
    command: |-
      {cmd}
"#,
        ipc_path = ipc_path.to_string_lossy().replace('\\', "\\\\"),
        cmd = get_worker_command("ipc_test", 10),
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

    tokio::time::sleep(Duration::from_millis(300)).await;

    let ipc_endpoint = Endpoint::parse(&ipc_path.to_string_lossy());

    let ipc_client = SupervisorClient::new(ipc_endpoint, None);
    let statuses = ipc_client
        .status(&[])
        .await
        .expect("query status via native IPC transport");
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].name, "ipc_worker");
    assert_eq!(statuses[0].state, "STOPPED");

    server_cancel.cancel();
    let _ = server_task.await;
    manager.shutdown().await.expect("shutdown manager");
}

#[tokio::test]
async fn test_server_engine_all_start_and_stop() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let config_path = temp_dir.path().join("rsupervisord.yaml");
    let port = get_ephemeral_port();
    let ipc_path = get_test_ipc_path("all_ops");

    let yaml = format!(
        r#"
server:
  uds_path: "{ipc_path}"
  http_bind: "127.0.0.1:{port}"

program_defaults:
  autostart: true
  start_secs: 0
  stop_wait_secs: 2

programs:
  all_svc_1:
    command: |-
      {cmd1}
    priority: 10
  all_svc_2:
    command: |-
      {cmd2}
    priority: 20
"#,
        ipc_path = ipc_path.to_string_lossy().replace('\\', "\\\\"),
        port = port,
        cmd1 = get_worker_command("all_svc_1", 10),
        cmd2 = get_worker_command("all_svc_2", 10),
    );

    std::fs::write(&config_path, &yaml).expect("write config");
    let config = SupervisorConfig::from_file(&config_path).expect("parse config");

    let mut manager = SupervisorManager::new(&config).expect("create manager");
    let manager_handle = manager.handle();

    // Start all autostart programs
    manager_handle
        .start_all()
        .await
        .expect("start all initially");

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

    let client = SupervisorClient::new(Endpoint::Tcp(format!("127.0.0.1:{}", port)), None);

    // Initial status: both should be RUNNING
    let statuses = client.status(&[]).await.expect("query status");
    assert_eq!(statuses.len(), 2);
    assert!(statuses.iter().all(|s| s.state == "RUNNING"));

    // Stop all
    let stop_results = client.stop("all", true, 10).await.expect("stop all");
    assert_eq!(stop_results.len(), 2);
    assert!(
        stop_results
            .iter()
            .all(|r| r.state == ProgramState::Stopped)
    );

    // Verify status: both STOPPED
    let statuses = client
        .status(&[])
        .await
        .expect("query status after stop all");
    assert!(statuses.iter().all(|s| s.state == "STOPPED"));

    // Start all
    let start_results = client.start("all", true, 10).await.expect("start all");
    assert_eq!(start_results.len(), 2);
    assert!(
        start_results
            .iter()
            .all(|r| r.state == ProgramState::Running)
    );

    // Verify status: both RUNNING
    let statuses = client
        .status(&[])
        .await
        .expect("query status after start all");
    assert!(statuses.iter().all(|s| s.state == "RUNNING"));

    server_cancel.cancel();
    let _ = server_task.await;
    manager.shutdown().await.expect("shutdown manager");
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn test_server_engine_windows_file_uds_transport() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let config_path = temp_dir.path().join("rsupervisord.yaml");
    let socket_path = temp_dir.path().join("rsupervisord.sock");

    let yaml = format!(
        r#"
server:
  uds_path: "{sock_path}"

program_defaults:
  autostart: false

programs:
  uds_worker:
    command: |-
      {cmd}
"#,
        sock_path = socket_path.to_string_lossy().replace('\\', "\\\\"),
        cmd = get_worker_command("uds_worker", 10),
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

    tokio::time::sleep(Duration::from_millis(300)).await;

    let connect_path = config.server.uds_path.clone();
    let endpoint = Endpoint::parse(&connect_path.to_string_lossy());
    let client = SupervisorClient::new(endpoint, None);

    let statuses = client
        .status(&[])
        .await
        .expect("query status via Windows UDS socket");
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].name, "uds_worker");

    server_cancel.cancel();
    let _ = server_task.await;
    manager.shutdown().await.expect("shutdown manager");
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn test_windows_uds_raw_socket_tokio_io() {
    use std::os::windows::io::{FromRawSocket, IntoRawSocket};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use uds_windows::{UnixListener as StdUnixListener, UnixStream as StdUnixStream};

    let temp_dir = tempfile::tempdir().unwrap();
    let sock_path = temp_dir.path().join("test_raw.sock");

    let listener = StdUnixListener::bind(&sock_path).unwrap();

    let path_clone = sock_path.clone();
    let client_handle = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let std_client = StdUnixStream::connect(&path_clone).unwrap();
        let raw = std_client.into_raw_socket();
        let tcp = unsafe { std::net::TcpStream::from_raw_socket(raw) };
        tcp.set_nonblocking(true).unwrap();
        let mut tokio_stream = tokio::net::TcpStream::from_std(tcp).unwrap();
        tokio_stream.write_all(b"PING").await.unwrap();
        let mut buf = [0u8; 4];
        tokio_stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"PONG");
    });

    let (std_server, _) = listener.accept().unwrap();
    let raw = std_server.into_raw_socket();
    let tcp = unsafe { std::net::TcpStream::from_raw_socket(raw) };
    tcp.set_nonblocking(true).unwrap();
    let mut tokio_server = tokio::net::TcpStream::from_std(tcp).unwrap();

    let mut buf = [0u8; 4];
    tokio_server.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"PING");
    tokio_server.write_all(b"PONG").await.unwrap();

    client_handle.await.unwrap();
}
