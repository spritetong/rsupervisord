// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use rsupervisord::logging::{InMemoryChannelRotator, LogChannel, LogRotator, RingBuffer};
use rsupervisord::program::ProcessProgram;
use rsupervisord::program::config::{AutoRestartPolicy, LogMode, ProgramConfig, ProgramLogsConfig};
use rsupervisord::program::state::ProgramState;
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
    config.logs.enabled = LogMode::Off;

    let program = ProcessProgram::new(config).unwrap();
    program.start().await.unwrap();

    // Wait for the short-lived process to exit after discarding output via Stdio::null().
    let _ = program
        .wait_for_state(ProgramState::Exited, Duration::from_secs(5))
        .await;
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
        enabled: LogMode::On,
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

    // Max 30 bytes per segment, 3 backups (retains all 4 test segments)
    let rotator = InMemoryLogRotator::new(30, 3);

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
        enabled: LogMode::On,
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
        enabled: LogMode::On,
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
        enabled: LogMode::On,
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
        resolved.is_err(),
        "resolve_programs should reject duplicate rotating log destinations"
    );
    let err = resolved.unwrap_err().to_string();
    assert!(err.contains("Duplicate rotating log file path"));

    // When max_bytes == 0 (append-only), sharing is permitted
    let yaml_append = r#"
programs:
  prog1:
    command: "echo 1"
    logs:
      stdout: "shared_append.log"
      stdout_max_bytes: 0
  prog2:
    command: "echo 2"
    logs:
      stdout: "shared_append.log"
      stdout_max_bytes: 0
"#;
    let config_append = SupervisorConfig::from_yaml_str(yaml_append).unwrap();
    assert!(config_append.resolve_programs().is_ok());
}

#[test]
fn test_log_mode_deserialization() {
    let raw_true: LogMode = serde_json::from_str("true").unwrap();
    assert_eq!(raw_true, LogMode::On);

    let raw_false: LogMode = serde_json::from_str("false").unwrap();
    assert_eq!(raw_false, LogMode::Off);

    let raw_on: LogMode = serde_json::from_str("\"on\"").unwrap();
    assert_eq!(raw_on, LogMode::On);

    let raw_off: LogMode = serde_json::from_str("\"off\"").unwrap();
    assert_eq!(raw_off, LogMode::Off);

    let raw_mem: LogMode = serde_json::from_str("\"in_memory_only\"").unwrap();
    assert_eq!(raw_mem, LogMode::InMemoryOnly);

    let raw_mem_alias: LogMode = serde_json::from_str("\"memory\"").unwrap();
    assert_eq!(raw_mem_alias, LogMode::InMemoryOnly);
}

#[test]
fn test_in_memory_seeding_and_rotation_archive() {
    let dir = tempdir().unwrap();
    let log_file = dir.path().join("service.log");

    // Write 5 lines into service.log
    let content = "Line 1: init system\nLine 2: loading modules\nLine 3: service online\nLine 4: ready\nLine 5: running\n";
    std::fs::write(&log_file, content).unwrap();
    let file_sz = content.len() as u64;

    let rotator = InMemoryChannelRotator::new(1024, 2);
    // Seed with large enough limit so entire file fits
    let seeded_bytes = rotator.seed_from_file(&log_file, 1024).unwrap();
    assert_eq!(seeded_bytes, file_sz);

    // Verify on-disk file was rotated to service.log.1
    let rotated_file = dir.path().join("service.log.1");
    assert!(
        !log_file.exists(),
        "Original service.log should have been renamed"
    );
    assert!(rotated_file.exists(), "Archived service.log.1 should exist");

    // Verify cumulative offset matches file size
    let (data, offset, overflow) = rotator.tail_bytes(0, 100);
    assert_eq!(offset, file_sz as i64);
    assert!(!overflow);
    assert!(data.contains("Line 5: running"));

    // Verify subsequent appends increment cumulative offset monotonically
    rotator.append_line("Line 6: new event");
    let (tail_data, new_offset, _) = rotator.tail_bytes(0, 100);
    assert!(new_offset > file_sz as i64);
    assert!(tail_data.contains("Line 6: new event"));
}

