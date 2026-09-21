// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use rsupervisord::program::{
    AutoRestartPolicy, ProcessProgram, Program, ProgramConfig, ProgramState,
};
use std::time::Duration;

fn get_sleep_command(secs: u64) -> (String, Vec<String>) {
    #[cfg(unix)]
    {
        ("sleep".to_string(), vec![secs.to_string()])
    }
    #[cfg(windows)]
    {
        (
            "powershell.exe".to_string(),
            vec![
                "-NoProfile".to_string(),
                "-Command".to_string(),
                format!("Start-Sleep -Seconds {}", secs),
            ],
        )
    }
}

fn get_exit_command(code: i32) -> (String, Vec<String>) {
    #[cfg(unix)]
    {
        (
            "sh".to_string(),
            vec!["-c".to_string(), format!("exit {}", code)],
        )
    }
    #[cfg(windows)]
    {
        (
            "cmd.exe".to_string(),
            vec!["/C".to_string(), format!("exit {}", code)],
        )
    }
}

fn get_stdin_echo_command() -> (String, Vec<String>) {
    #[cfg(unix)]
    {
        (
            "sh".to_string(),
            vec![
                "-c".to_string(),
                "read -r line; echo \"ECHO:$line\"".to_string(),
            ],
        )
    }
    #[cfg(windows)]
    {
        (
            "powershell.exe".to_string(),
            vec![
                "-NoProfile".to_string(),
                "-Command".to_string(),
                "$line = [Console]::In.ReadLine(); Write-Output ('ECHO:' + $line)".to_string(),
            ],
        )
    }
}

#[tokio::test]
async fn test_program_lifecycle_start_and_stop() {
    let (cmd, args) = get_sleep_command(10);
    let mut config = ProgramConfig::new("test_sleep", cmd);
    config.args = args;
    config.start_secs = 1;
    config.stop_wait_secs = 3;

    let mut program = ProcessProgram::new(config).expect("Failed to create ProcessProgram");
    assert_eq!(program.status().state, ProgramState::Stopped);
    assert_eq!(program.status().pid, None);

    // Start program
    program.start().await.expect("Failed to start program");
    assert_eq!(program.status().state, ProgramState::Starting);
    assert!(program.status().pid.is_some());

    // Wait until process survives start_secs (1s) -> transitions to Running
    program
        .wait_for_state(ProgramState::Running, Duration::from_secs(3))
        .await
        .expect("Program failed to transition to Running state");
    assert_eq!(program.status().state, ProgramState::Running);

    // Graceful stop
    program
        .stop(Duration::from_secs(3))
        .await
        .expect("Failed to stop program");
    assert_eq!(program.status().state, ProgramState::Stopped);
    assert_eq!(program.status().pid, None);

    // Clean shutdown of internal Actor
    program
        .shutdown()
        .await
        .expect("Failed to shutdown program");
}

#[tokio::test]
async fn test_program_restart() {
    let (cmd, args) = get_sleep_command(10);
    let mut config = ProgramConfig::new("test_restart", cmd);
    config.args = args;
    config.start_secs = 0; // Transition to Running immediately

    let mut program = ProcessProgram::new(config).expect("Failed to create ProcessProgram");
    program.start().await.expect("Failed to start program");
    let pid1 = program.status().pid.expect("PID should exist");

    // Restart
    program
        .restart(Duration::from_secs(2))
        .await
        .expect("Failed to restart program");
    let pid2 = program
        .status()
        .pid
        .expect("PID should exist after restart");

    assert_ne!(pid1, pid2, "New process should have different PID");

    program
        .stop(Duration::from_secs(2))
        .await
        .expect("Failed to stop program");
    program
        .shutdown()
        .await
        .expect("Failed to shutdown program");
}

#[tokio::test]
async fn test_program_restart_with_start_secs() {
    let (cmd, args) = get_sleep_command(10);
    let mut config = ProgramConfig::new("test_restart_start_secs", cmd);
    config.args = args;
    config.start_secs = 1;

    let mut program = ProcessProgram::new(config).expect("Failed to create ProcessProgram");
    program.start().await.expect("Failed to start program");
    program
        .wait_for_state(ProgramState::Running, Duration::from_secs(3))
        .await
        .expect("Program should transition to Running initial time");

    let pid1 = program.status().pid.expect("PID should exist");

    // Restart while running
    program
        .restart(Duration::from_secs(2))
        .await
        .expect("Failed to restart program");

    assert_eq!(program.status().state, ProgramState::Starting);
    let pid2 = program
        .status()
        .pid
        .expect("PID should exist after restart");
    assert_ne!(pid1, pid2, "New process should have different PID");

    // Wait for start_secs (1s) -> must transition to Running
    program
        .wait_for_state(ProgramState::Running, Duration::from_secs(3))
        .await
        .expect("Program should transition to Running state after restart with start_secs > 0");
    assert_eq!(program.status().state, ProgramState::Running);

    program
        .stop(Duration::from_secs(2))
        .await
        .expect("Failed to stop program");
    program
        .shutdown()
        .await
        .expect("Failed to shutdown program");
}

