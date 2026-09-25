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
    Fallback(File),
}

impl Write for RotatorInner {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match self {
            Self::Count(w) => w.write(buf),
            Self::Timestamp(w) => w.write(buf),
            Self::Append(w) => w.write(buf),
            Self::Fallback(w) => w.write(buf),
        }));

        match res {
            Ok(io_res) => io_res,
            Err(_) => Err(std::io::Error::other("file_rotate panicked during write")),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match self {
            Self::Count(w) => w.flush(),
            Self::Timestamp(w) => w.flush(),
            Self::Append(w) => w.flush(),
            Self::Fallback(w) => w.flush(),
        }));

        match res {
            Ok(io_res) => io_res,
            Err(_) => Err(std::io::Error::other("file_rotate panicked during flush")),
        }
    }
}

/// Thread-safe resilient file rotator wrapping `file_rotate::FileRotate` or append-only `File`.
#[derive(Clone)]
pub struct LogRotator {
    path: PathBuf,
    max_bytes: usize,
    backups: usize,
    timestamp_suffix: bool,
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

    fn build_inner(
        path: &Path,
        max_bytes: usize,
        backups: usize,
        timestamp_suffix: bool,
    ) -> Result<RotatorInner, ProgramError> {
        let initial_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| {
                ProgramError::ConfigError(format!(
                    "Failed to open/create log file '{}': {}",
                    path.display(),
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
                path,
                scheme,
                ContentLimit::Bytes(max_bytes),
                Compression::None,
                None,
            );
            RotatorInner::Timestamp(rotator)
        } else {
            let rotator = FileRotate::new(
                path,
                AppendCount::new(backups),
                ContentLimit::Bytes(max_bytes),
                Compression::None,
                None,
            );
            RotatorInner::Count(rotator)
        };
        Ok(inner)
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

        let inner = Self::build_inner(&path_buf, max_bytes, backups, timestamp_suffix)?;

        Ok(Self {
            path: path_buf,
            max_bytes,
            backups,
            timestamp_suffix,
            inner: Arc::new(Mutex::new(inner)),
        })
    }

    /// Writes a line of text to the rotating log file.
    pub fn write_line(&self, line: &str) -> std::io::Result<()> {
        let mut buf = Vec::with_capacity(line.len() + 1);
        buf.extend_from_slice(line.as_bytes());
        buf.push(b'\n');
        self.write_all(&buf)
    }

    /// Writes a raw byte buffer to the rotating log file with automatic recovery.
    pub fn write_all(&self, data: &[u8]) -> std::io::Result<()> {
        let mut guard = self.inner.lock();
        if let Err(err) = guard.write_all(data) {
            eprintln!(
                "[rsupervisord] Warning: log write failed for '{}': {}; falling back to append-only mode",
                self.path.display(),
                err
            );
            match std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
            {
                Ok(mut file) => {
                    let _ = file.write_all(data);
                    *guard = RotatorInner::Fallback(file);
                }
                Err(fallback_err) => {
                    return Err(fallback_err);
                }
            }
        }
        Ok(())
    }

    /// Flushes any buffered content to disk.
    pub fn flush(&self) -> std::io::Result<()> {
        let mut guard = self.inner.lock();
        let _ = guard.flush();
        Ok(())
    }

    /// Truncates the log file and resets rotation state without count desync.
    pub fn clear(&self) -> std::io::Result<()> {
        let mut guard = self.inner.lock();
        let _ = std::fs::write(&self.path, "");
        *guard = match Self::build_inner(
            &self.path,
            self.max_bytes,
            self.backups,
            self.timestamp_suffix,
        ) {
            Ok(inner) => inner,
            Err(_) => {
                let file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)?;
                RotatorInner::Fallback(file)
            }
        };
        Ok(())
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

impl std::io::Write for LogRotator {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        LogRotator::flush(self)
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

    #[test]
    fn test_log_rotator_clear_resets_file_and_state() {
        let dir = tempdir().unwrap();
        let log_file = dir.path().join("clear_test.log");
        let rotator = LogRotator::new(&log_file, 1000, 2).unwrap();

        rotator.write_line("line before clear").unwrap();
        assert!(log_file.exists());
        assert!(rotator.file_size().unwrap() > 0);

        rotator.clear().unwrap();
        assert_eq!(rotator.file_size().unwrap(), 0);

        rotator.write_line("line after clear").unwrap();
        let content = std::fs::read_to_string(&log_file).unwrap();
        assert_eq!(content, "line after clear\n");
    }

    #[test]
    fn test_log_rotator_write_resilience() {
        let dir = tempdir().unwrap();
        let log_file = dir.path().join("resilient.log");
        let rotator = LogRotator::new(&log_file, 100, 2).unwrap();

        // Write normal data
        rotator.write_line("initial normal line").unwrap();
        assert!(log_file.exists());

        // Subsequent writes succeed
        let write_res = rotator.write_line("second line");
        assert!(write_res.is_ok());

        // Clear resets the file and rotator state cleanly
        rotator.clear().unwrap();
        rotator.write_line("post-clear line").unwrap();
        let content = std::fs::read_to_string(&log_file).unwrap();
        assert_eq!(content, "post-clear line\n");
    }
}