#[test]
fn test_in_memory_offset_overflow_after_eviction() {
    // 2 segments of 20 bytes (1 active + 1 backup = max 40 bytes retained)
    let rotator = InMemoryChannelRotator::new(20, 1);

    // Each line is 10 bytes: e.g. "1: AAAAAA\n"
    rotator.append_line("1: AAAAAA"); // active = 10
    rotator.append_line("2: BBBBBB"); // active = 20
    rotator.append_line("3: CCCCCC"); // 20 + 10 > 20 -> rotates active [1,2] to backup; active=[3] (10)
    rotator.append_line("4: DDDDDD"); // active = 20
    rotator.append_line("5: EEEEEE"); // 20 + 10 > 20 -> rotates [3,4] to backup, evicts [1,2]! active=[5] (10)

    // At this point, lines 1 and 2 (first 20 bytes) have been evicted.
    // Reading from offset 5 (< oldest_available_offset = 20) should indicate overflow = true
    let (_, _, overflow) = rotator.tail_bytes(5, 10);
    assert!(
        overflow,
        "Reading from evicted offset must indicate overflow"
    );

    // Reading from a recent offset within the retained window should have overflow = false
    let current_offset = rotator.tail_bytes(0, 10).1;
    let (_, _, overflow_recent) = rotator.tail_bytes(current_offset - 10, 10);
    assert!(
        !overflow_recent,
        "Reading within available buffer window must not overflow"
    );
}

#[tokio::test]
async fn test_process_in_memory_only_pure_logging() {
    let dir = tempdir().unwrap();
    let stdout_log = dir.path().join("should_not_exist.log");

    #[cfg(windows)]
    let (cmd, args) = (
        "powershell.exe",
        vec![
            "-NoProfile".to_string(),
            "-Command".to_string(),
            "Write-Output 'pure in-memory log 1'; Start-Sleep -Milliseconds 100; Write-Output 'pure in-memory log 2'".to_string(),
        ],
    );
    #[cfg(not(windows))]
    let (cmd, args) = (
        "sh",
        vec![
            "-c".to_string(),
            "echo 'pure in-memory log 1'; sleep 0.1; echo 'pure in-memory log 2'".to_string(),
        ],
    );

    let mut config = ProgramConfig::new("in_memory_prog", cmd);
    config.args = args;
    config.autorestart = AutoRestartPolicy::Never;
    config.start_secs = Duration::from_secs(0);
    config.logs = ProgramLogsConfig {
        enabled: LogMode::InMemoryOnly,
        stdout: Some(stdout_log.clone()),
        buffer_size: Some(1024 * 1024),
        ..Default::default()
    };

    let program = ProcessProgram::new(config).unwrap();
    program.start().await.unwrap();

    let mut found = false;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let (tail_data, _, _) = program.tail_log(LogChannel::Stdout, 0, 4096).unwrap();
        if tail_data.contains("pure in-memory log 2") {
            found = true;
            break;
        }
    }
    let _ = program.stop(Duration::from_secs(1)).await;

    assert!(found, "Should have received stdout logs in memory rotator");
    assert!(
        !stdout_log.exists(),
        "On-disk log file must NOT be created in in_memory_only mode"
    );
}

#[test]
fn test_log_file_reader_operations() {
    use rsupervisord::logging::LogFileReader;
    use std::io::Write;
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("test_reader.log");

    {
        let mut f = std::fs::File::create(&file_path).unwrap();
        f.write_all(b"line1:hello\nline2:world\nline3:foo\nline4:bar\n")
            .unwrap();
    }

    // 1. Positive offset reading
    let s = LogFileReader::read_bytes(&file_path, 0, 12).unwrap();
    assert_eq!(s, "line1:hello\n");

    let s2 = LogFileReader::read_bytes(&file_path, 12, 0).unwrap();
    assert_eq!(s2, "line2:world\nline3:foo\nline4:bar\n");

    // 2. Negative offset (tail reading)
    let tail_str = LogFileReader::read_bytes(&file_path, -10, 0).unwrap();
    assert_eq!(tail_str, "line4:bar\n");

    // Bad arguments: offset < 0 with length != 0
    assert!(LogFileReader::read_bytes(&file_path, -10, 5).is_err());
    // Bad arguments: length < 0
    assert!(LogFileReader::read_bytes(&file_path, 0, -5).is_err());

    // 3. Slice reader
    let slice_data = b"alpha\nbeta\ngamma\n";
    let slice_res = LogFileReader::read_bytes_from_slice(slice_data, -6, 0).unwrap();
    assert_eq!(slice_res, "gamma\n");

    // 4. Tail bytes with overflow detection
    let (t_data, new_off, overflow) = LogFileReader::tail_bytes(&file_path, 0, 10);
    assert_eq!(t_data, "line4:bar\n");
    assert!(overflow);
    assert_eq!(new_off, 44);

    // 5. Seed and archive
    let archive_target = dir.path().join("to_seed.log");
    {
        let mut f = std::fs::File::create(&archive_target).unwrap();
        f.write_all(b"pre:leading-partial-discard\nvalid:entry1\nvalid:entry2\n")
            .unwrap();
    }

    let (seeded, total_sz) = LogFileReader::seed_and_archive(&archive_target, 28, Some("1"))
        .unwrap()
        .unwrap();
    assert_eq!(total_sz, 54);
    assert_eq!(
        String::from_utf8_lossy(&seeded),
        "valid:entry1\nvalid:entry2\n"
    );

    // Original file must be renamed to .1
    assert!(!archive_target.exists());
    let archived = format!("{}.1", archive_target.display());
    assert!(std::path::Path::new(&archived).exists());
}

