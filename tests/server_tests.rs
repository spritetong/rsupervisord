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
    let mut logs = Vec::new();
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Ok(l) = client.read_logs("worker", 50).await {
            logs = l;
            if logs
                .iter()
                .any(|line| line.contains("worker_online_marker"))
            {
                break;
            }
        }
    }
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

    // 1. Static shell is public so the login page can load without a browser
    //    native basic-auth dialog.
    {
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
            resp_str.contains("200 OK"),
            "Static shell must stay public, got: {}",
            resp_str
        );
        assert!(
            !resp_str.to_lowercase().contains("www-authenticate:"),
            "Static shell must not trigger native basic auth dialog: {}",
            resp_str
        );
    }

    // 2. Protected API without credentials -> bare 401, no WWW-Authenticate
    //    (browser must never pop the native dialog for fetch() calls).
    {
        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .expect("connect tcp");
        tokio::io::AsyncWriteExt::write_all(
            &mut stream,
            b"GET /api/v1/status HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .await
        .expect("send get status");
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
            !resp_str.to_lowercase().contains("www-authenticate:"),
            "API 401 must not carry WWW-Authenticate: {}",
            resp_str
        );
    }

    // 3. Client with wrong password -> fails with 401
    let wrong_client = SupervisorClient::new(tcp_endpoint.clone(), None)
        .with_basic_auth("admin".to_string(), "wrong_pass".to_string());
    let err_res = wrong_client.status(&[]).await;
    assert!(err_res.is_err());
    assert!(err_res.unwrap_err().to_string().contains("401"));

    // 4. Client with correct basic auth -> 200 OK
    let valid_client = SupervisorClient::new(tcp_endpoint.clone(), None)
        .with_basic_auth("admin".to_string(), "secret123".to_string());
    let ok_res = valid_client.status(&[]).await;
    assert!(ok_res.is_ok());

    // 5. Auth endpoints are public: config report works without credentials
    {
        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .expect("connect tcp");
        tokio::io::AsyncWriteExt::write_all(
            &mut stream,
            b"GET /api/v1/auth/config HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .await
        .expect("send auth config");
        let mut resp = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut stream, &mut resp)
            .await
            .expect("read resp");
        let resp_str = String::from_utf8_lossy(&resp);
        assert!(
            resp_str.contains("200 OK"),
            "auth/config must be public, got: {}",
            resp_str
        );
        assert!(
            resp_str.contains("\"basic\":true"),
            "auth/config should report basic=true: {}",
            resp_str
        );
    }

    // 6. IPC is also protected (uds credentials auto-fill from username/password)
    //    matching stock supervisor unix_http_server behavior.
    let ipc_endpoint = Endpoint::parse(&ipc_path.to_string_lossy());
    let ipc_unauth = SupervisorClient::new(ipc_endpoint.clone(), None);
    let ipc_err = ipc_unauth.status(&[]).await;
    assert!(ipc_err.is_err(), "IPC without credentials must be rejected");
    assert!(
        ipc_err.unwrap_err().to_string().contains("401"),
        "IPC without credentials must return 401"
    );

    let ipc_authed = SupervisorClient::new(ipc_endpoint, None)
        .with_basic_auth("admin".to_string(), "secret123".to_string());
    let ipc_ok = ipc_authed.status(&[]).await;
    assert!(ipc_ok.is_ok(), "IPC with valid basic auth must succeed");

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

/// Performs a raw HTTP/1.1 request over TCP and returns the full response text.
async fn raw_http(port: u16, request: &str) -> String {
    let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .expect("connect tcp");
    tokio::io::AsyncWriteExt::write_all(&mut stream, request.as_bytes())
        .await
        .expect("write request");
    let mut resp = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut stream, &mut resp)
        .await
        .expect("read response");
    String::from_utf8_lossy(&resp).to_string()
}

/// Reads only the status line (for long-lived streams like SSE that never close).
async fn raw_http_status(port: u16, request: &str) -> String {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .expect("connect tcp");
    tokio::io::AsyncWriteExt::write_all(&mut stream, request.as_bytes())
        .await
        .expect("write request");
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut line))
        .await
        .expect("status line timeout")
        .expect("read status line");
    line
}

