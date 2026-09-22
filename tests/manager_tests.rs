// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use rsupervisord::config::SupervisorConfig;
use rsupervisord::manager::SupervisorManager;
use rsupervisord::program::{ProgramState, StopSignal};

fn get_sleep_cmd(secs: u64) -> String {
    #[cfg(unix)]
    {
        format!("sleep {}", secs)
    }
    #[cfg(windows)]
    {
        format!(
            "powershell.exe -NoProfile -Command Start-Sleep -Seconds {}",
            secs
        )
    }
}

fn get_exit_cmd(code: i32) -> String {
    #[cfg(unix)]
    {
        format!("sh -c 'exit {}'", code)
    }
    #[cfg(windows)]
    {
        format!("cmd.exe /C exit {}", code)
    }
}

#[tokio::test]
async fn test_manager_start_and_stop_all() {
    let yaml = format!(
        r#"
program_defaults:
  autostart: true
  start_secs: 0
  stop_wait_secs: 2

programs:
  service_a:
    command: "{cmd_a}"
    priority: 10
  service_b:
    command: "{cmd_b}"
    priority: 20
    depends_on: ["service_a"]
"#,
        cmd_a = get_sleep_cmd(10),
        cmd_b = get_sleep_cmd(10),
    );

    let config = SupervisorConfig::from_yaml_str(&yaml).expect("parse yaml");
    let mut manager = SupervisorManager::new(&config).expect("create manager");
    let handle = manager.handle();

    // Start all programs
    handle.start_all().await.expect("start all");

    let status_a = handle.get_status("service_a").await.expect("status a");
    let status_b = handle.get_status("service_b").await.expect("status b");

    assert_eq!(status_a.state, ProgramState::Running);
    assert_eq!(status_b.state, ProgramState::Running);
    assert!(status_a.pid.is_some());
    assert!(status_b.pid.is_some());

    // Stop all programs
    handle.stop_all(None).await.expect("stop all");

    let status_a_stopped = handle.get_status("service_a").await.expect("status a");
    let status_b_stopped = handle.get_status("service_b").await.expect("status b");

    assert_eq!(status_a_stopped.state, ProgramState::Stopped);
    assert_eq!(status_b_stopped.state, ProgramState::Stopped);

    manager.shutdown().await.expect("shutdown manager");
}

#[tokio::test]
async fn test_manager_hot_reload_preserves_unchanged() {
    let yaml_v1 = format!(
        r#"
program_defaults:
  autostart: true
  start_secs: 0
  stop_wait_secs: 2

programs:
  keep_alive:
    command: "{cmd_keep}"
    priority: 10
  to_be_modified:
    command: "{cmd_mod_v1}"
    priority: 20
"#,
        cmd_keep = get_sleep_cmd(20),
        cmd_mod_v1 = get_sleep_cmd(10),
    );

    let config_v1 = SupervisorConfig::from_yaml_str(&yaml_v1).expect("parse v1");
    let mut manager = SupervisorManager::new(&config_v1).expect("create manager");
    let handle = manager.handle();

    handle.start_all().await.expect("start all v1");

    let keep_before = handle.get_status("keep_alive").await.expect("keep status");
    let mod_before = handle
        .get_status("to_be_modified")
        .await
        .expect("mod status");

    let pid_keep_before = keep_before.pid.expect("keep PID");
    let pid_mod_before = mod_before.pid.expect("mod PID");

    // v2 config: keep_alive unchanged; to_be_modified updated with new duration; added_service added
    let yaml_v2 = format!(
        r#"
program_defaults:
  autostart: true
  start_secs: 0
  stop_wait_secs: 2

programs:
  keep_alive:
    command: "{cmd_keep}"
    priority: 10
  to_be_modified:
    command: "{cmd_mod_v2}"
    priority: 20
  added_service:
    command: "{cmd_add}"
    priority: 30
"#,
        cmd_keep = get_sleep_cmd(20),
        cmd_mod_v2 = get_sleep_cmd(15),
        cmd_add = get_sleep_cmd(10),
    );

    let config_v2 = SupervisorConfig::from_yaml_str(&yaml_v2).expect("parse v2");
    let summary = handle.reload_config(config_v2).await.expect("reload v2");

    assert_eq!(summary.unchanged, vec!["keep_alive".to_string()]);
    assert_eq!(summary.modified, vec!["to_be_modified".to_string()]);
    assert_eq!(summary.added, vec!["added_service".to_string()]);
    assert!(summary.removed.is_empty());

    // Critical assertion: Unchanged keep_alive process remains online with identical PID (zero-downtime)
    let keep_after = handle
        .get_status("keep_alive")
        .await
        .expect("keep status after");
    assert_eq!(keep_after.state, ProgramState::Running);
    assert_eq!(
        keep_after.pid,
        Some(pid_keep_before),
        "Unchanged program PID must not change"
    );

    // Modified to_be_modified was restarted with a new PID
    let mod_after = handle
        .get_status("to_be_modified")
        .await
        .expect("mod status after");
    assert_eq!(mod_after.state, ProgramState::Running);
    assert_ne!(
        mod_after.pid,
        Some(pid_mod_before),
        "Modified program must have new PID"
    );

    // Newly added service is started
    let add_status = handle
        .get_status("added_service")
        .await
        .expect("add status");
    assert_eq!(add_status.state, ProgramState::Running);

    manager.shutdown().await.expect("shutdown manager");
}

