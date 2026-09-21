// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use rsupervisord::compat::xmlrpc::fault::FaultCode;
use rsupervisord::config::SupervisorConfig;
use rsupervisord::manager::SupervisorManager;
use rsupervisord::server::api::{AppState, build_router};
use rsupervisord::server::auth::BasicAuthConfig;
use std::time::Duration;
use tower::ServiceExt;

fn get_sleep_cmd(secs: u64) -> String {
    #[cfg(unix)]
    {
        format!("sh -c 'while true; do sleep {}; done'", secs)
    }
    #[cfg(windows)]
    {
        format!(
            "powershell.exe -NoProfile -Command \"while ($true) {{ Start-Sleep -Seconds {} }}\"",
            secs
        )
    }
}

async fn call_rpc(
    app: &axum::Router,
    xml_req: &str,
    auth_header: Option<&str>,
) -> (StatusCode, String) {
    let mut req_builder = Request::builder()
        .method("POST")
        .uri("/RPC2")
        .header("Content-Type", "text/xml");

    if let Some(auth) = auth_header {
        req_builder = req_builder.header("Authorization", auth);
    }

    let req = req_builder.body(Body::from(xml_req.to_string())).unwrap();
    let response = app.clone().oneshot(req).await.expect("execute request");

    let status = response.status();
    let body_bytes = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("read body");
    let body_str = String::from_utf8(body_bytes.to_vec()).expect("utf8 string");
    (status, body_str)
}

fn parse_fault(xml: &str) -> (i32, String) {
    assert!(
        xml.contains("<fault>"),
        "XML does not contain fault: {}",
        xml
    );
    // Extract faultCode and faultString
    let code = xml
        .split("<name>faultCode</name>")
        .nth(1)
        .and_then(|s| s.split("<int>").nth(1))
        .and_then(|s| s.split("</int>").next())
        .expect("faultCode")
        .trim()
        .parse::<i32>()
        .expect("parse int");

    let msg = xml
        .split("<name>faultString</name>")
        .nth(1)
        .and_then(|s| s.split("<string>").nth(1))
        .and_then(|s| s.split("</string>").next())
        .expect("faultString")
        .to_string();

    (code, msg)
}

