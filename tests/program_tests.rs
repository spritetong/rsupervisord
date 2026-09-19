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
