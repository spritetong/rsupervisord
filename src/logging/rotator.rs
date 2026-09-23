// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::error::ProgramError;
use file_rotate::{ContentLimit, FileRotate, compression::Compression, suffix::AppendCount};
use parking_lot::Mutex;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Thread-safe file rotator wrapping `file_rotate::FileRotate`.
#[derive(Clone)]
pub struct LogRotator {
    path: PathBuf,
    inner: Arc<Mutex<FileRotate<AppendCount>>>,
}

impl LogRotator {
    /// Creates a new LogRotator at the given path with max_bytes and retained backups count.
    pub fn new(
        path: impl AsRef<Path>,
        max_bytes: usize,
        backups: usize,
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
        std::fs::OpenOptions::new()
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

        let max_bytes = max_bytes.max(1);
        let rotator = FileRotate::new(
            path_buf.clone(),
            AppendCount::new(backups),
            ContentLimit::Bytes(max_bytes),
            Compression::None,
            None,
        );

        Ok(Self {
            path: path_buf,
            inner: Arc::new(Mutex::new(rotator)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_log_rotator_rotation() {
        let dir = tempdir().unwrap();
        let log_file = dir.path().join("test.log");

        // Rotate every 15 bytes, keep 2 backups
        let rotator = LogRotator::new(&log_file, 15, 2).unwrap();

        rotator.write_line("Line 1: 12345").unwrap(); // 14 bytes with newline
        rotator.write_line("Line 2: 67890").unwrap(); // triggers rotation

        assert!(log_file.exists());
        let backup1 = dir.path().join("test.log.1");
        assert!(backup1.exists(), "Backup test.log.1 should exist");
        assert!(rotator.file_size().is_ok());
    }
}