#[test]
fn test_config_example_yaml_parsing() {
    let example_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config-example.yaml");
    assert!(example_path.exists(), "config-example.yaml must exist");

    let config =
        SupervisorConfig::from_file(&example_path).expect("config-example.yaml must be valid");
    assert_eq!(config.programs.len(), 6);
    assert!(config.programs.contains_key("redis"));
    assert!(config.programs.contains_key("api-server"));
    assert!(config.programs.contains_key("worker-task"));
    assert!(config.programs.contains_key("web-frontend"));
    assert!(config.programs.contains_key("nightly-backup"));
    assert!(config.programs.contains_key("microservice"));

    let resolved = config
        .resolve_programs()
        .expect("resolve programs in config-example.yaml");
    assert_eq!(resolved.len(), 6);

    let redis = &resolved["redis"];
    assert_eq!(redis.priority, 10);
    assert!(redis.health_check.is_some());

    let api = &resolved["api-server"];
    assert_eq!(api.priority, 20);
    assert_eq!(api.depends_on, vec!["redis"]);
    assert!(api.health_check.is_some());

    let backup = &resolved["nightly-backup"];
    assert_eq!(backup.cron.as_deref(), Some("0 2 * * *"));
    assert_eq!(backup.cron_stop.as_deref(), Some("0 4 * * *"));
    assert!(backup.pre_start.is_some());
    assert!(backup.pre_stop.is_some());

    let micro = &resolved["microservice"];
    assert!(micro.restart_when_binary_changed);
    assert_eq!(
        micro.restart_directory_monitor.as_deref(),
        Some(
            example_path
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join("./config")
                .as_ref()
        )
    );
    assert_eq!(micro.restart_file_pattern.as_deref(), Some("*.json"));
    assert_eq!(
        micro.restart_signal_when_file_changed,
        Some(StopSignal::Hup)
    );
    assert_eq!(micro.restart_debounce_secs, 5);
}

#[tokio::test]
async fn test_activity_tracker_and_idle_timeout() {
    use rsupervisord::manager::ActivityTracker;
    let tracker = ActivityTracker::new(1, true); // 1s timeout
    assert!(tracker.is_metrics_active());

    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    assert!(
        !tracker.is_metrics_active(),
        "Should be inactive after idle timeout"
    );

    tracker.record_activity();
    assert!(
        tracker.is_metrics_active(),
        "Should be active again after recording activity"
    );

    // Test idle_timeout_secs: 0 keeps active indefinitely
    let continuous_tracker = ActivityTracker::new(0, true);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(continuous_tracker.is_metrics_active());
}

#[tokio::test]
async fn test_program_with_logs_disabled() {
    let yaml = format!(
        r#"
programs:
  silent_prog:
    command: "{cmd}"
    autostart: true
    start_secs: 0
    logs:
      enabled: false
"#,
        cmd = get_sleep_cmd(5),
    );

    let config = SupervisorConfig::from_yaml_str(&yaml).expect("parse yaml");
    let mut manager = SupervisorManager::new(&config).expect("create manager");
    let handle = manager.handle();

    handle.start_all().await.expect("start silent program");
    let status = handle.get_status("silent_prog").await.expect("get status");
    assert_eq!(status.state, ProgramState::Running);
    assert!(status.pid.is_some());

    // Verify logs buffer is empty because logs were disabled
    let logs = handle
        .read_logs("silent_prog", None)
        .await
        .expect("read logs");
    assert!(
        logs.is_empty(),
        "Logs should be completely empty when disabled"
    );

    handle.stop_all(None).await.expect("stop all");
    manager.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn test_manager_shutdown_cancels_backoff_programs() {
    let yaml = format!(
        r#"
programs:
  failing_prog:
    command: "{cmd}"
    autostart: true
    start_secs: 3
    start_retries: 5
"#,
        cmd = get_exit_cmd(1),
    );

    let config = SupervisorConfig::from_yaml_str(&yaml).expect("parse yaml");
    let mut manager = SupervisorManager::new(&config).expect("create manager");
    let handle = manager.handle();

    handle.start_all().await.expect("start all");

    // Wait until program enters Backoff
    let mut in_backoff = false;
    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        if let Ok(st) = handle.get_status("failing_prog").await
            && st.state == ProgramState::Backoff
        {
            in_backoff = true;
            break;
        }
    }
    assert!(in_backoff, "Program should enter Backoff");

    // Manager shutdown must cleanly stop programs in Backoff and not hang
    let start = std::time::Instant::now();
    manager.shutdown().await.expect("manager shutdown");
    assert!(
        start.elapsed() < std::time::Duration::from_secs(3),
        "Shutdown took too long: {:?}",
        start.elapsed()
    );
}

