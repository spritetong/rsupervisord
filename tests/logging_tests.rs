// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use rsupervisord::logging::{LogRotator, RingBuffer};
use rsupervisord::program::ProcessProgram;
use rsupervisord::program::config::{AutoRestartPolicy, ProgramConfig, ProgramLogsConfig};
use rsupervisord::program::traits::Program;
use rsupervisord::serde_util::string_to_bytes;
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
    assert_eq!(string_to_bytes("500B").unwrap(), 500);
    assert_eq!(string_to_bytes("10KB").unwrap(), 10240);
    assert_eq!(string_to_bytes("5MB").unwrap(), 5 * 1024 * 1024);
    assert_eq!(string_to_bytes("2GB").unwrap(), 2 * 1024 * 1024 * 1024);
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
    config.start_secs = Duration::from_secs(0);

    let program = ProcessProgram::new(config).unwrap();
    program.start().await.unwrap();

    // Wait for process to output
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if program.read_logs(None).len() >= 3 {
            break;
        }
    }
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
async fn test_process_redirect_stderr_into_stdout_logs() {
    #[cfg(windows)]
    let (cmd, args) = (
        "powershell.exe",
        vec![
            "-NoProfile".to_string(),
            "-Command".to_string(),
            "Write-Output 'out-marker'; [Console]::Error.WriteLine('err-marker')".to_string(),
        ],
    );
    #[cfg(not(windows))]
    let (cmd, args) = (
        "sh",
        vec![
            "-c".to_string(),
            "echo out-marker; echo err-marker >&2".to_string(),
        ],
    );

    let mut config = ProgramConfig::new("redirect_stderr_test", cmd);
    config.args = args;
    config.autorestart = AutoRestartPolicy::Never;
    config.start_secs = Duration::from_secs(0);
    config.logs.redirect_stderr = true;

    let program = ProcessProgram::new(config).unwrap();
    program.start().await.unwrap();

    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let logs = program.read_logs(None);
        if logs.iter().any(|l| l.contains("out-marker"))
            && logs.iter().any(|l| l.contains("err-marker"))
        {
            break;
        }
    }
    let _ = program.stop(Duration::from_secs(1)).await;

    let logs = program.read_logs(None);
    assert!(
        logs.iter().any(|l| l.contains("out-marker")),
        "Expected 'out-marker' in logs: {:?}",
        logs
    );
    assert!(
        logs.iter().any(|l| l.contains("err-marker")),
        "Expected redirected 'err-marker' in logs: {:?}",
        logs
    );
}

#[tokio::test]
async fn test_process_stderr_only_capture() {
    #[cfg(windows)]
    let (cmd, args) = (
        "powershell.exe",
        vec![
            "-NoProfile".to_string(),
            "-Command".to_string(),
            "[Console]::Error.WriteLine('err-only-marker'); Write-Output 'stdout-should-not-appear'"
                .to_string(),
        ],
    );
    #[cfg(not(windows))]
    let (cmd, args) = (
        "sh",
        vec![
            "-c".to_string(),
            "echo err-only-marker >&2; echo stdout-should-not-appear".to_string(),
        ],
    );

    let mut config = ProgramConfig::new("stderr_only_test", cmd);
    config.args = args;
    config.autorestart = AutoRestartPolicy::Never;
    config.start_secs = Duration::from_secs(0);
    config.logs.stdout = Some(std::path::PathBuf::from("null"));
    config.logs.redirect_stderr = false;

    let program = ProcessProgram::new(config).unwrap();
    program.start().await.unwrap();

    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if program
            .read_logs(None)
            .iter()
            .any(|l| l.contains("err-only-marker"))
        {
            break;
        }
    }
    let _ = program.stop(Duration::from_secs(1)).await;

    let logs = program.read_logs(None);
    assert!(
        logs.iter().any(|l| l.contains("err-only-marker")),
        "Expected 'err-only-marker' in logs: {:?}",
        logs
    );
    assert!(
        !logs.iter().any(|l| l.contains("stdout-should-not-appear")),
        "Stdout must be discarded when logs.stdout=null: {:?}",
        logs
    );
}

