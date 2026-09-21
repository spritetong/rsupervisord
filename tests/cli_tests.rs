// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use clap::Parser;
use rsupervisord::cli::args::{CliArgs, CliCommand};
use rsupervisord::cli::client::SupervisorClient;
use rsupervisord::cli::transport::Endpoint;
use rsupervisord::control::protocol::{
    ActionResponse, ApiResponse, LogLinesResponse, ProgramStatusDto, ReloadResponse,
};
use rsupervisord::program::state::ProgramState;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[test]
fn test_cli_args_parsing() {
    let args = CliArgs::parse_from(["rsupervisorctl", "status"]);
    assert!(matches!(args.command, Some(CliCommand::Status { names }) if names.is_empty()));

    let args = CliArgs::parse_from(["rsupervisorctl", "status", "web", "api"]);
    assert!(
        matches!(args.command, Some(CliCommand::Status { names }) if names == vec!["web", "api"])
    );

    let args = CliArgs::parse_from(["rsupervisorctl", "start", "web", "--async", "-t", "15"]);
    assert!(matches!(
        args.command,
        Some(CliCommand::Start {
            names,
            r#async: true,
            timeout: 15
        }) if names == vec!["web"]
    ));

    let args = CliArgs::parse_from(["rsupervisorctl", "stop", "api", "-a"]);
    assert!(matches!(
        args.command,
        Some(CliCommand::Stop {
            names,
            r#async: true,
            ..
        }) if names == vec!["api"]
    ));

    let args = CliArgs::parse_from(["rsupervisorctl", "tail", "web", "-f", "-n", "50"]);
    assert!(matches!(
        args.command,
        Some(CliCommand::Tail {
            name,
            follow: true,
            lines: 50
        }) if name == "web"
    ));

    let args = CliArgs::parse_from(["rsupervisorctl", "tail", "all", "-f"]);
    assert!(matches!(
        args.command,
        Some(CliCommand::Tail {
            name,
            follow: true,
            ..
        }) if name == "all"
    ));

    let args = CliArgs::parse_from(["rsupervisorctl", "events"]);
    assert!(matches!(args.command, Some(CliCommand::Events)));

    let args = CliArgs::parse_from(["rsupervisorctl", "stdin", "web", "command_line\n"]);
    assert!(matches!(
        args.command,
        Some(CliCommand::Stdin { name, chars }) if name == "web" && chars == "command_line\n"
    ));

    let args = CliArgs::parse_from(["rsupervisorctl", "send-stdin", "worker", "echo ping"]);
    assert!(matches!(
        args.command,
        Some(CliCommand::Stdin { name, chars }) if name == "worker" && chars == "echo ping"
    ));
}