#[tokio::test]
async fn test_web_session_login_flow_with_basic() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let config_path = temp_dir.path().join("rsupervisord.yaml");
    let port = get_ephemeral_port();
    let ipc_path = get_test_ipc_path("session_login");
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
    std::fs::write(&config_path, &yaml).expect("write config");
    let config = SupervisorConfig::from_file(&config_path).expect("parse config");
    let mut manager = SupervisorManager::new(&config).expect("create manager");
    let server_cancel = CancellationToken::new();
    let server = ServerEngine::new(
        manager.handle(),
        Some(config_path.clone()),
        config.server.clone(),
    );
    let server_token = server_cancel.clone();
    let server_task = tokio::spawn(async move {
        let _ = server.run(server_token).await;
    });
    tokio::time::sleep(Duration::from_millis(200)).await;

    // auth/config is public and reports basic=true, session=false
    let cfg = raw_http(
        port,
        "GET /api/v1/auth/config HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(cfg.contains("200 OK"), "auth/config public: {}", cfg);
    assert!(cfg.contains("\"basic\":true"), "basic flag: {}", cfg);
    assert!(cfg.contains("\"session\":false"), "no session yet: {}", cfg);

    // Bad login -> 401, no session cookie
    let bad_body = r#"{"username":"admin","password":"wrong"}"#;
    let bad_req = format!(
        "POST /api/v1/auth/login HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        bad_body.len(),
        bad_body
    );
    let bad = raw_http(port, &bad_req).await;
    assert!(bad.contains("401 Unauthorized"), "bad login: {}", bad);
    assert!(
        !bad.to_lowercase()
            .contains("set-cookie: rsupervisord_session="),
        "bad login must not issue cookie: {}",
        bad
    );

    // Good login -> 200 + Set-Cookie
    let good_body = r#"{"username":"admin","password":"secret123"}"#;
    let good_req = format!(
        "POST /api/v1/auth/login HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        good_body.len(),
        good_body
    );
    let good = raw_http(port, &good_req).await;
    assert!(good.contains("200 OK"), "good login: {}", good);
    assert!(
        good.to_lowercase()
            .contains("set-cookie: rsupervisord_session="),
        "login must issue session cookie: {}",
        good
    );

    // Extract session cookie value
    let cookie_line = good
        .lines()
        .find(|l| {
            l.to_lowercase()
                .starts_with("set-cookie: rsupervisord_session=")
        })
        .expect("set-cookie header")
        .to_string();
    let session_value = cookie_line
        .split(';')
        .next()
        .expect("cookie pair")
        .split('=')
        .nth(1)
        .expect("cookie value")
        .to_string();
    assert!(!session_value.is_empty());
    let cookie_header = format!("Cookie: rsupervisord_session={}", session_value);

    // Session cookie authorizes protected API
    let authed = raw_http(
        port,
        &format!(
            "GET /api/v1/status HTTP/1.1\r\nHost: localhost\r\n{}\r\nConnection: close\r\n\r\n",
            cookie_header
        ),
    )
    .await;
    assert!(authed.contains("200 OK"), "cookie API access: {}", authed);

    // auth/config now reports session=true
    let cfg2 = raw_http(
        port,
        &format!(
            "GET /api/v1/auth/config HTTP/1.1\r\nHost: localhost\r\n{}\r\nConnection: close\r\n\r\n",
            cookie_header
        ),
    )
    .await;
    assert!(cfg2.contains("\"session\":true"), "session flag: {}", cfg2);

    // SSE accepts the session cookie (status line only; stream stays open)
    let sse = raw_http_status(
        port,
        &format!(
            "GET /api/v1/events HTTP/1.1\r\nHost: localhost\r\n{}\r\nAccept: text/event-stream\r\n\r\n",
            cookie_header
        ),
    )
    .await;
    assert!(sse.contains("200 OK"), "cookie SSE access: {}", sse);

    // Logout clears the session
    let logout_req = format!(
        "POST /api/v1/auth/logout HTTP/1.1\r\nHost: localhost\r\n{}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        cookie_header
    );
    let out = raw_http(port, &logout_req).await;
    assert!(out.contains("200 OK"), "logout: {}", out);

    let after = raw_http(
        port,
        &format!(
            "GET /api/v1/status HTTP/1.1\r\nHost: localhost\r\n{}\r\nConnection: close\r\n\r\n",
            cookie_header
        ),
    )
    .await;
    assert!(
        after.contains("401 Unauthorized"),
        "session invalidated after logout: {}",
        after
    );

    server_cancel.cancel();
    let _ = server_task.await;
    manager.shutdown().await.expect("shutdown manager");
}