#[tokio::test]
async fn test_process_logs_disabled_uses_null_stdio() {
    #[cfg(windows)]
    let (cmd, args) = (
        "powershell.exe",
        vec![
            "-NoProfile".to_string(),
            "-Command".to_string(),
            "Write-Output 'disabled-marker'".to_string(),
        ],
    );
    #[cfg(not(windows))]
    let (cmd, args) = (
        "sh",
        vec!["-c".to_string(), "echo disabled-marker".to_string()],
    );

    let mut config = ProgramConfig::new("logs_disabled_test", cmd);
    config.args = args;
    config.autorestart = AutoRestartPolicy::Never;
    config.start_secs = Duration::from_secs(0);
    config.logs.enabled = false;

    let program = ProcessProgram::new(config).unwrap();
    program.start().await.unwrap();

    // Give the child time to write output that must be discarded via Stdio::null().
    tokio::time::sleep(Duration::from_millis(500)).await;
    let _ = program.stop(Duration::from_secs(1)).await;

    let logs = program.read_logs(None);
    assert!(
        logs.is_empty(),
        "Expected no logs when logs.enabled=false: {:?}",
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
    config.start_secs = Duration::from_secs(0);
    config.logs = ProgramLogsConfig {
        enabled: true,
        stdout: Some(stdout_log.clone()),
        stderr: None,
        max_bytes: Some(25), // Low threshold to force rotation
        backups: Some(3),
        stdout_timestamp_suffix: false,
        redirect_stderr: true,
        stdout_events_enabled: false,
        stderr_events_enabled: false,
        ..Default::default()
    };

    let program = ProcessProgram::new(config).unwrap();
    program.start().await.unwrap();

    // Wait for process to output and rotate
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if dir.path().join("app_stdout.log.1").exists() {
            break;
        }
    }
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
    config.start_secs = Duration::from_secs(0);

    let program = ProcessProgram::new(config).unwrap();
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

#[tokio::test]
async fn test_in_memory_rotator_and_reader() {
    use bytes::Bytes;
    use rsupervisord::logging::{
        InMemoryLogRotator, InstantLogReader, LogBackend, LogChannel, LogChunk,
    };

    // Max 30 bytes per segment, 2 backups
    let rotator = InMemoryLogRotator::new(30, 2);

    // 1. Write chunks to stdout
    let chunk1 = LogChunk::new(
        LogChannel::Stdout,
        "test_app",
        Some(100),
        Bytes::from_static(b"line 1: 1234567890\n"),
    );
    rotator.write_chunk(&chunk1).await.unwrap();

    let chunk2 = LogChunk::new(
        LogChannel::Stdout,
        "test_app",
        Some(100),
        Bytes::from_static(b"line 2: 1234567890\n"),
    );
    rotator.write_chunk(&chunk2).await.unwrap();

    // Line count and byte size check
    assert_eq!(rotator.line_count(LogChannel::Stdout), 2);
    assert_eq!(rotator.line_count(LogChannel::Stderr), 0);

    // 2. Trigger rotation by writing 3rd and 4th chunks
    let chunk3 = LogChunk::new(
        LogChannel::Stdout,
        "test_app",
        Some(100),
        Bytes::from_static(b"line 3: 1234567890\n"),
    );
    rotator.write_chunk(&chunk3).await.unwrap();

    let chunk4 = LogChunk::new(
        LogChannel::Stdout,
        "test_app",
        Some(100),
        Bytes::from_static(b"line 4: 1234567890\n"),
    );
    rotator.write_chunk(&chunk4).await.unwrap();

    // 3. Test read_lines
    let lines = rotator.read_lines(LogChannel::Stdout, Some(2));
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1], "line 4: 1234567890");

    // 4. Test XML-RPC read_bytes
    let (data, sz, overflow) = rotator.read_bytes(LogChannel::Stdout, 0, 10);
    assert_eq!(data.len(), 10);
    assert!(sz > 10);
    assert!(!overflow);

    // 5. Test XML-RPC tail_bytes
    // Total size is ~76 bytes. Requesting 10 bytes results in overflow = true (more history exists)
    let (tail_data, tail_sz, tail_overflow) = rotator.tail_bytes(LogChannel::Stdout, 0, 10);
    assert_eq!(tail_data.len(), 10);
    assert!(tail_sz > 10);
    assert!(tail_overflow);

    // Requesting entire size results in overflow = false
    let (_, _, no_overflow) = rotator.tail_bytes(LogChannel::Stdout, 0, 200);
    assert!(!no_overflow);

    // 6. Test clear
    rotator.clear(Some(LogChannel::Stdout));
    assert_eq!(rotator.line_count(LogChannel::Stdout), 0);
    assert_eq!(rotator.byte_size(LogChannel::Stdout), 0);
}