#[tokio::test]
async fn test_xmlrpc_meta_methods() {
    let yaml = r#"
programs:
  alpha:
    command: "echo test"
    autostart: false
"#;
    let config: SupervisorConfig = serde_yaml::from_str(yaml).expect("parse yaml");
    let manager = SupervisorManager::new(&config).expect("create manager");
    let state = AppState::new(manager.handle(), None, None, None);
    let app = build_router(state);

    // 1. getAPIVersion
    let req = "<methodCall><methodName>supervisor.getAPIVersion</methodName><params></params></methodCall>";
    let (status, body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<string>3.0</string>"));

    // 2. getVersion (alias)
    let req = "<methodCall><methodName>supervisor.getVersion</methodName></methodCall>";
    let (status, body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<string>3.0</string>"));

    // 3. getSupervisorVersion
    let req = "<methodCall><methodName>supervisor.getSupervisorVersion</methodName></methodCall>";
    let (status, body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<string>4.2.5</string>"));

    // 4. getIdentification
    let req = "<methodCall><methodName>supervisor.getIdentification</methodName></methodCall>";
    let (status, body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<string>rsupervisord-compat</string>"));

    // 5. getState
    let req = "<methodCall><methodName>supervisor.getState</methodName></methodCall>";
    let (status, body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<name>statename</name>"));
    assert!(body.contains("<string>RUNNING</string>"));
    assert!(body.contains("<name>statecode</name>"));
    assert!(body.contains("<int>1</int>"));

    // 6. getPID
    let req = "<methodCall><methodName>supervisor.getPID</methodName></methodCall>";
    let (status, body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<int>"));
}

#[tokio::test]
async fn test_xmlrpc_system_introspection_and_multicall() {
    let yaml = r#"
programs:
  alpha:
    command: "echo test"
    autostart: false
"#;
    let config: SupervisorConfig = serde_yaml::from_str(yaml).expect("parse yaml");
    let manager = SupervisorManager::new(&config).expect("create manager");
    let state = AppState::new(manager.handle(), None, None, None);
    let app = build_router(state);

    // 1. system.listMethods
    let req = "<methodCall><methodName>system.listMethods</methodName></methodCall>";
    let (status, body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<string>supervisor.getAPIVersion</string>"));
    assert!(body.contains("<string>system.multicall</string>"));

    // 2. system.methodHelp
    let req = "<methodCall><methodName>system.methodHelp</methodName><params><param><value><string>supervisor.getAPIVersion</string></value></param></params></methodCall>";
    let (status, body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.to_lowercase().contains("version"));

    // 3. system.methodHelp unknown
    let req = "<methodCall><methodName>system.methodHelp</methodName><params><param><value><string>supervisor.unknown</string></value></param></params></methodCall>";
    let (status, body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);
    let (code, _) = parse_fault(&body);
    assert_eq!(code, FaultCode::SignatureUnsupported.code());

    // 4. system.methodSignature
    let req = "<methodCall><methodName>system.methodSignature</methodName><params><param><value><string>supervisor.getAPIVersion</string></value></param></params></methodCall>";
    let (status, body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<array>"));

    // 5. system.multicall
    let req = r#"<methodCall>
      <methodName>system.multicall</methodName>
      <params>
        <param>
          <value>
            <array>
              <data>
                <value>
                  <struct>
                    <member><name>methodName</name><value><string>supervisor.getAPIVersion</string></value></member>
                    <member><name>params</name><value><array><data></data></array></value></member>
                  </struct>
                </value>
                <value>
                  <struct>
                    <member><name>methodName</name><value><string>supervisor.nonExistentMethod</string></value></member>
                    <member><name>params</name><value><array><data></data></array></value></member>
                  </struct>
                </value>
              </data>
            </array>
          </value>
        </param>
      </params>
    </methodCall>"#;
    let (status, body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<string>3.0</string>"));
    assert!(body.contains("<name>faultCode</name>"));
    assert!(body.contains("<int>1</int>"));

    // 6. system.multicall recursion denied
    let req_recursive = r#"<methodCall>
      <methodName>system.multicall</methodName>
      <params>
        <param>
          <value>
            <array>
              <data>
                <value>
                  <struct>
                    <member><name>methodName</name><value><string>system.multicall</string></value></member>
                    <member><name>params</name><value><array><data></data></array></value></member>
                  </struct>
                </value>
              </data>
            </array>
          </value>
        </param>
      </params>
    </methodCall>"#;
    let (status, body) = call_rpc(&app, req_recursive, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<int>2</int>")); // INCORRECT_PARAMETERS
    assert!(body.contains("Recursive multicall"));
}

#[tokio::test]
async fn test_xmlrpc_process_info_and_lifecycle() {
    let cmd = get_sleep_cmd(30);
    let yaml = format!(
        r#"
groups:
  services:
    programs:
      - ticker
programs:
  ticker:
    command: |-
      {cmd}
    autostart: true
    start_secs: 0
  echo:
    command: "echo one-shot"
    autostart: false
    autorestart: never
"#
    );
    let config: SupervisorConfig = serde_yaml::from_str(&yaml).expect("parse yaml");
    let mut manager = SupervisorManager::new(&config).expect("create manager");
    manager.handle().start_all().await.expect("start all");
    let state = AppState::new(manager.handle(), None, None, None);
    let app = build_router(state);

    tokio::time::sleep(Duration::from_millis(500)).await;

    // 1. getAllProcessInfo
    let req = "<methodCall><methodName>supervisor.getAllProcessInfo</methodName></methodCall>";
    let (status, body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<name>name</name>"));
    assert!(body.contains("<string>ticker</string>"));
    assert!(body.contains("<name>group</name>"));
    assert!(body.contains("<string>services</string>"));
    assert!(body.contains("<name>statename</name>"));
    assert!(body.contains("<string>RUNNING</string>"));
    assert!(body.contains("<name>stdout_logfile</name>"));
    assert!(body.contains("<name>stderr_logfile</name>"));
    assert!(body.contains("<name>description</name>"));

    // 2. getProcessInfo with namespec
    let req = "<methodCall><methodName>supervisor.getProcessInfo</methodName><params><param><value><string>services:ticker</string></value></param></params></methodCall>";
    let (status, body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<string>ticker</string>"));
    assert!(body.contains("<string>RUNNING</string>"));

    // 3. getProcessInfo bad name -> Fault 10
    let req = "<methodCall><methodName>supervisor.getProcessInfo</methodName><params><param><value><string>unknown_proc</string></value></param></params></methodCall>";
    let (status, body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);
    let (code, msg) = parse_fault(&body);
    assert_eq!(code, FaultCode::BadName.code());
    assert!(msg.contains("BAD_NAME"));

    // 4. startProcess on already started -> Fault 60
    let req = "<methodCall><methodName>supervisor.startProcess</methodName><params><param><value><string>services:ticker</string></value></param></params></methodCall>";
    let (status, body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);
    let (code, msg) = parse_fault(&body);
    assert_eq!(code, FaultCode::AlreadyStarted.code());
    assert!(msg.contains("ALREADY_STARTED"));

    // 5. signalProcess with HUP
    let req = "<methodCall><methodName>supervisor.signalProcess</methodName><params><param><value><string>services:ticker</string></value></param><param><value><string>HUP</string></value></param></params></methodCall>";
    let (status, body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<boolean>1</boolean>"));

    // 6. signalProcess with invalid signal -> Fault 11
    let req = "<methodCall><methodName>supervisor.signalProcess</methodName><params><param><value><string>services:ticker</string></value></param><param><value><string>NOTASIGNAL</string></value></param></params></methodCall>";
    let (status, body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);
    let (code, msg) = parse_fault(&body);
    assert_eq!(code, FaultCode::BadSignal.code());
    assert!(msg.contains("BAD_SIGNAL"));

    // 7. stopProcess
    let req = "<methodCall><methodName>supervisor.stopProcess</methodName><params><param><value><string>services:ticker</string></value></param></params></methodCall>";
    let (status, body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<boolean>1</boolean>"));

    // 8. stopProcess when already stopped -> Fault 70
    let req = "<methodCall><methodName>supervisor.stopProcess</methodName><params><param><value><string>services:ticker</string></value></param></params></methodCall>";
    let (status, body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);
    let (code, msg) = parse_fault(&body);
    assert_eq!(code, FaultCode::NotRunning.code());
    assert!(msg.contains("NOT_RUNNING"));

    // 9. signalProcess when not running -> Fault 70
    let req = "<methodCall><methodName>supervisor.signalProcess</methodName><params><param><value><string>services:ticker</string></value></param><param><value><string>HUP</string></value></param></params></methodCall>";
    let (status, body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);
    let (code, _) = parse_fault(&body);
    assert_eq!(code, FaultCode::NotRunning.code());

    // Clean up
    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_xmlrpc_basic_auth_and_cve_protection() {
    let yaml = r#"
programs:
  dummy:
    command: "echo dummy"
    autostart: false
"#;
    let config: SupervisorConfig = serde_yaml::from_str(yaml).expect("parse yaml");
    let manager = SupervisorManager::new(&config).expect("create manager");
    let basic_auth = BasicAuthConfig::new(Some("admin".to_string()), Some("secret123".to_string()));
    let state = AppState::new(manager.handle(), None, None, basic_auth);
    let app = build_router(state);

    let req = "<methodCall><methodName>supervisor.getAPIVersion</methodName></methodCall>";

    // 1. Without credentials -> 401 Unauthorized
    let (status, _) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // 2. With invalid credentials -> 401 Unauthorized
    let (status, _) = call_rpc(&app, req, Some("Basic YWRtaW46d3Jvbmc=")).await; // admin:wrong
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // 3. With valid credentials -> 200 OK
    let (status, body) = call_rpc(&app, req, Some("Basic YWRtaW46c2VjcmV0MTIz")).await; // admin:secret123
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<string>3.0</string>"));

    // 4. CVE-2017-11610 protection: private method starting with '_'
    let req_private = "<methodCall><methodName>_privateMethod</methodName></methodCall>";
    let (status, body) = call_rpc(&app, req_private, Some("Basic YWRtaW46c2VjcmV0MTIz")).await;
    assert_eq!(status, StatusCode::OK); // XML-RPC faults return 200 OK
    let (code, _) = parse_fault(&body);
    assert_eq!(code, FaultCode::UnknownMethod.code());

    // 5. CVE-2017-11610 protection: 3 segments
    let req_sub = "<methodCall><methodName>supervisor.sub.method</methodName></methodCall>";
    let (status, body) = call_rpc(&app, req_sub, Some("Basic YWRtaW46c2VjcmV0MTIz")).await;
    assert_eq!(status, StatusCode::OK);
    let (code, _) = parse_fault(&body);
    assert_eq!(code, FaultCode::UnknownMethod.code());
}

#[tokio::test]
async fn test_xmlrpc_group_and_all_operations() {
    let cmd = get_sleep_cmd(30);
    let yaml = format!(
        r#"
groups:
  services:
    programs:
      - p1
      - p2
programs:
  p1:
    command: |-
      {cmd}
    autostart: true
    start_secs: 0
  p2:
    command: |-
      {cmd}
    autostart: true
    start_secs: 0
"#
    );
    let config: SupervisorConfig = serde_yaml::from_str(&yaml).expect("parse yaml");
    let mut manager = SupervisorManager::new(&config).expect("create manager");
    manager.handle().start_all().await.expect("start all");
    let state = AppState::new(manager.handle(), None, None, None);
    let app = build_router(state);

    tokio::time::sleep(Duration::from_millis(500)).await;

    // 1. stopProcessGroup
    let req = "<methodCall><methodName>supervisor.stopProcessGroup</methodName><params><param><value><string>services</string></value></param></params></methodCall>";
    let (status, body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<name>status</name>"));
    assert!(body.contains("<int>80</int>")); // SUCCESS code

    // 2. startProcessGroup
    let req = "<methodCall><methodName>supervisor.startProcessGroup</methodName><params><param><value><string>services</string></value></param></params></methodCall>";
    let (status, body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<name>status</name>"));
    assert!(body.contains("<int>80</int>"));

    // 3. stopAllProcesses
    let req = "<methodCall><methodName>supervisor.stopAllProcesses</methodName></methodCall>";
    let (status, body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<name>status</name>"));

    // 4. startAllProcesses
    let req = "<methodCall><methodName>supervisor.startAllProcesses</methodName></methodCall>";
    let (status, body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<name>status</name>"));

    // 5. signalProcessGroup with probe (0)
    let req = "<methodCall><methodName>supervisor.signalProcessGroup</methodName><params><param><value><string>services</string></value></param><param><value><string>0</string></value></param></params></methodCall>";
    let (status, _body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);

    // 6. signalAllProcesses with probe (0)
    let req = "<methodCall><methodName>supervisor.signalAllProcesses</methodName><params><param><value><string>0</string></value></param></params></methodCall>";
    let (status, _body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);

    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_xmlrpc_logs_and_config() {
    let yaml = r#"
programs:
  alpha:
    command: "echo test-log-output"
    autostart: false
"#;
    let config: SupervisorConfig = serde_yaml::from_str(yaml).expect("parse yaml");
    let mut manager = SupervisorManager::new(&config).expect("create manager");
    let state = AppState::new(manager.handle(), None, None, None);
    let app = build_router(state);

    // 1. getAllConfigInfo
    let req = "<methodCall><methodName>supervisor.getAllConfigInfo</methodName></methodCall>";
    let (status, body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<name>name</name>"));
    assert!(body.contains("<string>alpha</string>"));
    assert!(body.contains("<name>group</name>"));
    assert!(body.contains("<name>autostart</name>"));

    // 2. readMainLog / readLog
    let req = "<methodCall><methodName>supervisor.readLog</methodName><params><param><value><int>0</int></value><param><value><int>1024</int></value></param></params></methodCall>";
    let (status, body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<string>"));

    // 3. clearLog
    let req = "<methodCall><methodName>supervisor.clearLog</methodName></methodCall>";
    let (status, body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<boolean>1</boolean>"));

    // 4. clearProcessLogs
    let req = "<methodCall><methodName>supervisor.clearProcessLogs</methodName><params><param><value><string>alpha</string></value></param></params></methodCall>";
    let (status, body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<boolean>1</boolean>"));

    // 5. Unsupported runtime group modification stubs
    let req = "<methodCall><methodName>supervisor.addProcessGroup</methodName><params><param><value><string>dyn_grp</string></value></param></params></methodCall>";
    let (status, body) = call_rpc(&app, req, None).await;
    assert_eq!(status, StatusCode::OK);
    let (code, msg) = parse_fault(&body);
    assert_eq!(code, FaultCode::Failed.code());
    assert!(msg.contains("not supported"));

    manager.shutdown().await.unwrap();
}