#[tokio::test]
async fn test_daemon_main_log_in_memory_and_maintail() {
    use rsupervisord::config::schema::{LoggingConfig, SupervisorConfig};
    use rsupervisord::manager::SupervisorManager;

    let dir = tempdir().unwrap();
    let main_log = dir.path().join("daemon_main.log");

    // Write initial log on disk to verify cold start pre-read
    std::fs::write(&main_log, "cold start: supervisor initial line\n").unwrap();

    let config = SupervisorConfig {
        logging: LoggingConfig {
            enabled: LogMode::InMemoryOnly,
            file: Some(main_log.clone()),
            buffer_size: Some(1024 * 1024),
            ..Default::default()
        },
        ..Default::default()
    };

    let manager = SupervisorManager::builder(config).build().unwrap();
    let handle = manager.handle();

    // The disk file should have been archived to .1 and seeded
    let read_val = handle.read_main_log(0, 0).await.unwrap();
    assert!(read_val.contains("cold start: supervisor initial line"));

    // Tail main log
    let (tail_val, new_off, _) = handle.tail_main_log(0, 4096).await.unwrap();
    assert!(tail_val.contains("cold start: supervisor initial line"));
    assert!(new_off > 0);

    // Clear main log
    handle.clear_main_log().await.unwrap();
    let cleared_val = handle.read_main_log(0, 0).await.unwrap();
    assert_eq!(cleared_val, "");
}

#[test]
fn test_program_logs_effective_sizing_decoupled() {
    let logs_cfg = ProgramLogsConfig {
        buffer_size: Some(2 * 1024 * 1024),       // 2MB memory buffer
        stdout_max_bytes: Some(10 * 1024 * 1024), // 10MB file limit
        stderr_max_bytes: None,
        max_bytes: Some(20 * 1024 * 1024), // 20MB fallback file limit
        ..Default::default()
    };

    // In-memory buffer size must honor buffer_size
    assert_eq!(logs_cfg.effective_buffer_size(), 2 * 1024 * 1024);

    // File max bytes must NEVER be polluted by buffer_size
    assert_eq!(logs_cfg.effective_stdout_max_bytes(), 10 * 1024 * 1024);
    assert_eq!(logs_cfg.effective_stderr_max_bytes(), 20 * 1024 * 1024);
}

#[test]
fn test_seed_archive_collision_safe() {
    let dir = tempdir().unwrap();
    let log_file = dir.path().join("preseed.log");
    let archive_file = dir.path().join("preseed.log.1");

    // Pre-create both original log file and collision target .1
    std::fs::write(&log_file, "original unrotated content\n").unwrap();
    std::fs::write(&archive_file, "stale existing backup\n").unwrap();

    let res = rsupervisord::logging::LogFileReader::seed_and_archive(&log_file, 1024, Some("1"));
    assert!(
        res.is_ok(),
        "seed_and_archive must succeed despite existing .1 file"
    );

    let (seeded, file_size) = res.unwrap().expect("seeded data");
    assert_eq!(file_size, "original unrotated content\n".len() as u64);
    assert_eq!(seeded, b"original unrotated content\n");

    // The collision file .1 should now contain the seeded log content
    let archive_content = std::fs::read_to_string(&archive_file).unwrap();
    assert_eq!(archive_content, "original unrotated content\n");
}