#[tokio::test]
async fn test_xmlrpc_or_semantics_basic_and_token_over_tcp() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let config_path = temp_dir.path().join("rsupervisord.yaml");
    let port = get_ephemeral_port();
    let ipc_path = get_test_ipc_path("xmlrpc_token_or");

    let yaml = format!(
        r#"
server:
  uds_path: "{ipc_path}"
  http_bind: "127.0.0.1:{port}"
  auth_token: "tok123"
  username: "admin"
  password: "secret123"

programs: {{}}
"#,
        ipc_path = ipc_path.to_string_lossy().replace('\\', "\\\\"),
        port = port,
    );
    std::fs::write(&config_path, &yaml).expect("write config");
    let config = SupervisorConfig::from_file(&config_path).expect("parse config");
    let mut manager = SupervisorManager::new(&config).expect("create manager");
    let server_cancel = CancellationToken::new();
    let server = ServerEngine::new(
        manager.handle(),
        Some(config_path.clone()),
        config.server.clone(),
    );
    let server_token = server_cancel.clone();
    let server_task = tokio::spawn(async move {
        let _ = server.run(server_token).await;
    });
    tokio::time::sleep(Duration::from_millis(200)).await;

    let xml = r#"<methodCall><methodName>supervisor.getAPIVersion</methodName></methodCall>"#;
    let rpc_noauth = format!(
        "POST /RPC2 HTTP/1.1\r\nHost: localhost\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        xml.len(),
        xml
    );
    use base64::Engine;
    let basic_b64 = base64::engine::general_purpose::STANDARD.encode("admin:secret123");
    let rpc_basic = format!(
        "POST /RPC2 HTTP/1.1\r\nHost: localhost\r\nContent-Type: text/xml\r\nAuthorization: Basic {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        basic_b64,
        xml.len(),
        xml
    );
    let rpc_token = format!(
        "POST /RPC2 HTTP/1.1\r\nHost: localhost\r\nContent-Type: text/xml\r\nAuthorization: Bearer tok123\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        xml.len(),
        xml
    );
    let rpc_wrong = format!(
        "POST /RPC2 HTTP/1.1\r\nHost: localhost\r\nContent-Type: text/xml\r\nAuthorization: Bearer nope\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        xml.len(),
        xml
    );

    // No credentials -> 401 with supervisor realm challenge
    let r1 = raw_http(port, &rpc_noauth).await;
    assert!(r1.contains("401 Unauthorized"), "no creds: {}", r1);
    assert!(
        r1.to_lowercase()
            .contains("www-authenticate: basic realm=\"supervisor\""),
        "RPC challenge realm: {}",
        r1
    );

    // Valid basic -> 200 (OR leg 1)
    let r2 = raw_http(port, &rpc_basic).await;
    assert!(r2.contains("200 OK"), "basic leg: {}", r2);
    assert!(r2.contains("<string>3.0</string>"), "basic body: {}", r2);

    // Valid token -> 200 (OR leg 2; previously broken when basic was also set)
    let r3 = raw_http(port, &rpc_token).await;
    assert!(r3.contains("200 OK"), "token leg: {}", r3);
    assert!(r3.contains("<string>3.0</string>"), "token body: {}", r3);

    // Wrong token -> 401
    let r4 = raw_http(port, &rpc_wrong).await;
    assert!(r4.contains("401 Unauthorized"), "wrong token: {}", r4);

    // IPC enforces the same rules (uds basic auto-filled from username/password)
    let ipc_endpoint = Endpoint::parse(&ipc_path.to_string_lossy());
    let ipc_unauth = SupervisorClient::new(ipc_endpoint.clone(), None);
    assert!(
        ipc_unauth.status(&[]).await.is_err(),
        "IPC without credentials must be rejected"
    );
    let ipc_authed = SupervisorClient::new(ipc_endpoint, None)
        .with_basic_auth("admin".to_string(), "secret123".to_string());
    assert!(
        ipc_authed.status(&[]).await.is_ok(),
        "IPC with valid basic auth must succeed"
    );

    server_cancel.cancel();
    let _ = server_task.await;
    manager.shutdown().await.expect("shutdown manager");
}

