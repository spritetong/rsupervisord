// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use rsupervisord::config::SupervisorConfig;
use rsupervisord::manager::{SupervisorManager, SystemEvent};
use rsupervisord::program::state::ProgramState;
use std::time::Duration;

fn get_worker_command(secs: u64) -> String {
    #[cfg(unix)]
    {
        format!("sh -c 'sleep {}'", secs)
    }
    #[cfg(windows)]
    {
        format!(
            "powershell.exe -NoProfile -Command \"Start-Sleep -Seconds {}\"",
            secs
        )
    }
}

#[tokio::test]
async fn test_cron_config_validation() {
    let valid_yaml = r#"
programs:
  job1:
    command: "echo test"
    cron: "*/5 * * * *"
    cron_stop: "0 2 * * *"
"#;
    let cfg = SupervisorConfig::from_yaml_str(valid_yaml).expect("valid cron yaml should pass");
    let resolved = cfg.resolve_programs().expect("resolve programs");
    assert_eq!(
        resolved.get("job1").unwrap().cron.as_deref(),
        Some("*/5 * * * *")
    );
    assert_eq!(
        resolved.get("job1").unwrap().cron_stop.as_deref(),
        Some("0 2 * * *")
    );
    // Since cron is set and autostart omitted, autostart should default to false
    assert!(!resolved.get("job1").unwrap().autostart);

    let invalid_yaml = r#"
programs:
  bad_job:
    command: "echo test"
    cron: "this is totally not a valid cron expression"
"#;
    let err = SupervisorConfig::from_yaml_str(invalid_yaml).unwrap_err();
    let err_str = err.to_string();
    assert!(
        err_str.contains("Invalid cron expression"),
        "Expected InvalidCronExpression error, got: {}",
        err_str
    );
}

#[tokio::test]
async fn test_cron_scheduler_triggers_start() {
    let cmd = get_worker_command(10);
    // Every second cron expression: 6-field format with second field
    let yaml = format!(
        r#"
programs:
  cron_task:
    command: |-
      {cmd}
    autostart: false
    start_secs: 0
    cron: "* * * * * *"
"#,
        cmd = cmd
    );

    let config = SupervisorConfig::from_yaml_str(&yaml).expect("parse config");
    let mut manager = SupervisorManager::new(&config).expect("create manager");
    let handle = manager.handle();

    let mut event_rx = handle.subscribe_events();

    // Verify initial status: not running, cron populated
    let initial_status = handle
        .get_status("cron_task")
        .await
        .expect("initial status");
    assert_eq!(initial_status.state, ProgramState::Stopped);
    assert_eq!(initial_status.cron.as_deref(), Some("* * * * * *"));
    assert!(initial_status.next_cron_run.is_some());

    // Wait for CronTriggered event within 3 seconds
    let timeout_fut = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(SystemEvent::CronTriggered { name, action, .. }) = event_rx.recv().await
                && name == "cron_task"
                && action == "start"
            {
                return true;
            }
        }
    });

    let triggered = timeout_fut.await.unwrap_or(false);
    assert!(
        triggered,
        "CronTriggered event should be emitted by scheduler"
    );

    // Wait briefly for process to transition to Running
    tokio::time::sleep(Duration::from_millis(300)).await;
    let status_after = handle
        .get_status("cron_task")
        .await
        .expect("status after cron");
    assert_eq!(status_after.state, ProgramState::Running);

    manager.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn test_cron_scheduler_triggers_stop() {
    let cmd = get_worker_command(10);
    let yaml = format!(
        r#"
programs:
  cron_stopper:
    command: |-
      {cmd}
    autostart: true
    start_secs: 0
    cron_stop: "* * * * * *"
"#,
        cmd = cmd
    );

    let config = SupervisorConfig::from_yaml_str(&yaml).expect("parse config");
    let mut manager = SupervisorManager::new(&config).expect("create manager");
    let handle = manager.handle();

    let mut event_rx = handle.subscribe_events();

    // Start program
    handle.start_program("cron_stopper").await.expect("start");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        handle.get_status("cron_stopper").await.unwrap().state,
        ProgramState::Running
    );

    // Wait for CronTriggered stop event within 3 seconds
    let timeout_fut = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(SystemEvent::CronTriggered { name, action, .. }) = event_rx.recv().await
                && name == "cron_stopper"
                && action == "stop"
            {
                return true;
            }
        }
    });

    let stopped_evt = timeout_fut.await.unwrap_or(false);
    assert!(stopped_evt, "CronTriggered stop event should be emitted");

    // Wait briefly for process to be stopped
    tokio::time::sleep(Duration::from_millis(400)).await;
    let status_after = handle
        .get_status("cron_stopper")
        .await
        .expect("status after cron stop");
    assert_eq!(status_after.state, ProgramState::Stopped);

    manager.shutdown().await.expect("shutdown");
}
