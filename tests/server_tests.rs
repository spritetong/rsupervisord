// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

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

    // Trigger config reload via API (hot reload)
    let reload_resp = client
        .config_reload()
        .await
        .expect("execute config_reload via client");
    assert_eq!(reload_resp.unchanged, vec!["service_alpha"]);
    assert_eq!(reload_resp.added, vec!["service_beta"]);

    // Verify status now shows both programs
    let updated = client.status(&[]).await.expect("updated status");
    assert_eq!(updated.len(), 2);
    let names: Vec<String> = updated.into_iter().map(|s| s.name).collect();
    assert!(names.contains(&"service_alpha".to_string()));
    assert!(names.contains(&"service_beta".to_string()));

    // Trigger full daemon reload (Python compatible restart)
    let restart_msg = client.reload().await.expect("execute reload via client");
    assert!(restart_msg.contains("restarted") || restart_msg.contains("OK"));

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

#[tokio::test]
async fn test_server_engine_basic_auth_plaintext() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let config_path = temp_dir.path().join("rsupervisord.yaml");
    let port = get_ephemeral_port();
    let ipc_path = get_test_ipc_path("basic_auth_plain");

    let yaml = format!(
        r#"
server:
  uds_path: "{ipc_path}"
  http_bind: "127.0.0.1:{port}"
  username: "admin"
  password: "secret123"

programs: {{}}
"#,
        ipc_path = ipc_path.to_string_lossy().replace('\\', "\\\\"),
        port = port,
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

    // 1. Raw TCP HTTP request without credentials -> 401 with WWW-Authenticate header
    let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .expect("connect tcp");
    tokio::io::AsyncWriteExt::write_all(
        &mut stream,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await
    .expect("send get");
    let mut resp = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut stream, &mut resp)
        .await
        .expect("read resp");
    let resp_str = String::from_utf8_lossy(&resp);
    assert!(
        resp_str.contains("401 Unauthorized"),
        "Expected 401: {}",
        resp_str
    );
    assert!(
        resp_str
            .to_lowercase()
            .contains("www-authenticate: basic realm=\"supervisor\""),
        "Expected WWW-Authenticate header: {}",
        resp_str
    );

    // 2. Client with wrong password -> fails with 401
    let wrong_client = SupervisorClient::new(tcp_endpoint.clone(), None)
        .with_basic_auth("admin".to_string(), "wrong_pass".to_string());
    let err_res = wrong_client.status(&[]).await;
    assert!(err_res.is_err());
    assert!(err_res.unwrap_err().to_string().contains("401"));

    // 3. Client with correct basic auth -> 200 OK
    let valid_client = SupervisorClient::new(tcp_endpoint.clone(), None)
        .with_basic_auth("admin".to_string(), "secret123".to_string());
    let ok_res = valid_client.status(&[]).await;
    assert!(ok_res.is_ok());

    // 4. Raw TCP HTTP request with valid Authorization header to Web UI (/) -> 200 OK
    use base64::Engine;
    let b64_auth = base64::engine::general_purpose::STANDARD.encode("admin:secret123");
    let mut stream_auth = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .expect("connect tcp");
    let auth_req = format!(
        "GET / HTTP/1.1\r\nHost: localhost\r\nAuthorization: Basic {}\r\nConnection: close\r\n\r\n",
        b64_auth
    );
    tokio::io::AsyncWriteExt::write_all(&mut stream_auth, auth_req.as_bytes())
        .await
        .expect("send auth req");
    let mut resp_auth = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut stream_auth, &mut resp_auth)
        .await
        .expect("read resp auth");
    let resp_auth_str = String::from_utf8_lossy(&resp_auth);
    assert!(
        resp_auth_str.contains("200 OK"),
        "Expected 200 OK for authenticated web UI request: {}",
        resp_auth_str
    );

    // 5. Local IPC client (without basic auth) continues to work
    let ipc_endpoint = Endpoint::parse(&ipc_path.to_string_lossy());
    let ipc_client = SupervisorClient::new(ipc_endpoint, None);
    let ipc_res = ipc_client.status(&[]).await;
    assert!(ipc_res.is_ok());

    server_cancel.cancel();
    let _ = server_task.await;
    manager.shutdown().await.expect("shutdown manager");
}

#[tokio::test]
async fn test_server_engine_basic_auth_sha1() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let config_path = temp_dir.path().join("rsupervisord.yaml");
    let port = get_ephemeral_port();
    let ipc_path = get_test_ipc_path("basic_auth_sha1");

    // sha1("thepassword") = 82ab876d1387bfafe46cc1c8a2ef074eae50cb1d
    let yaml = format!(
        r#"
server:
  uds_path: "{ipc_path}"
  http_bind: "127.0.0.1:{port}"
  username: "test1"
  password: "{{SHA}}82ab876d1387bfafe46cc1c8a2ef074eae50cb1d"

programs: {{}}
"#,
        ipc_path = ipc_path.to_string_lossy().replace('\\', "\\\\"),
        port = port,
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

    // Client with valid plaintext password matching SHA-1 hash -> succeeds
    let valid_client = SupervisorClient::new(tcp_endpoint.clone(), None)
        .with_basic_auth("test1".to_string(), "thepassword".to_string());
    let ok_res = valid_client.status(&[]).await;
    assert!(ok_res.is_ok());

    // Client with wrong password -> fails with 401
    let wrong_client = SupervisorClient::new(tcp_endpoint.clone(), None)
        .with_basic_auth("test1".to_string(), "badpassword".to_string());
    let err_res = wrong_client.status(&[]).await;
    assert!(err_res.is_err());
    assert!(err_res.unwrap_err().to_string().contains("401"));

    server_cancel.cancel();
    let _ = server_task.await;
    manager.shutdown().await.expect("shutdown manager");
}