#[tokio::test]
async fn test_token_only_ipc_enforced() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let config_path = temp_dir.path().join("rsupervisord.yaml");
    let ipc_path = get_test_ipc_path("token_only_ipc");

    let yaml = format!(
        r#"
server:
  uds_path: "{ipc_path}"
  auth_token: "ipc_secret_token"

programs: {{}}
"#,
        ipc_path = ipc_path.to_string_lossy().replace('\\', "\\\\"),
    );
    std::fs::write(&config_path, &yaml).expect("write config");
    let config = SupervisorConfig::from_file(&config_path).expect("parse config");
    let mut manager = SupervisorManager::new(&config).expect("create manager");
    let server_cancel = CancellationToken::new();
    let server = ServerEngine::new(
        manager.handle(),
        Some(config_path.clone()),
        config.server.clone(),
    );
    let server_token = server_cancel.clone();
    let server_task = tokio::spawn(async move {
        let _ = server.run(server_token).await;
    });
    tokio::time::sleep(Duration::from_millis(200)).await;

    let ipc_endpoint = Endpoint::parse(&ipc_path.to_string_lossy());

    // Without token -> 401 (previously IPC token-only was wide open)
    let unauth = SupervisorClient::new(ipc_endpoint.clone(), None);
    let err = unauth.status(&[]).await;
    assert!(err.is_err(), "IPC without token must be rejected");
    assert!(
        err.unwrap_err().to_string().contains("401"),
        "IPC without token must return 401"
    );

    // With token -> OK
    let authed = SupervisorClient::new(ipc_endpoint, Some("ipc_secret_token".to_string()));
    assert!(
        authed.status(&[]).await.is_ok(),
        "IPC with valid token must succeed"
    );

    server_cancel.cancel();
    let _ = server_task.await;
    manager.shutdown().await.expect("shutdown manager");
}

#[tokio::test]
async fn test_get_program_details_returns_hooks_from_manager() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let config_path = temp_dir.path().join("rsupervisord.yaml");
    let port = get_ephemeral_port();
    let ipc_path = get_test_ipc_path("hooks_dto");

    let yaml = format!(
        r#"
server:
  uds_path: "{ipc_path}"
  http_bind: "127.0.0.1:{port}"

program_defaults:
  autostart: false
  start_secs: 0
  stop_wait_secs: 1

programs:
  single_hooked:
    command: |-
      {cmd_single}
    pre_start: "echo single-pre-start"
    pre_stop: "echo single-pre-stop"
  multi_hooked:
    command: |-
      {cmd_multi}
    numprocs: 2
    pre_start: "echo multi-pre-start"
    pre_stop: "echo multi-pre-stop"
"#,
        ipc_path = ipc_path.to_string_lossy().replace('\\', "\\\\"),
        port = port,
        cmd_single = get_worker_command("single", 5),
        cmd_multi = get_worker_command("multi", 5),
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

    let client = SupervisorClient::new(Endpoint::Tcp(format!("127.0.0.1:{}", port)), None);

    let single = client
        .get_program("single_hooked")
        .await
        .expect("get single_hooked");
    assert_eq!(single.pre_start.as_deref(), Some("echo single-pre-start"));
    assert_eq!(single.pre_stop.as_deref(), Some("echo single-pre-stop"));

    // numprocs > 1: instance key is multi_hooked:0 (raw disk map lookup missed it)
    let multi0 = client
        .get_program("multi_hooked:0")
        .await
        .expect("get multi_hooked:0");
    assert_eq!(multi0.pre_start.as_deref(), Some("echo multi-pre-start"));
    assert_eq!(multi0.pre_stop.as_deref(), Some("echo multi-pre-stop"));

    let multi1 = client
        .get_program("multi_hooked:1")
        .await
        .expect("get multi_hooked:1");
    assert_eq!(multi1.pre_start.as_deref(), Some("echo multi-pre-start"));

    server_cancel.cancel();
    let _ = server_task.await;
    manager.shutdown().await.expect("shutdown manager");
}
