// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use clap::Parser;
use rsupervisord::cli::args::{CliArgs, CliCommand};
use rsupervisord::cli::client::SupervisorClient;
use rsupervisord::cli::transport::Endpoint;
use rsupervisord::control::protocol::{
    ActionResponse, ApiResponse, LogLinesResponse, ProgramStatusDto, ReloadResponse,
};
use rsupervisord::program::state::ProgramState;
use rsupervisord::service::ServiceOp;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[test]
fn test_cli_args_parsing() {
    let args = CliArgs::parse_from(["supervisorctl", "status"]);
    assert!(matches!(args.command, Some(CliCommand::Status { names }) if names.is_empty()));

    let args = CliArgs::parse_from(["supervisorctl", "status", "web", "api"]);
    assert!(
        matches!(args.command, Some(CliCommand::Status { names }) if names == vec!["web", "api"])
    );

    let args = CliArgs::parse_from(["supervisorctl", "start", "web", "--async", "-t", "15"]);
    assert!(matches!(
        args.command,
        Some(CliCommand::Start {
            names,
            r#async: true,
            timeout: 15
        }) if names == vec!["web"]
    ));

    let args = CliArgs::parse_from(["supervisorctl", "stop", "api", "-a"]);
    assert!(matches!(
        args.command,
        Some(CliCommand::Stop {
            names,
            r#async: true,
            ..
        }) if names == vec!["api"]
    ));

    let args = CliArgs::parse_from(["supervisorctl", "tail", "web", "-f", "-n", "50"]);
    assert!(matches!(
        args.command,
        Some(CliCommand::Tail {
            name,
            follow: true,
            lines: Some(50),
            ..
        }) if name == "web"
    ));

    let args = CliArgs::parse_from(["supervisorctl", "tail", "all", "-f"]);
    assert!(matches!(
        args.command,
        Some(CliCommand::Tail {
            name,
            follow: true,
            ..
        }) if name == "all"
    ));

    let args = CliArgs::parse_from(["supervisorctl", "events"]);
    assert!(matches!(args.command, Some(CliCommand::Events)));

    let args = CliArgs::parse_from(["supervisorctl", "stdin", "web", "command_line\n"]);
    assert!(matches!(
        args.command,
        Some(CliCommand::Stdin { name, chars }) if name == "web" && chars == "command_line\n"
    ));

    let args = CliArgs::parse_from(["supervisorctl", "send-stdin", "worker", "echo ping"]);
    assert!(matches!(
        args.command,
        Some(CliCommand::Stdin { name, chars }) if name == "worker" && chars == "echo ping"
    ));

    let args = CliArgs::parse_from(["supervisorctl", "service", "install"]);
    assert!(matches!(
        args.command,
        Some(CliCommand::Service {
            op: ServiceOp::Install
        })
    ));

    let args = CliArgs::parse_from(["supervisorctl", "service", "stop", "-c", "/etc/cfg.yaml"]);
    assert!(matches!(
        args.command,
        Some(CliCommand::Service {
            op: ServiceOp::Stop
        })
    ));
    assert_eq!(
        args.config.as_deref(),
        Some(std::path::Path::new("/etc/cfg.yaml"))
    );

    // Bare `service` requires an operation.
    assert!(CliArgs::try_parse_from(["supervisorctl", "service"]).is_err());
}

#[test]
fn test_cli_help_bridges_to_clap() {
    // Bare `help` renders the full command surface derived from CliArgs.
    let code = rsupervisord::cli::commands::handle_help(None, None).unwrap();
    assert_eq!(code, 0);

    // Per-command help, including alias-only names, with a bin_name override
    // (the `supervisord ctl ...` invocation form).
    let code =
        rsupervisord::cli::commands::handle_help(Some("status"), Some("supervisord ctl")).unwrap();
    assert_eq!(code, 0);
    let code = rsupervisord::cli::commands::handle_help(Some("send-stdin"), None).unwrap();
    assert_eq!(code, 0);

    // Unknown command reports an error exit code (wording is UX, not asserted).
    let code = rsupervisord::cli::commands::handle_help(Some("no-such-cmd"), None).unwrap();
    assert_eq!(code, 1);
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
                } else if request.starts_with("POST /api/v1/config/reload") {
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
                } else if request.starts_with("POST /api/v1/reload") {
                    (
                        "200 OK",
                        serde_json::to_string(&ApiResponse::ok("Daemon restarted successfully"))
                            .unwrap(),
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

    // 3. Test config reload & daemon reload
    let reload_res = client.config_reload().await.unwrap();
    assert_eq!(reload_res.unchanged, vec!["mysql"]);
    assert_eq!(reload_res.added, vec!["redis"]);
    assert_eq!(reload_res.modified, vec!["web"]);

    let daemon_reload_msg = client.reload().await.unwrap();
    assert!(daemon_reload_msg.contains("restarted") || daemon_reload_msg.contains("OK"));

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

/// Candidate chain: first reachable endpoint wins; unreachable ones are skipped.
#[tokio::test]
async fn test_cli_candidate_chain_falls_through_to_reachable() {
    use rsupervisord::cli::EndpointCandidate;

    // Real mock daemon on an ephemeral TCP port.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = [0u8; 4096];
            let _ = socket.read(&mut buf).await;
            let body = serde_json::to_string(&ApiResponse::ok(vec![ProgramStatusDto {
                name: "ok".into(),
                group: "g".into(),
                state: "RUNNING".into(),
                health: "-".into(),
                pid: "1".into(),
                cpu: "-".into(),
                mem: "-".into(),
                uptime: "-".into(),
                cron: "-".into(),
                description: "-".into(),
            }]))
            .unwrap();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = socket.write_all(resp.as_bytes()).await;
        }
    });

    // Dead path first (will fail not-found), then live TCP.
    let dead = Endpoint::Ipc(std::path::PathBuf::from(
        r"\\.\pipe\rsupervisord-candidate-dead",
    ));
    let candidates = vec![
        EndpointCandidate {
            endpoint: dead,
            basic: None,
        },
        EndpointCandidate {
            endpoint: Endpoint::Tcp(addr.to_string()),
            basic: None,
        },
    ];

    let client = SupervisorClient::new_with_candidates(candidates, None, None);
    let res = client.status(&[]).await;
    assert!(
        res.is_ok(),
        "chain must fall through to TCP: {:?}",
        res.err()
    );
}

