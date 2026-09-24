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
        redirect_stderr: true,
        stdout_events_enabled: false,
        stderr_events_enabled: false,
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
        InstantLogReader, InMemoryLogRotator, LogBackend, LogChannel, LogChunk,
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
    let mut transport = platform.create_process_log_transport(&config).await.unwrap();

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