#[tokio::test]
async fn test_cli_client_against_mock_daemon() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        loop {
            let (mut socket, _) = match listener.accept().await {
                Ok(conn) => conn,
                Err(_) => break,
            };

            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]);

                let (status, body) = if request.starts_with("GET /api/v1/status") {
                    let dtos = vec![
                        ProgramStatusDto {
                            name: "mysql".to_string(),
                            group: "mysql".to_string(),
                            state: "RUNNING".to_string(),
                            health: "HEALTHY".to_string(),
                            pid: "1234".to_string(),
                            cpu: "0.5%".to_string(),
                            mem: "15.2 MB".to_string(),
                            uptime: "5m".to_string(),
                            cron: "-".to_string(),
                            description: "Running for 5m".to_string(),
                        },
                        ProgramStatusDto {
                            name: "web".to_string(),
                            group: "web".to_string(),
                            state: "STOPPED".to_string(),
                            health: "-".to_string(),
                            pid: "-".to_string(),
                            cpu: "-".to_string(),
                            mem: "-".to_string(),
                            uptime: "-".to_string(),
                            cron: "-".to_string(),
                            description: "Stopped".to_string(),
                        },
                    ];
                    (
                        "200 OK",
                        serde_json::to_string(&ApiResponse::ok(dtos)).unwrap(),
                    )
                } else if request.starts_with("POST /api/v1/programs/web/start") {
                    let action = ActionResponse {
                        name: "web".to_string(),
                        state: ProgramState::Running,
                        pid: Some(5678),
                        description: "Running".to_string(),
                        elapsed_ms: 120,
                    };
                    (
                        "200 OK",
                        serde_json::to_string(&ApiResponse::ok(action)).unwrap(),
                    )
                } else if request.starts_with("POST /api/v1/reload") {
                    let reload = ReloadResponse {
                        added: vec!["redis".to_string()],
                        removed: vec![],
                        modified: vec!["web".to_string()],
                        unchanged: vec!["mysql".to_string()],
                    };
                    (
                        "200 OK",
                        serde_json::to_string(&ApiResponse::ok(reload)).unwrap(),
                    )
                } else if request.starts_with("GET /api/v1/programs/web/logs") {
                    let logs = LogLinesResponse {
                        name: "web".to_string(),
                        lines: vec!["hello".to_string(), "world".to_string()],
                    };
                    (
                        "200 OK",
                        serde_json::to_string(&ApiResponse::ok(logs)).unwrap(),
                    )
                } else {
                    ("404 Not Found", "{}".to_string())
                };

                let response = format!(
                    "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    status,
                    body.len(),
                    body
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });

    let client = SupervisorClient::new(Endpoint::Tcp(addr.to_string()), None);

    // 1. Test status
    let status_list = client.status(&[]).await.unwrap();
    assert_eq!(status_list.len(), 2);
    assert_eq!(status_list[0].name, "mysql");
    assert_eq!(status_list[0].state, "RUNNING");

    let filtered = client.status(&["mysql".to_string()]).await.unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].name, "mysql");

    // 2. Test start
    let start_res = client.start("web", true, 10).await.unwrap();
    assert_eq!(start_res.len(), 1);
    assert_eq!(start_res[0].name, "web");
    assert_eq!(start_res[0].pid, Some(5678));

    // 3. Test reload
    let reload_res = client.reload().await.unwrap();
    assert_eq!(reload_res.unchanged, vec!["mysql"]);
    assert_eq!(reload_res.added, vec!["redis"]);
    assert_eq!(reload_res.modified, vec!["web"]);

    // 4. Test read logs
    let logs = client.read_logs("web", 10).await.unwrap();
    assert_eq!(logs, vec!["hello", "world"]);
}

#[tokio::test]
async fn test_cli_command_handlers_execution() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            let dtos = vec![ProgramStatusDto {
                name: "demo".to_string(),
                group: "demo".to_string(),
                state: "RUNNING".to_string(),
                health: "HEALTHY".to_string(),
                pid: "42".to_string(),
                cpu: "1.2%".to_string(),
                mem: "8.5 MB".to_string(),
                uptime: "10s".to_string(),
                cron: "-".to_string(),
                description: "Demo task".to_string(),
            }];
            let body = serde_json::to_string(&ApiResponse::ok(dtos)).unwrap();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = socket.write_all(resp.as_bytes()).await;
        }
    });

    let client = SupervisorClient::new(Endpoint::Tcp(addr.to_string()), None);
    let res = rsupervisord::cli::commands::handle_status(&client, &[]).await;
    assert!(res.is_ok());
}

#[tokio::test]
async fn test_cli_connection_refused_error() {
    // Pick an unused port
    let client = SupervisorClient::new(Endpoint::Tcp("127.0.0.1:54321".to_string()), None);
    let res = client.status(&[]).await;
    assert!(res.is_err());
    let err_msg = res.unwrap_err().to_string();
    assert!(
        err_msg.contains("Cannot connect")
            || err_msg.contains("Connection timed out")
            || err_msg.contains("refused")
    );
}

#[tokio::test]
async fn test_cli_handle_stdin() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            let body =
                serde_json::to_string(&ApiResponse::ok(serde_json::json!({ "success": true })))
                    .unwrap();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = socket.write_all(resp.as_bytes()).await;
        }
    });

    let client = SupervisorClient::new(Endpoint::Tcp(addr.to_string()), None);
    let res = rsupervisord::cli::commands::handle_stdin(&client, "worker", "ping\n").await;
    assert!(res.is_ok());
}