#[tokio::test]
async fn test_platform_process_log_transport() {
    use rsupervisord::platform::{ProcessTransportConfig, native_platform};

    let config = ProcessTransportConfig {
        program_name: "test_transport".to_string(),
        capture_stdout: true,
        capture_stderr: true,
        redirect_stderr: false,
    };

    let platform = native_platform();
    let mut transport = platform
        .create_process_log_transport(&config)
        .await
        .unwrap();

    // Take stdio handles for child process
    let stdio_handles = transport.take_child_stdio().unwrap();
    assert!(stdio_handles.stdout.is_some());
    assert!(stdio_handles.stderr.is_some());

    // Subsequent take must fail
    assert!(transport.take_child_stdio().is_err());

    // Convert into async reading streams
    let streams = transport.into_streams().unwrap();
    assert!(streams.stdout.is_some());
    assert!(streams.stderr.is_some());
}

#[tokio::test]
async fn test_process_composite_logging() {
    let dir = tempdir().unwrap();
    let file1 = dir.path().join("out1.log");
    let file2 = dir.path().join("out2.log");

    #[cfg(windows)]
    let (cmd, args) = (
        "powershell.exe",
        vec![
            "-NoProfile".to_string(),
            "-Command".to_string(),
            "Write-Output 'composite-fanout-line'".to_string(),
        ],
    );
    #[cfg(not(windows))]
    let (cmd, args) = (
        "sh",
        vec!["-c".to_string(), "echo composite-fanout-line".to_string()],
    );

    let mut config = ProgramConfig::new("composite_test", cmd);
    config.args = args;
    config.autorestart = AutoRestartPolicy::Never;
    config.start_secs = Duration::from_secs(0);

    // Composite destination: file1, file2
    let dest_str = format!("{}, {}", file1.display(), file2.display());
    config.logs = ProgramLogsConfig {
        enabled: true,
        stdout: Some(std::path::PathBuf::from(dest_str)),
        ..Default::default()
    };

    let program = ProcessProgram::new(config).unwrap();
    program.start().await.unwrap();

    // Wait for output in both files
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if file1.exists() && file2.exists() {
            let c1 = std::fs::read_to_string(&file1).unwrap_or_default();
            let c2 = std::fs::read_to_string(&file2).unwrap_or_default();
            if c1.contains("composite-fanout-line") && c2.contains("composite-fanout-line") {
                break;
            }
        }
    }
    let _ = program.stop(Duration::from_secs(1)).await;

    let c1 = std::fs::read_to_string(&file1).expect("file1 should exist");
    let c2 = std::fs::read_to_string(&file2).expect("file2 should exist");
    assert!(
        c1.contains("composite-fanout-line"),
        "file1 should receive output"
    );
    assert!(
        c2.contains("composite-fanout-line"),
        "file2 should receive output"
    );
}