#[tokio::test]
async fn test_program_normal_exit_detection() {
    let (cmd, args) = get_exit_command(0);
    let mut config = ProgramConfig::new("test_normal_exit", cmd);
    config.args = args;
    config.start_secs = 0;
    config.autorestart = AutoRestartPolicy::Unexpected; // Do not restart on expected exit
    config.exit_codes = vec![0];

    let mut program = ProcessProgram::new(config).expect("Failed to create ProcessProgram");
    program.start().await.expect("Failed to start program");

    // Wait for exit and transition to Exited
    program
        .wait_for_state(ProgramState::Exited, Duration::from_secs(5))
        .await
        .expect("Program should transition to Exited state on exit 0");

    assert_eq!(program.status().state, ProgramState::Exited);
    assert_eq!(program.status().exit_code, Some(0));

    program
        .shutdown()
        .await
        .expect("Failed to shutdown program");
}

#[tokio::test]
async fn test_program_trait_object_safety() {
    let (cmd, args) = get_sleep_command(5);
    let mut config = ProgramConfig::new("test_dyn", cmd);
    config.args = args;
    config.start_secs = 0;

    let program: Box<dyn Program> = Box::new(ProcessProgram::new(config).expect("create"));
    assert_eq!(program.name(), "test_dyn");
    assert_eq!(program.priority(), 50);
    assert_eq!(program.status().state, ProgramState::Stopped);
}