/// Candidate chain fail-closed: authorization errors must not advance; only
/// not-found/refused may fall through.
#[test]
fn test_cli_candidate_chain_auth_fail_closed_classifiers() {
    use rsupervisord::cli::transport::{is_authorization_error, is_retryable_not_found};

    // Authorization errors: fail closed (do not advance to next candidate).
    let denied = std::io::Error::from_raw_os_error(5); // ERROR_ACCESS_DENIED
    assert!(is_authorization_error(&denied));
    assert!(!is_retryable_not_found(&denied));

    let eacces = std::io::Error::from_raw_os_error(13); // Unix EACCES
    assert!(is_authorization_error(&eacces));

    // Not-found / refused: safe to advance to the next candidate.
    let missing = std::io::Error::from_raw_os_error(2); // ERROR_FILE_NOT_FOUND / ENOENT
    assert!(!is_authorization_error(&missing));
    assert!(is_retryable_not_found(&missing));

    let refused = std::io::Error::from_raw_os_error(111); // Unix ECONNREFUSED
    assert!(is_retryable_not_found(&refused));
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

#[test]
fn test_cli_args_apply_field_independent_overrides() {
    use rsupervisord::config::CtlConfig;

    let base = || CtlConfig {
        serverurl: Some("http://cfg:1".into()),
        username: Some("cfgu".into()),
        password: Some("cfgp".into()),
        auth_token: Some("cfgk".into()),
    };

    // -s only: overrides serverurl, leaves credentials untouched.
    let mut ctl = base();
    let args = CliArgs::parse_from(["supervisorctl", "-s", "http://cli:2", "status"]);
    args.apply(&mut ctl);
    assert_eq!(ctl.serverurl.as_deref(), Some("http://cli:2"));
    assert_eq!(ctl.username.as_deref(), Some("cfgu"));
    assert_eq!(ctl.password.as_deref(), Some("cfgp"));
    assert_eq!(ctl.auth_token.as_deref(), Some("cfgk"));

    // -k only: overrides token, leaves serverurl/credentials untouched.
    let mut ctl = base();
    let args = CliArgs::parse_from(["supervisorctl", "-k", "newtoken", "status"]);
    args.apply(&mut ctl);
    assert_eq!(ctl.auth_token.as_deref(), Some("newtoken"));
    assert_eq!(ctl.serverurl.as_deref(), Some("http://cfg:1"));
    assert_eq!(ctl.username.as_deref(), Some("cfgu"));

    // -u alone: password becomes empty string (pair override).
    let mut ctl = base();
    let args = CliArgs::parse_from(["supervisorctl", "-u", "cliu", "status"]);
    args.apply(&mut ctl);
    assert_eq!(ctl.username.as_deref(), Some("cliu"));
    assert_eq!(ctl.password.as_deref(), Some(""));

    // -p alone: username becomes empty string.
    let mut ctl = base();
    let args = CliArgs::parse_from(["supervisorctl", "-p", "clip", "status"]);
    args.apply(&mut ctl);
    assert_eq!(ctl.username.as_deref(), Some(""));
    assert_eq!(ctl.password.as_deref(), Some("clip"));

    // No flags: nothing changes.
    let mut ctl = base();
    let args = CliArgs::parse_from(["supervisorctl", "status"]);
    args.apply(&mut ctl);
    assert_eq!(ctl, base());
}