#[tokio::test]
async fn test_process_timestamp_suffix_rotation() {
    let dir = tempdir().unwrap();
    let stdout_log = dir.path().join("timestamp_rotate.log");

    #[cfg(windows)]
    let (cmd, args) = (
        "powershell.exe",
        vec![
            "-NoProfile".to_string(),
            "-Command".to_string(),
            "Write-Output 'First payload line exceeding limit'; Start-Sleep -Milliseconds 100; Write-Output 'Second payload line trigger rotation'".to_string(),
        ],
    );
    #[cfg(not(windows))]
    let (cmd, args) = (
        "sh",
        vec![
            "-c".to_string(),
            "echo 'First payload line exceeding limit'; sleep 0.1; echo 'Second payload line trigger rotation'".to_string(),
        ],
    );

    let mut config = ProgramConfig::new("ts_rotate_test", cmd);
    config.args = args;
    config.autorestart = AutoRestartPolicy::Never;
    config.start_secs = Duration::from_secs(0);
    config.logs = ProgramLogsConfig {
        enabled: true,
        stdout: Some(stdout_log.clone()),
        stdout_max_bytes: Some(20),
        stdout_backups: Some(3),
        stdout_timestamp_suffix: true,
        ..Default::default()
    };

    let program = ProcessProgram::new(config).unwrap();
    program.start().await.unwrap();

    let mut rotated_found = false;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Ok(entries) = std::fs::read_dir(dir.path()) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with("timestamp_rotate") && name != "timestamp_rotate.log" {
                    rotated_found = true;
                    break;
                }
            }
        }
        if rotated_found {
            break;
        }
    }
    let _ = program.stop(Duration::from_secs(1)).await;

    assert!(stdout_log.exists(), "Primary stdout log should exist");
    assert!(
        rotated_found,
        "Rotated file with timestamp suffix should exist"
    );
}

#[tokio::test]
async fn test_process_max_bytes_zero_no_rotate() {
    let dir = tempdir().unwrap();
    let stdout_log = dir.path().join("no_rotate.log");

    #[cfg(windows)]
    let (cmd, args) = (
        "powershell.exe",
        vec![
            "-NoProfile".to_string(),
            "-Command".to_string(),
            "Write-Output 'Message 1: 1234567890'; Write-Output 'Message 2: 1234567890'; Write-Output 'Message 3: 1234567890'".to_string(),
        ],
    );
    #[cfg(not(windows))]
    let (cmd, args) = (
        "sh",
        vec![
            "-c".to_string(),
            "echo 'Message 1: 1234567890'; echo 'Message 2: 1234567890'; echo 'Message 3: 1234567890'".to_string(),
        ],
    );

    let mut config = ProgramConfig::new("no_rotate_test", cmd);
    config.args = args;
    config.autorestart = AutoRestartPolicy::Never;
    config.start_secs = Duration::from_secs(0);
    config.logs = ProgramLogsConfig {
        enabled: true,
        stdout: Some(stdout_log.clone()),
        stdout_max_bytes: Some(0), // max_bytes = 0 means never rotate
        stdout_backups: Some(5),
        ..Default::default()
    };

    let program = ProcessProgram::new(config).unwrap();
    program.start().await.unwrap();

    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if stdout_log.exists() {
            let content = std::fs::read_to_string(&stdout_log).unwrap_or_default();
            if content.contains("Message 3") {
                break;
            }
        }
    }
    let _ = program.stop(Duration::from_secs(1)).await;

    assert!(stdout_log.exists(), "Log file should exist");
    // Verify no backup/rotated files were created
    let entries: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(
        entries,
        vec!["no_rotate.log"],
        "Only the primary un-rotated file should exist"
    );
}

#[cfg(windows)]
#[test]
fn test_windows_syslog_config_fails() {
    use rsupervisord::logging::{SyslogFacility, SyslogLogBackend, SyslogSeverity, SyslogTarget};

    let result = SyslogLogBackend::new(
        SyslogTarget::Local,
        SyslogFacility::Daemon,
        SyslogSeverity::Notice,
        "test_tag",
    );
    assert!(result.is_err(), "Syslog backend must fail on Windows");
    let err_str = result.err().unwrap().to_string();
    assert!(
        err_str.contains("not supported on Windows"),
        "Error message should mention Windows: {}",
        err_str
    );
}

#[test]
fn test_startup_collision_warning_resolution() {
    use rsupervisord::config::schema::SupervisorConfig;

    let yaml = r#"
programs:
  prog1:
    command: "echo 1"
    logs:
      stdout: "shared.log"
      stdout_max_bytes: 1024
  prog2:
    command: "echo 2"
    logs:
      stdout: "shared.log"
      stdout_max_bytes: 1024
"#;

    let config = SupervisorConfig::from_yaml_str(yaml).unwrap();
    let resolved = config.resolve_programs();
    assert!(
        resolved.is_ok(),
        "resolve_programs should succeed while logging warning"
    );
    let resolved_map = resolved.unwrap();
    assert_eq!(resolved_map.len(), 2);
}