#[cfg(unix)]
#[tokio::test]
async fn test_unix_process_group_cleanup() {
    // Spawn a shell script that derives grandchild processes
    let script = "sleep 10 & wait";
    let mut config = ProgramConfig::new("test_pgid_tree", "sh");
    config.args = vec!["-c".to_string(), script.to_string()];
    config.start_secs = 0;
    config.stop_wait_secs = 2;

    let mut program = ProcessProgram::new(config).expect("create");
    program.start().await.expect("start");

    let pid = program.status().pid.expect("PID should exist");

    // Allow grandchild process to spawn
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Gracefully stop
    program.stop(Duration::from_secs(2)).await.expect("stop");
    assert_eq!(program.status().state, ProgramState::Stopped);

    // Verify all processes in the process group are terminated (kill -0 should fail with ESRCH)
    let pgid = nix::unistd::Pid::from_raw(-(pid as i32));
    let probe = nix::sys::signal::kill(pgid, None);
    assert!(probe.is_err(), "Process group should no longer exist");

    program.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn test_grandchild_pipe_retention_bounded_drain() {
    #[cfg(unix)]
    let (cmd, args) = (
        "sh".to_string(),
        vec!["-c".to_string(), "(sleep 30 >&1) & exit 0".to_string()],
    );
    #[cfg(windows)]
    let (cmd, args) = (
        "cmd.exe".to_string(),
        vec![
            "/C".to_string(),
            "start /b ping -n 30 127.0.0.1 >nul & exit 0".to_string(),
        ],
    );

    let mut config = ProgramConfig::new("grandchild_pipe", cmd);
    config.args = args;
    config.start_secs = 0;
    config.autorestart = AutoRestartPolicy::Never;

    let mut program = ProcessProgram::new(config).expect("create");
    program.start().await.expect("start");

    tokio::time::sleep(Duration::from_millis(300)).await;

    let start_stop = std::time::Instant::now();
    let _ = program.stop(Duration::from_secs(1)).await;
    assert!(
        start_stop.elapsed() < Duration::from_secs(5),
        "Stop took too long: {:?}",
        start_stop.elapsed()
    );

    program.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn test_spawn_failure_error() {
    let config = ProgramConfig::new("bad_cmd", "definitely_nonexistent_binary_12345");
    let mut program = ProcessProgram::new(config).expect("create");
    let res = program.start().await;
    assert!(res.is_err(), "Start should fail for nonexistent command");
    assert_eq!(program.status().state, ProgramState::Stopped);
    program.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn test_backoff_stop_cancellation() {
    let (cmd, args) = get_exit_command(1);
    let mut config = ProgramConfig::new("backoff_cancel", cmd);
    config.args = args;
    config.start_secs = 3;
    config.start_retries = 3;
    config.autorestart = AutoRestartPolicy::Never;

    let mut program = ProcessProgram::new(config).expect("create");
    program.start().await.expect("start");

    // Wait for crash and transition to Backoff
    program
        .wait_for_state(ProgramState::Backoff, Duration::from_secs(3))
        .await
        .expect("Program should enter Backoff");
    assert_eq!(program.status().state, ProgramState::Backoff);

    // Stop while in Backoff
    let start_stop = std::time::Instant::now();
    program
        .stop(Duration::from_secs(1))
        .await
        .expect("Stop should succeed in Backoff");
    assert_eq!(program.status().state, ProgramState::Stopped);
    assert!(
        start_stop.elapsed() < Duration::from_millis(500),
        "Stop during backoff took too long: {:?}",
        start_stop.elapsed()
    );

    program.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn test_autorestart_never_startup_retries() {
    let (cmd, args) = get_exit_command(1);
    let mut config = ProgramConfig::new("retry_never", cmd);
    config.args = args;
    config.start_secs = 2;
    config.start_retries = 1;
    config.autorestart = AutoRestartPolicy::Never;

    let mut program = ProcessProgram::new(config).expect("create");
    program.start().await.expect("start");

    // First crash -> Backoff (retry 1)
    program
        .wait_for_state(ProgramState::Backoff, Duration::from_secs(3))
        .await
        .expect("Should enter Backoff on first crash");

    // Second crash -> Fatal (exceeded start_retries)
    program
        .wait_for_state(ProgramState::Fatal, Duration::from_secs(6))
        .await
        .expect("Should transition to Fatal when retries exceeded even if autorestart is Never");

    assert_eq!(program.status().state, ProgramState::Fatal);
    program.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn test_program_no_restart_during_shutdown_or_stop() {
    let (cmd, args) = get_sleep_command(10);
    let mut config = ProgramConfig::new("test_always_no_restart", cmd);
    config.args = args;
    config.start_secs = 0;
    config.autorestart = AutoRestartPolicy::Always; // Even with Always policy!

    let mut program = ProcessProgram::new(config).expect("create");
    program.start().await.expect("start");
    assert_eq!(program.status().state, ProgramState::Running);

    // Stop program
    program.stop(Duration::from_secs(2)).await.expect("stop");
    assert_eq!(program.status().state, ProgramState::Stopped);

    // Wait a brief window; process must NOT restart
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        program.status().state,
        ProgramState::Stopped,
        "Program with AutoRestartPolicy::Always must stay Stopped after stop()"
    );

    program.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn test_program_backoff_cancelled_on_shutdown() {
    let (cmd, args) = get_exit_command(1);
    let mut config = ProgramConfig::new("backoff_shutdown", cmd);
    config.args = args;
    config.start_secs = 3;
    config.start_retries = 3;
    config.autorestart = AutoRestartPolicy::Always;

    let mut program = ProcessProgram::new(config).expect("create");
    program.start().await.expect("start");

    // Wait for crash and transition to Backoff
    program
        .wait_for_state(ProgramState::Backoff, Duration::from_secs(3))
        .await
        .expect("Program should enter Backoff");
    assert_eq!(program.status().state, ProgramState::Backoff);

    // Shutdown while in Backoff
    let start_shutdown = std::time::Instant::now();
    program.shutdown().await.expect("shutdown");
    assert!(
        start_shutdown.elapsed() < Duration::from_secs(2),
        "Shutdown during backoff took too long: {:?}",
        start_shutdown.elapsed()
    );
    assert_eq!(program.status().state, ProgramState::Stopped);
}

fn get_hook_exit_command(exit_code: i32) -> String {
    format!("exit {}", exit_code)
}

#[tokio::test]
async fn test_pre_start_hook_success_and_marker() {
    let temp_dir = tempfile::tempdir().unwrap();
    let marker = temp_dir.path().join("pre_start_ok.txt");

    let (cmd, args) = get_sleep_command(10);
    let mut config = ProgramConfig::new("pre_start_ok_test", cmd);
    config.args = args;
    config.start_secs = 0;

    let hook = format!("echo ok > \"{}\"", marker.display());
    config.pre_start = Some(hook);
    config.pre_start_ignore_failure = false;

    let mut program = ProcessProgram::new(config).expect("create");
    program.start().await.expect("start program");
    assert_eq!(program.status().state, ProgramState::Running);
    assert!(marker.exists(), "pre_start hook marker file must exist");

    program.stop(Duration::from_secs(2)).await.expect("stop");
    program.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn test_pre_start_hook_failure_blocks_start() {
    let (cmd, args) = get_sleep_command(10);
    let mut config = ProgramConfig::new("pre_start_fail_test", cmd);
    config.args = args;
    config.start_secs = 0;
    config.pre_start = Some(get_hook_exit_command(1));
    config.pre_start_ignore_failure = false;

    let mut program = ProcessProgram::new(config).expect("create");
    let res = program.start().await;
    assert!(
        res.is_err(),
        "Start must fail when pre_start hook returns non-zero"
    );
    assert_eq!(program.status().state, ProgramState::Fatal);

    program.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn test_pre_start_hook_failure_degradation_ignored() {
    let (cmd, args) = get_sleep_command(10);
    let mut config = ProgramConfig::new("pre_start_degrade_test", cmd);
    config.args = args;
    config.start_secs = 0;
    config.pre_start = Some(get_hook_exit_command(1));
    config.pre_start_ignore_failure = true; // Degrade gracefully

    let mut program = ProcessProgram::new(config).expect("create");
    let res = program.start().await;
    assert!(
        res.is_ok(),
        "Start must succeed when pre_start_ignore_failure is true"
    );
    assert_eq!(program.status().state, ProgramState::Running);

    program.stop(Duration::from_secs(2)).await.expect("stop");
    program.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn test_pre_stop_hook_executed_and_failure_degradation() {
    let temp_dir = tempfile::tempdir().unwrap();
    let marker = temp_dir.path().join("pre_stop_ok.txt");

    let (cmd, args) = get_sleep_command(10);
    let mut config = ProgramConfig::new("pre_stop_test", cmd);
    config.args = args;
    config.start_secs = 0;

    let hook = format!("echo stopped > \"{}\"", marker.display());
    config.pre_stop = Some(hook);

    let mut program = ProcessProgram::new(config).expect("create");
    program.start().await.expect("start program");
    assert_eq!(program.status().state, ProgramState::Running);

    program.stop(Duration::from_secs(2)).await.expect("stop");
    assert_eq!(program.status().state, ProgramState::Stopped);
    assert!(
        marker.exists(),
        "pre_stop marker must be written before stop completes"
    );

    // Also test failing pre_stop hook does NOT prevent process stop
    let (cmd2, args2) = get_sleep_command(10);
    let mut config2 = ProgramConfig::new("pre_stop_fail_test", cmd2);
    config2.args = args2;
    config2.start_secs = 0;
    config2.pre_stop = Some(get_hook_exit_command(42));

    let mut program2 = ProcessProgram::new(config2).expect("create");
    program2.start().await.expect("start");
    assert_eq!(program2.status().state, ProgramState::Running);

    let stop_res = program2.stop(Duration::from_secs(2)).await;
    assert!(
        stop_res.is_ok(),
        "Process must stop gracefully even if pre_stop hook fails"
    );
    assert_eq!(program2.status().state, ProgramState::Stopped);

    program.shutdown().await.expect("shutdown 1");
    program2.shutdown().await.expect("shutdown 2");
}

#[tokio::test]
async fn test_process_send_stdin_success_echo() {
    let (cmd, args) = get_stdin_echo_command();
    let mut config = ProgramConfig::new("stdin_echo_test", cmd);
    config.args = args;
    config.start_secs = 0;

    let mut program = ProcessProgram::new(config).expect("create");
    program.start().await.expect("start");
    assert_eq!(program.status().state, ProgramState::Running);

    // Send stdin
    let send_res = program.send_stdin(b"HelloSupervisor\n".to_vec()).await;
    assert!(
        send_res.is_ok(),
        "Sending stdin should succeed: {:?}",
        send_res
    );

    // Wait up to 5s for the echo line to appear in logs
    let mut found = false;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let logs = program.read_logs(Some(20));
        if logs.iter().any(|l| l.contains("ECHO:HelloSupervisor")) {
            found = true;
            break;
        }
    }
    assert!(found, "Child should have echoed stdin input to stdout");

    program.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn test_process_send_stdin_not_running() {
    let (cmd, args) = get_sleep_command(10);
    let mut config = ProgramConfig::new("stdin_stopped_test", cmd);
    config.args = args;
    config.start_secs = 0;

    let program = ProcessProgram::new(config).expect("create");
    // Program is not started yet
    let res = program.send_stdin(b"test\n".to_vec()).await;
    assert!(matches!(
        res,
        Err(rsupervisord::error::ProgramError::NotRunning { .. })
    ));
}

#[tokio::test]
async fn test_process_send_stdin_fresh_pipe_after_restart() {
    let (cmd, args) = get_sleep_command(10);
    let mut config = ProgramConfig::new("stdin_restart_test", cmd);
    config.args = args;
    config.start_secs = 0;

    let mut program = ProcessProgram::new(config).expect("create");
    program.start().await.expect("start");
    assert_eq!(program.status().state, ProgramState::Running);

    program
        .send_stdin(b"before restart\n".to_vec())
        .await
        .expect("stdin 1");

    program
        .restart(Duration::from_secs(1))
        .await
        .expect("restart");
    assert_eq!(program.status().state, ProgramState::Running);

    // New child has fresh stdin writer
    let res = program.send_stdin(b"after restart\n".to_vec()).await;
    assert!(
        res.is_ok(),
        "Should send stdin to restarted child: {:?}",
        res
    );

    program.stop(Duration::from_secs(1)).await.expect("stop");
    program.shutdown().await.expect("shutdown");
}
