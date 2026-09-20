// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use rsupervisord::config::SupervisorConfig;
use rsupervisord::manager::SupervisorManager;
use rsupervisord::program::ProgramState;

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
    assert_eq!(config.programs.len(), 4);
    assert!(config.programs.contains_key("redis"));
    assert!(config.programs.contains_key("api-server"));
    assert!(config.programs.contains_key("worker-task"));
    assert!(config.programs.contains_key("web-frontend"));

    let resolved = config
        .resolve_programs()
        .expect("resolve programs in config-example.yaml");
    assert_eq!(resolved.len(), 4);

    let redis = &resolved["redis"];
    assert_eq!(redis.priority, 10);
    assert!(redis.health_check.is_some());

    let api = &resolved["api-server"];
    assert_eq!(api.priority, 20);
    assert_eq!(api.depends_on, vec!["redis"]);
    assert!(api.health_check.is_some());
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