#[test]
fn test_in_memory_read_lines_utf8_split_across_segments() {
    let rotator = InMemoryChannelRotator::new(10, 5);

    // "你好世界\n" in UTF-8:
    // 你: [0xe4, 0xbd, 0xa0] (3 bytes)
    // 好: [0xe5, 0xa5, 0xbd] (3 bytes)
    // 世: [0xe4, 0xb8, 0x96] (3 bytes)
    // 界: [0xe7, 0x95, 0x8c] (3 bytes)
    // \n: [0x0a] (1 byte)
    // Total 13 bytes.
    // If we write 7 bytes first: 你 (3) + 好 (3) + 1st byte of 世 (0xe4)
    // Then second chunk: remaining 2 bytes of 世 + 界 (3) + \n (1) = 6 bytes.
    let full_str = "你好世界\n";
    let bytes = full_str.as_bytes();
    rotator.append_bytes(&bytes[..7]);
    rotator.append_bytes(&bytes[7..]);

    let lines = rotator.read_lines(None);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0], "你好世界");
}

#[tokio::test]
async fn test_log_rotator_clear_resets_counter_and_file() {
    use rsupervisord::logging::LogBackend;
    let dir = tempdir().unwrap();
    let log_path = dir.path().join("test_clear.log");
    let rotator = LogRotator::with_options(&log_path, 100, 2, false).unwrap();

    let chunk = rsupervisord::logging::LogChunk::new(
        LogChannel::Stdout,
        "test",
        None,
        bytes::Bytes::from_static(b"hello world before clear\n"),
    );
    rotator.write_chunk(&chunk).await.unwrap();
    rotator.flush().unwrap();

    assert!(log_path.exists());
    let size = std::fs::metadata(&log_path).unwrap().len();
    assert!(size > 0);

    // Clear via LogBackend::clear
    rotator.clear().unwrap();

    let size_after = std::fs::metadata(&log_path).unwrap().len();
    assert_eq!(size_after, 0);

    // Writing again should start from 0 bytes
    let chunk2 = rsupervisord::logging::LogChunk::new(
        LogChannel::Stdout,
        "test",
        None,
        bytes::Bytes::from_static(b"new data\n"),
    );
    rotator.write_chunk(&chunk2).await.unwrap();
    rotator.flush().unwrap();

    let content = std::fs::read_to_string(&log_path).unwrap();
    assert_eq!(content, "new data\n");
}

#[tokio::test]
async fn test_process_program_clear_logs_clears_backends() {
    let dir = tempdir().unwrap();
    let stdout_path = dir.path().join("prog_out.log");
    let config = ProgramConfig {
        name: "clear_test".to_string(),
        command: "true".to_string(),
        logs: ProgramLogsConfig {
            stdout: Some(stdout_path.clone()),
            max_bytes: Some(1024),
            ..Default::default()
        },
        ..Default::default()
    };
    let prog = ProcessProgram::new(config).unwrap();
    prog.in_memory_rotator()
        .stdout()
        .append_bytes(b"buffered data\n");

    // Pre-create disk file
    std::fs::write(&stdout_path, "disk log line\n").unwrap();

    assert_eq!(prog.in_memory_rotator().stdout().line_count(), 1);
    assert!(stdout_path.exists() && std::fs::metadata(&stdout_path).unwrap().len() > 0);

    prog.clear_logs().unwrap();

    assert_eq!(prog.in_memory_rotator().stdout().line_count(), 0);
    assert_eq!(std::fs::metadata(&stdout_path).unwrap().len(), 0);
}

#[tokio::test]
async fn test_process_program_read_log_fallback_to_in_memory() {
    let dir = tempdir().unwrap();
    let stdout_path = dir.path().join("absent_file.log");
    let config = ProgramConfig {
        name: "fallback_test".to_string(),
        command: "true".to_string(),
        logs: ProgramLogsConfig {
            stdout: Some(stdout_path.clone()),
            ..Default::default()
        },
        ..Default::default()
    };
    let prog = ProcessProgram::new(config).unwrap();
    let _ = std::fs::remove_file(&stdout_path);

    // Absent on disk and empty in memory -> fails with ReadLogFailed ("no log file")
    assert!(prog.read_log(LogChannel::Stdout, 0, 100).is_err());

    // When in-memory rotator has captured logs, it gracefully serves them
    prog.in_memory_rotator()
        .stdout()
        .append_bytes(b"captured before flush\n");
    let (data, _, _) = prog.read_log(LogChannel::Stdout, 0, 100).unwrap();
    assert_eq!(data, "captured before flush\n");

    let (tail_data, _, _) = prog.tail_log(LogChannel::Stdout, 0, 100).unwrap();
    assert_eq!(tail_data, "captured before flush\n");
}
