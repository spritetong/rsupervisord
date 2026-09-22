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

/// Parses human-readable byte sizes into numeric bytes.
/// Supports units: B, KB, K, MB, M, GB, G (case-insensitive).
pub fn parse_byte_size(s: &str) -> Result<usize, ProgramError> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(ProgramError::ConfigError(
            "Byte size string cannot be empty".to_string(),
        ));
    }

    let upper = trimmed.to_uppercase();
    if let Some(num) = upper.strip_suffix("GB").or_else(|| upper.strip_suffix('G')) {
        let val: f64 = num.trim().parse().map_err(|_| {
            ProgramError::ConfigError(format!("Invalid gigabyte value: '{}'", trimmed))
        })?;
        Ok((val * 1024.0 * 1024.0 * 1024.0) as usize)
    } else if let Some(num) = upper.strip_suffix("MB").or_else(|| upper.strip_suffix('M')) {
        let val: f64 = num.trim().parse().map_err(|_| {
            ProgramError::ConfigError(format!("Invalid megabyte value: '{}'", trimmed))
        })?;
        Ok((val * 1024.0 * 1024.0) as usize)
    } else if let Some(num) = upper.strip_suffix("KB").or_else(|| upper.strip_suffix('K')) {
        let val: f64 = num.trim().parse().map_err(|_| {
            ProgramError::ConfigError(format!("Invalid kilobyte value: '{}'", trimmed))
        })?;
        Ok((val * 1024.0) as usize)
    } else if let Some(num) = upper.strip_suffix('B') {
        let val: usize = num
            .trim()
            .parse()
            .map_err(|_| ProgramError::ConfigError(format!("Invalid byte value: '{}'", trimmed)))?;
        Ok(val)
    } else {
        let val: usize = upper.parse().map_err(|_| {
            ProgramError::ConfigError(format!("Invalid numeric byte value: '{}'", trimmed))
        })?;
        Ok(val)
    }
}

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
    fn test_parse_byte_size() {
        assert_eq!(parse_byte_size("1024").unwrap(), 1024);
        assert_eq!(parse_byte_size("1024B").unwrap(), 1024);
        assert_eq!(parse_byte_size("10KB").unwrap(), 10240);
        assert_eq!(parse_byte_size("10K").unwrap(), 10240);
        assert_eq!(parse_byte_size("20MB").unwrap(), 20 * 1024 * 1024);
        assert_eq!(parse_byte_size("20M").unwrap(), 20 * 1024 * 1024);
        assert_eq!(parse_byte_size("1GB").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(
            parse_byte_size("1.5MB").unwrap(),
            (1.5 * 1024.0 * 1024.0) as usize
        );
        assert!(parse_byte_size("").is_err());
        assert!(parse_byte_size("invalid").is_err());
    }

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