#[tokio::test]
async fn test_server_engine_group_api_and_cli_operations() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let config_path = temp_dir.path().join("config.yaml");
    let port = get_ephemeral_port();
    let ipc_path = get_test_ipc_path("group_test");

    let yaml = format!(
        r#"
server:
  uds_path: "{ipc_path}"
  http_bind: "127.0.0.1:{port}"

groups:
  cluster_group:
    programs:
      - node1
      - node2

programs:
  node1:
    command: |-
      {cmd1}
    autostart: false
    start_secs: 0
    stop_wait_secs: 2
  node2:
    command: |-
      {cmd2}
    autostart: false
    start_secs: 0
    stop_wait_secs: 2
  single_worker:
    command: |-
      {cmd3}
    autostart: false
    start_secs: 0
    stop_wait_secs: 2
"#,
        ipc_path = ipc_path.to_string_lossy().replace('\\', "\\\\"),
        port = port,
        cmd1 = get_worker_command("node1", 10),
        cmd2 = get_worker_command("node2", 10),
        cmd3 = get_worker_command("worker", 10),
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

    let client = SupervisorClient::new(Endpoint::Tcp(format!("127.0.0.1:{}", port)), None);

    // 1. Check all status: group fields populated
    let all = client.status(&[]).await.expect("status all");
    assert_eq!(all.len(), 3);
    let n1 = all.iter().find(|p| p.name == "node1").unwrap();
    let n2 = all.iter().find(|p| p.name == "node2").unwrap();
    let sw = all.iter().find(|p| p.name == "single_worker").unwrap();
    assert_eq!(n1.group, "cluster_group");
    assert_eq!(n2.group, "cluster_group");
    assert_eq!(sw.group, "single_worker");

    // 2. Filter status by group:*
    let group_status = client
        .status(&["cluster_group:*".to_string()])
        .await
        .expect("status group:*");
    assert_eq!(group_status.len(), 2);
    assert!(group_status.iter().all(|p| p.group == "cluster_group"));

    // 3. Filter status by group:program
    let single_in_group = client
        .status(&["cluster_group:node1".to_string()])
        .await
        .expect("status group:prog");
    assert_eq!(single_in_group.len(), 1);
    assert_eq!(single_in_group[0].name, "node1");

    // 4. Start by group wildcard
    let start_resp = client
        .start("cluster_group:*", false, 5)
        .await
        .expect("start group:*");
    assert_eq!(start_resp.len(), 2);

    tokio::time::sleep(Duration::from_millis(300)).await;
    let st1 = client.get_program("node1").await.unwrap();
    let st2 = client.get_program("node2").await.unwrap();
    let st3 = client.get_program("single_worker").await.unwrap();
    assert_eq!(st1.state, ProgramState::Running);
    assert_eq!(st1.group, "cluster_group");
    assert_eq!(st2.state, ProgramState::Running);
    assert_eq!(st2.group, "cluster_group");
    assert_eq!(st3.state, ProgramState::Stopped);

    // 5. Stop by group wildcard
    let stop_resp = client
        .stop("cluster_group:*", false, 5)
        .await
        .expect("stop group:*");
    assert_eq!(stop_resp.len(), 2);

    tokio::time::sleep(Duration::from_millis(300)).await;
    let st1_after = client.get_program("node1").await.unwrap();
    assert_eq!(st1_after.state, ProgramState::Stopped);

    server_cancel.cancel();
    let _ = server_task.await;
    manager.shutdown().await.expect("shutdown manager");
}

#[tokio::test]
async fn test_server_api_send_stdin() {
    let _temp_dir = tempfile::tempdir().expect("create tempdir");
    let port = get_ephemeral_port();
    let ipc_path = get_test_ipc_path("stdin");
    let ipc_str = ipc_path.to_string_lossy().replace('\\', "\\\\");

    let yaml = format!(
        r#"
server:
  uds_path: "{ipc_str}"
  http_bind: "127.0.0.1:{port}"

program_defaults:
  autostart: true
  start_secs: 0
  stop_wait_secs: 2

programs:
  worker:
    command: |-
      {cmd}
"#,
        cmd = get_worker_command("ready", 10),
    );

    let config = SupervisorConfig::from_yaml_str(&yaml).expect("parse yaml");
    let mut manager = SupervisorManager::new(&config).expect("create manager");
    let handle = manager.handle();

    handle.start_program("worker").await.expect("start worker");

    let server_engine = ServerEngine::new(handle, None, config.server.clone());
    let server_cancel = CancellationToken::new();
    let cancel_clone = server_cancel.clone();

    let server_task = tokio::spawn(async move {
        let _ = server_engine.run(cancel_clone).await;
    });

    tokio::time::sleep(Duration::from_millis(200)).await;

    let client = SupervisorClient::new(Endpoint::Tcp(format!("127.0.0.1:{}", port)), None);

    // Send stdin via HTTP client
    let res = client.send_stdin("worker", "test input payload\n").await;
    assert!(res.is_ok(), "client.send_stdin should succeed: {:?}", res);

    // Send stdin to unknown program returns error (404)
    let err_res = client.send_stdin("nonexistent", "data\n").await;
    assert!(
        err_res.is_err(),
        "client.send_stdin to unknown program should fail"
    );

    server_cancel.cancel();
    let _ = server_task.await;
    manager.shutdown().await.expect("shutdown manager");
}