#[tokio::test]
async fn test_manager_start_and_stop_group() {
    let yaml = format!(
        r#"
program_defaults:
  autostart: false
  start_secs: 0
  stop_wait_secs: 2

groups:
  web_group:
    programs:
      - srv_1
      - srv_2

programs:
  srv_1:
    command: "{cmd_1}"
    priority: 10
  srv_2:
    command: "{cmd_2}"
    priority: 20
  other_srv:
    command: "{cmd_3}"
    priority: 50
"#,
        cmd_1 = get_sleep_cmd(10),
        cmd_2 = get_sleep_cmd(10),
        cmd_3 = get_sleep_cmd(10),
    );

    let config = SupervisorConfig::from_yaml_str(&yaml).expect("parse yaml");
    let mut manager = SupervisorManager::new(&config).expect("create manager");
    let handle = manager.handle();

    // Start only web_group
    let started = handle.start_group("web_group").await.expect("start group");
    assert_eq!(started.len(), 2);
    assert!(started.contains(&"srv_1".to_string()));
    assert!(started.contains(&"srv_2".to_string()));

    // Verify srv_1 and srv_2 are running, while other_srv is still stopped
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let st1 = handle.get_status("srv_1").await.unwrap();
    let st2 = handle.get_status("srv_2").await.unwrap();
    let st3 = handle.get_status("other_srv").await.unwrap();

    assert_eq!(st1.state, ProgramState::Running);
    assert_eq!(st1.group, "web_group");
    assert_eq!(st1.full_name(), "web_group:srv_1");

    assert_eq!(st2.state, ProgramState::Running);
    assert_eq!(st2.group, "web_group");
    assert_eq!(st2.full_name(), "web_group:srv_2");

    assert_eq!(st3.state, ProgramState::Stopped);
    assert_eq!(st3.group, "other_srv");
    assert_eq!(st3.full_name(), "other_srv");

    // Also test finding match via full name
    let st_by_full = handle.get_status("web_group:srv_1").await.unwrap();
    assert_eq!(st_by_full.name, "srv_1");

    // Stop web_group
    let stopped = handle
        .stop_group("web_group", None)
        .await
        .expect("stop group");
    assert_eq!(stopped.len(), 2);

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let st1_after = handle.get_status("srv_1").await.unwrap();
    assert_eq!(st1_after.state, ProgramState::Stopped);

    manager.shutdown().await.expect("manager shutdown");
}

#[tokio::test]
async fn test_manager_send_stdin() {
    let yaml = format!(
        r#"
programs:
  worker:
    command: "{cmd}"
    autostart: true
    start_secs: 0
    stop_wait_secs: 2
"#,
        cmd = get_sleep_cmd(10),
    );

    let config = SupervisorConfig::from_yaml_str(&yaml).expect("parse yaml");
    let mut manager = SupervisorManager::new(&config).expect("create manager");
    let handle = manager.handle();

    handle.start_program("worker").await.expect("start worker");
    let st = handle.get_status("worker").await.unwrap();
    assert_eq!(st.state, ProgramState::Running);

    // Send stdin via manager handle
    let res = handle
        .send_stdin("worker", b"hello via manager\n".to_vec())
        .await;
    assert!(
        res.is_ok(),
        "Sending stdin via manager handle should succeed: {:?}",
        res
    );

    // Send stdin to non-existent program returns NotFound
    let err_res = handle.send_stdin("unknown_program", b"data".to_vec()).await;
    assert!(matches!(
        err_res,
        Err(rsupervisord::error::ProgramError::NotFound { .. })
    ));

    manager.shutdown().await.expect("manager shutdown");
}
