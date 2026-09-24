// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::error::ProgramError;
use crate::logging::backend::LogBackend;
use crate::logging::types::LogChunk;
use async_trait::async_trait;
use file_rotate::suffix::{AppendCount, AppendTimestamp, DateFrom, FileLimit};
use file_rotate::{ContentLimit, FileRotate, compression::Compression};
use parking_lot::Mutex;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

enum RotatorInner {
    Count(FileRotate<AppendCount>),
    Timestamp(FileRotate<AppendTimestamp>),
    Append(File),
}

impl Write for RotatorInner {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Count(w) => w.write(buf),
            Self::Timestamp(w) => w.write(buf),
            Self::Append(w) => w.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Count(w) => w.flush(),
            Self::Timestamp(w) => w.flush(),
            Self::Append(w) => w.flush(),
        }
    }
}

/// Thread-safe file rotator wrapping `file_rotate::FileRotate` or append-only `File`.
#[derive(Clone)]
pub struct LogRotator {
    path: PathBuf,
    inner: Arc<Mutex<RotatorInner>>,
}

impl LogRotator {
    /// Creates a new LogRotator at the given path with max_bytes and retained backups count
    /// using classic numeric suffix naming (`.1`, `.2`, ...).
    pub fn new(
        path: impl AsRef<Path>,
        max_bytes: usize,
        backups: usize,
    ) -> Result<Self, ProgramError> {
        Self::with_options(path, max_bytes, backups, false)
    }

    /// Creates a LogRotator with explicit timestamp_suffix selection.
    ///
    /// When `max_bytes == 0`, file rotation is disabled and output is appended continuously.
    /// When `timestamp_suffix == true`, rotated files use `app.log.YYYY-MM-DDTHH-MM-SS`.
    /// When `timestamp_suffix == false`, rotated files use classic `app.log.1`, `app.log.2`.
    pub fn with_options(
        path: impl AsRef<Path>,
        max_bytes: usize,
        backups: usize,
        timestamp_suffix: bool,
    ) -> Result<Self, ProgramError> {
        let path_buf = path.as_ref().to_path_buf();
        if let Some(parent) = path_buf.parent()
            && !parent.as_os_str().is_empty()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent).map_err(|e| {
                ProgramError::ConfigError(format!(
                    "Failed to create log directory '{}': {}",
                    parent.display(),
                    e
                ))
            })?;
        }

        // Probe target path to ensure write permissions upfront
        let initial_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path_buf)
            .map_err(|e| {
                ProgramError::ConfigError(format!(
                    "Failed to open/create log file '{}': {}",
                    path_buf.display(),
                    e
                ))
            })?;

        let inner = if max_bytes == 0 {
            // max_bytes == 0 means never rotate (Python and Go compatibility contract)
            RotatorInner::Append(initial_file)
        } else if timestamp_suffix {
            let scheme = AppendTimestamp::with_format(
                "%Y-%m-%dT%H-%M-%S",
                FileLimit::MaxFiles(backups),
                DateFrom::Now,
            );
            let rotator = FileRotate::new(
                path_buf.clone(),
                scheme,
                ContentLimit::Bytes(max_bytes),
                Compression::None,
                None,
            );
            RotatorInner::Timestamp(rotator)
        } else {
            let rotator = FileRotate::new(
                path_buf.clone(),
                AppendCount::new(backups),
                ContentLimit::Bytes(max_bytes),
                Compression::None,
                None,
            );
            RotatorInner::Count(rotator)
        };

        Ok(Self {
            path: path_buf,
            inner: Arc::new(Mutex::new(inner)),
        })
    }

    /// Writes a line of text to the rotating log file.
    pub fn write_line(&self, line: &str) -> std::io::Result<()> {
        let mut guard = self.inner.lock();
        writeln!(guard, "{}", line)
    }

    /// Writes a raw byte buffer to the rotating log file.
    pub fn write_all(&self, data: &[u8]) -> std::io::Result<()> {
        let mut guard = self.inner.lock();
        guard.write_all(data)
    }

    /// Flushes any buffered content to disk.
    pub fn flush(&self) -> std::io::Result<()> {
        let mut guard = self.inner.lock();
        guard.flush()
    }

    /// Returns the primary log file path.
    #[inline]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the current size of the active log file on disk.
    pub fn file_size(&self) -> std::io::Result<u64> {
        std::fs::metadata(&self.path).map(|m| m.len())
    }
}

#[async_trait]
impl LogBackend for LogRotator {
    async fn write_chunk(&self, chunk: &LogChunk) -> Result<(), ProgramError> {
        self.write_all(&chunk.data).map_err(|e| {
            ProgramError::PlatformError(format!(
                "Failed to write chunk to log file '{}': {}",
                self.path.display(),
                e
            ))
        })
    }

    async fn flush(&self) -> Result<(), ProgramError> {
        self.flush().map_err(|e| {
            ProgramError::PlatformError(format!(
                "Failed to flush log file '{}': {}",
                self.path.display(),
                e
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logging::types::LogChannel;
    use tempfile::tempdir;

    #[test]
    fn test_log_rotator_numeric_rotation() {
        let dir = tempdir().unwrap();
        let log_file = dir.path().join("test.log");

        // Rotate every 15 bytes, keep 2 backups, numeric suffix mode
        let rotator = LogRotator::with_options(&log_file, 15, 2, false).unwrap();

        rotator.write_line("Line 1: 12345").unwrap(); // 14 bytes with newline
        rotator.write_line("Line 2: 67890").unwrap(); // triggers rotation

        assert!(log_file.exists());
        let backup1 = dir.path().join("test.log.1");
        assert!(
            backup1.exists(),
            "Backup test.log.1 should exist in numeric mode"
        );
        assert!(rotator.file_size().is_ok());
    }

    #[test]
    fn test_log_rotator_timestamp_rotation() {
        let dir = tempdir().unwrap();
        let log_file = dir.path().join("timestamp_test.log");

        // Rotate every 15 bytes, keep 2 backups, timestamp suffix mode
        let rotator = LogRotator::with_options(&log_file, 15, 2, true).unwrap();

        rotator.write_line("Line 1: 12345").unwrap();
        rotator.write_line("Line 2: 67890").unwrap(); // triggers rotation

        assert!(log_file.exists());
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();

        assert!(
            entries
                .iter()
                .any(|name| name.starts_with("timestamp_test.log.20")),
            "Expected timestamp rotated file in {:?}: {:?}",
            dir.path(),
            entries
        );
    }

    #[test]
    fn test_log_rotator_max_bytes_zero_never_rotates() {
        let dir = tempdir().unwrap();
        let log_file = dir.path().join("never_rotate.log");

        // max_bytes = 0 -> never rotate
        let rotator = LogRotator::with_options(&log_file, 0, 5, false).unwrap();

        for i in 0..10 {
            rotator
                .write_line(&format!("Line {}: some log output", i))
                .unwrap();
        }

        assert!(log_file.exists());
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "Only primary log file should exist when max_bytes=0"
        );
    }

    #[tokio::test]
    async fn test_log_rotator_as_log_backend() {
        let dir = tempdir().unwrap();
        let log_file = dir.path().join("backend_test.log");
        let rotator = LogRotator::new(&log_file, 1000, 2).unwrap();

        let chunk = LogChunk::new(LogChannel::Stdout, "worker", Some(1234), "hello chunk\n");
        rotator.write_chunk(&chunk).await.unwrap();
        <LogRotator as LogBackend>::flush(&rotator).await.unwrap();

        let content = std::fs::read_to_string(&log_file).unwrap();
        assert_eq!(content, "hello chunk\n");
    }
}
