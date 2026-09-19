use rsupervisord::logging::{LogRotator, RingBuffer, parse_byte_size};
use rsupervisord::program::ProcessProgram;
use rsupervisord::program::config::{AutoRestartPolicy, ProgramConfig, ProgramLogsConfig};
use rsupervisord::program::traits::Program;
use std::time::Duration;
use tempfile::tempdir;

#[test]
fn test_ring_buffer_operations() {
    let buf = RingBuffer::new(5);
    for i in 1..=10 {
        buf.push(format!("msg {}", i));
    }
    assert_eq!(buf.len(), 5);
    let lines = buf.get_lines(None);
    assert_eq!(lines, vec!["msg 6", "msg 7", "msg 8", "msg 9", "msg 10"]);

    let partial = buf.get_lines(Some(2));
    assert_eq!(partial, vec!["msg 9", "msg 10"]);
}

#[tokio::test]
async fn test_ring_buffer_streaming() {
    let buf = RingBuffer::new(10);
    let mut rx = buf.subscribe();

    buf.push("stream message 1");
    buf.push("stream message 2");

    assert_eq!(rx.recv().await.unwrap(), "stream message 1");
    assert_eq!(rx.recv().await.unwrap(), "stream message 2");
}

#[test]
fn test_byte_size_parsing() {
    assert_eq!(parse_byte_size("500B").unwrap(), 500);
    assert_eq!(parse_byte_size("10KB").unwrap(), 10240);
    assert_eq!(parse_byte_size("5MB").unwrap(), 5 * 1024 * 1024);
    assert_eq!(parse_byte_size("2GB").unwrap(), 2 * 1024 * 1024 * 1024);
}

#[test]
fn test_log_rotator_backups() {
    let dir = tempdir().unwrap();
    let log_path = dir.path().join("service.log");

    // Max 20 bytes per file, keep 2 backups
    let rotator = LogRotator::new(&log_path, 20, 2).unwrap();

    // Write enough to trigger multiple rotations
    rotator.write_line("AAAAA AAAAA AAAAA").unwrap(); // ~18 bytes
    rotator.write_line("BBBBB BBBBB BBBBB").unwrap(); // triggers rotation to service.log.1
    rotator.write_line("CCCCC CCCCC CCCCC").unwrap(); // triggers rotation to service.log.2

    assert!(log_path.exists());
    assert!(dir.path().join("service.log.1").exists());
}

#[tokio::test]
async fn test_process_stdout_capture_into_ring_buffer() {
    #[cfg(windows)]
    let (cmd, args) = (
        "powershell.exe",
        vec![
            "-NoProfile".to_string(),
            "-Command".to_string(),
            "Write-Output 'alpha'; Write-Output 'beta'; Write-Output 'gamma'".to_string(),
        ],
    );
    #[cfg(not(windows))]
    let (cmd, args) = (
        "sh",
        vec![
            "-c".to_string(),
            "echo alpha; echo beta; echo gamma".to_string(),
        ],
    );

    let mut config = ProgramConfig::new("echo_test", cmd);
    config.args = args;
    config.autorestart = AutoRestartPolicy::Never;
    config.start_secs = 0;

    let mut program = ProcessProgram::new(config).unwrap();
    program.start().await.unwrap();

    // Give process a brief moment to finish outputting
    tokio::time::sleep(Duration::from_millis(500)).await;
    let _ = program.stop(Duration::from_secs(1)).await;

    let logs = program.read_logs(None);
    assert!(
        logs.iter().any(|l| l.contains("alpha")),
        "Expected 'alpha' in logs: {:?}",
        logs
    );
    assert!(
        logs.iter().any(|l| l.contains("beta")),
        "Expected 'beta' in logs: {:?}",
        logs
    );
    assert!(
        logs.iter().any(|l| l.contains("gamma")),
        "Expected 'gamma' in logs: {:?}",
        logs
    );
}

#[tokio::test]
async fn test_process_stdout_file_logging_and_rotation() {
    let dir = tempdir().unwrap();
    let stdout_log = dir.path().join("app_stdout.log");

    #[cfg(windows)]
    let (cmd, args) = (
        "powershell.exe",
        vec![
            "-NoProfile".to_string(),
            "-Command".to_string(),
            "1..10 | ForEach-Object { Write-Output ('line ' + $_) }".to_string(),
        ],
    );
    #[cfg(not(windows))]
    let (cmd, args) = (
        "sh",
        vec![
            "-c".to_string(),
            "for i in $(seq 1 10); do echo \"line $i\"; done".to_string(),
        ],
    );

    let mut config = ProgramConfig::new("rotate_test", cmd);
    config.args = args;
    config.autorestart = AutoRestartPolicy::Never;
    config.start_secs = 0;
    config.logs = ProgramLogsConfig {
        stdout: Some(stdout_log.clone()),
        stderr: None,
        max_bytes: Some("25B".to_string()), // Low threshold to force rotation
        backups: Some(3),
        redirect_stderr: true,
    };

    let mut program = ProcessProgram::new(config).unwrap();
    program.start().await.unwrap();

    tokio::time::sleep(Duration::from_millis(800)).await;
    let _ = program.stop(Duration::from_secs(1)).await;

    // Check that primary log file and rotated backups exist
    assert!(stdout_log.exists(), "Primary stdout log should exist");
    let backup1 = dir.path().join("app_stdout.log.1");
    assert!(
        backup1.exists(),
        "Rotated log app_stdout.log.1 should exist"
    );
}

#[tokio::test]
async fn test_process_live_log_subscription() {
    #[cfg(windows)]
    let (cmd, args) = (
        "powershell.exe",
        vec![
            "-NoProfile".to_string(),
            "-Command".to_string(),
            "Write-Output 'live-msg'".to_string(),
        ],
    );
    #[cfg(not(windows))]
    let (cmd, args) = ("sh", vec!["-c".to_string(), "echo live-msg".to_string()]);

    let mut config = ProgramConfig::new("live_sub_test", cmd);
    config.args = args;
    config.autorestart = AutoRestartPolicy::Never;
    config.start_secs = 0;

    let mut program = ProcessProgram::new(config).unwrap();
    let mut rx = program.subscribe_logs();

    program.start().await.unwrap();

    let mut found = false;
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        if let Ok(msg) = rx.try_recv()
            && msg.contains("live-msg")
        {
            found = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let _ = program.stop(Duration::from_secs(1)).await;
    assert!(
        found,
        "Expected to receive 'live-msg' from live subscription"
    );
}
