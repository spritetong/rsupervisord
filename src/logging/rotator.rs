// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::error::ProgramError;
use crate::logging::backend::LogBackend;
use crate::logging::types::LogChunk;
use async_trait::async_trait;
use parking_lot::Mutex;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

enum RotatorInner {
    Active {
        file: Option<File>,
        current_size: u64,
    },
    Fallback {
        file: File,
        writes_since_fallback: usize,
    },
}

/// Thread-safe resilient file rotator with standard library atomic rotation and append-only fallback.
#[derive(Clone)]
pub struct LogRotator {
    path: PathBuf,
    max_bytes: usize,
    backups: usize,
    timestamp_suffix: bool,
    inner: Arc<Mutex<RotatorInner>>,
    last_fallback_warn_epoch: Arc<std::sync::atomic::AtomicU64>,
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
        _backups: usize,
        _timestamp_suffix: bool,
    ) -> Result<RotatorInner, ProgramError> {
        let file = std::fs::OpenOptions::new()
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

        let current_size = if max_bytes > 0 {
            std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
        } else {
            0
        };

        Ok(RotatorInner::Active {
            file: Some(file),
            current_size,
        })
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
            last_fallback_warn_epoch: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        })
    }

    /// Writes a line of text to the rotating log file.
    pub fn write_line(&self, line: &str) -> std::io::Result<()> {
        let mut buf = Vec::with_capacity(line.len() + 1);
        buf.extend_from_slice(line.as_bytes());
        buf.push(b'\n');
        self.write_all(&buf)
    }

    fn rotate_on_disk(path: &Path, backups: usize, timestamp_suffix: bool) -> std::io::Result<()> {
        if !path.exists() {
            return Ok(());
        }
        if backups == 0 {
            let _ = std::fs::remove_file(path);
            return Ok(());
        }

        if timestamp_suffix {
            let timestamp = chrono::Local::now().format("%Y-%m-%dT%H-%M-%S").to_string();
            let mut target = PathBuf::from(format!("{}.{}", path.display(), timestamp));
            if target.exists() {
                for counter in 1.. {
                    let candidate =
                        PathBuf::from(format!("{}.{}-{}", path.display(), timestamp, counter));
                    if !candidate.exists() {
                        target = candidate;
                        break;
                    }
                }
            }
            std::fs::rename(path, &target)?;

            // Prune older files exceeding backups count
            if let Some(parent) = path.parent() {
                let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let prefix = format!("{}.", filename);
                if let Ok(entries) = std::fs::read_dir(parent) {
                    let mut ts_files = Vec::new();
                    for entry in entries.flatten() {
                        if let Ok(ft) = entry.file_type()
                            && ft.is_file()
                        {
                            let name = entry.file_name().to_string_lossy().into_owned();
                            if name.starts_with(&prefix) && name != filename {
                                ts_files.push((name, entry.path()));
                            }
                        }
                    }
                    ts_files.sort_by(|a, b| a.0.cmp(&b.0));
                    if ts_files.len() > backups {
                        let to_remove = ts_files.len() - backups;
                        for (_, old_path) in ts_files.into_iter().take(to_remove) {
                            let _ = std::fs::remove_file(old_path);
                        }
                    }
                }
            }
        } else {
            // Numeric rotation: .1, .2, ..., .backups
            let oldest = PathBuf::from(format!("{}.{}", path.display(), backups));
            if oldest.exists() {
                std::fs::remove_file(&oldest)?;
            }
            for i in (1..backups).rev() {
                let src = PathBuf::from(format!("{}.{}", path.display(), i));
                let dst = PathBuf::from(format!("{}.{}", path.display(), i + 1));
                if src.exists() {
                    if dst.exists() {
                        std::fs::remove_file(&dst)?;
                    }
                    std::fs::rename(&src, &dst)?;
                }
            }
            let dst_1 = PathBuf::from(format!("{}.1", path.display()));
            if dst_1.exists() {
                std::fs::remove_file(&dst_1)?;
            }
            std::fs::rename(path, &dst_1)?;
        }
        Ok(())
    }

    fn write_fallback(
        &self,
        guard: &mut RotatorInner,
        data: &[u8],
        reason: &str,
    ) -> std::io::Result<()> {
        let now_epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let last = self
            .last_fallback_warn_epoch
            .load(std::sync::atomic::Ordering::Relaxed);
        let should_warn = now_epoch.saturating_sub(last) >= 5;
        if should_warn {
            self.last_fallback_warn_epoch
                .store(now_epoch, std::sync::atomic::Ordering::Relaxed);
            let already_fallback = matches!(*guard, RotatorInner::Fallback { .. });
            if already_fallback {
                eprintln!(
                    "[rsupervisord] Warning: log write failed while already in append-only mode for '{}': {}",
                    self.path.display(),
                    reason
                );
            } else {
                eprintln!(
                    "[rsupervisord] Warning: log write/rotation failed for '{}': {}; falling back to append-only mode",
                    self.path.display(),
                    reason
                );
            }
        }

        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            Ok(mut file) => match file.write_all(data) {
                Ok(()) => {
                    *guard = RotatorInner::Fallback {
                        file,
                        writes_since_fallback: 0,
                    };
                    Ok(())
                }
                Err(e) => {
                    if should_warn {
                        eprintln!(
                            "[rsupervisord] Warning: append-only fallback write failed for '{}': {}",
                            self.path.display(),
                            e
                        );
                    }
                    Err(e)
                }
            },
            Err(e) => {
                if should_warn {
                    eprintln!(
                        "[rsupervisord] Warning: append-only fallback open failed for '{}': {}",
                        self.path.display(),
                        e
                    );
                }
                Err(e)
            }
        }
    }

    /// Writes a raw byte buffer to the rotating log file with automatic recovery.
    pub fn write_all(&self, mut data: &[u8]) -> std::io::Result<()> {
        let mut guard = self.inner.lock();

        // Check if we can recover from Fallback back to standard Active rotator
        if let RotatorInner::Fallback {
            ref mut writes_since_fallback,
            ..
        } = *guard
        {
            *writes_since_fallback += 1;
            if *writes_since_fallback >= 50 {
                match Self::build_inner(
                    &self.path,
                    self.max_bytes,
                    self.backups,
                    self.timestamp_suffix,
                ) {
                    Ok(recovered) => {
                        *guard = recovered;
                    }
                    Err(_) => {
                        *writes_since_fallback = 0;
                    }
                }
            }
        }

        match *guard {
            RotatorInner::Active {
                ref mut file,
                ref mut current_size,
            } => {
                if self.max_bytes == 0 {
                    let f = file
                        .as_mut()
                        .ok_or_else(|| std::io::Error::other("file handle missing"))?;
                    if let Err(e) = f.write_all(data) {
                        return self.write_fallback(&mut guard, data, &e.to_string());
                    }
                    *current_size += data.len() as u64;
                    return Ok(());
                }

                while !data.is_empty() {
                    if *current_size + data.len() as u64 > self.max_bytes as u64 {
                        let bytes_left =
                            (self.max_bytes as u64).saturating_sub(*current_size) as usize;
                        if bytes_left > 0 {
                            let f = file
                                .as_mut()
                                .ok_or_else(|| std::io::Error::other("file handle missing"))?;
                            if let Err(e) = f.write_all(&data[..bytes_left]) {
                                return self.write_fallback(&mut guard, data, &e.to_string());
                            }
                            *current_size += bytes_left as u64;
                            data = &data[bytes_left..];
                        }

                        // Close current file handle so Windows allows renaming
                        if let Some(mut f) = file.take() {
                            let _ = f.flush();
                        }

                        if let Err(e) =
                            Self::rotate_on_disk(&self.path, self.backups, self.timestamp_suffix)
                        {
                            return self.write_fallback(&mut guard, data, &e.to_string());
                        }

                        match std::fs::OpenOptions::new()
                            .create(true)
                            .write(true)
                            .truncate(true)
                            .open(&self.path)
                        {
                            Ok(new_file) => {
                                *file = Some(new_file);
                                *current_size = 0;
                            }
                            Err(e) => {
                                return self.write_fallback(&mut guard, data, &e.to_string());
                            }
                        }
                    } else {
                        let f = file
                            .as_mut()
                            .ok_or_else(|| std::io::Error::other("file handle missing"))?;
                        if let Err(e) = f.write_all(data) {
                            return self.write_fallback(&mut guard, data, &e.to_string());
                        }
                        *current_size += data.len() as u64;
                        break;
                    }
                }
                Ok(())
            }
            RotatorInner::Fallback { ref mut file, .. } => {
                if let Err(e) = file.write_all(data) {
                    return self.write_fallback(&mut guard, data, &e.to_string());
                }
                Ok(())
            }
        }
    }

    /// Flushes any buffered content to disk.
    pub fn flush(&self) -> std::io::Result<()> {
        let mut guard = self.inner.lock();
        match *guard {
            RotatorInner::Active { ref mut file, .. } => {
                if let Some(f) = file {
                    f.flush()?;
                }
            }
            RotatorInner::Fallback { ref mut file, .. } => {
                file.flush()?;
            }
        }
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
                RotatorInner::Fallback {
                    file,
                    writes_since_fallback: 0,
                }
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

    fn clear(&self) -> Result<(), ProgramError> {
        LogRotator::clear(self).map_err(|e| {
            ProgramError::PlatformError(format!(
                "Failed to clear log file '{}': {}",
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

    #[test]
    fn test_log_rotator_fallback_auto_recovery() {
        let dir = tempdir().unwrap();
        let log_file = dir.path().join("recovery.log");
        let rotator = LogRotator::with_options(&log_file, 50, 2, false).unwrap();

        // 1. Manually transition to Fallback state
        {
            let mut guard = rotator.inner.lock();
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_file)
                .unwrap();
            *guard = RotatorInner::Fallback {
                file,
                writes_since_fallback: 0,
            };
        }

        // Verify it is currently Fallback
        {
            let guard = rotator.inner.lock();
            assert!(matches!(*guard, RotatorInner::Fallback { .. }));
        }

        // 2. Perform 50 writes to trigger auto-recovery
        for i in 0..50 {
            rotator
                .write_all(format!("fallback-line-{}\n", i).as_bytes())
                .unwrap();
        }

        // 3. Verify inner state has recovered back to RotatorInner::Active
        {
            let guard = rotator.inner.lock();
            assert!(
                matches!(*guard, RotatorInner::Active { .. }),
                "Expected rotator to recover back to Active variant"
            );
        }

        // 4. Subsequent writes should rotate files normally
        rotator
            .write_all(b"large line that exceeds segment max bytes after recovery 1234567890\n")
            .unwrap();
        rotator
            .write_all(b"another line that pushes past limit into rotation\n")
            .unwrap();

        let backup1 = dir.path().join("recovery.log.1");
        assert!(
            backup1.exists(),
            "Backup recovery.log.1 should be created after recovery to FileRotate"
        );
    }

    #[test]
    fn test_log_rotator_blocked_target_entry_to_fallback() {
        let dir = tempdir().unwrap();
        let log_file = dir.path().join("panic_entry.log");
        let rotator = LogRotator::with_options(&log_file, 50, 1, false).unwrap();

        // Create an unremovable orphan file (exclusive lock on Windows, unwritable dir on Unix)
        let orphan = dir.path().join("panic_entry.log.1");
        std::fs::write(&orphan, b"orphan").unwrap();

        #[cfg(windows)]
        let _lock = {
            use std::os::windows::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .share_mode(0)
                .open(&orphan)
                .unwrap()
        };

        #[cfg(not(windows))]
        {
            // On Unix, make file unremovable by removing write permissions on parent dir
            let mut dir_perms = std::fs::metadata(dir.path()).unwrap().permissions();
            dir_perms.set_readonly(true);
            let _ = std::fs::set_permissions(dir.path(), dir_perms);
        }

        // When rotation is blocked because target cannot be removed, rotator enters fallback
        rotator.write_all(&[b'a'; 60]).unwrap();

        {
            let guard = rotator.inner.lock();
            assert!(
                matches!(*guard, RotatorInner::Fallback { .. }),
                "Blocked rotation must transition the rotator into Fallback mode without panicking"
            );
        }

        // The fallback file remains writable afterwards.
        rotator.write_line("still writable").unwrap();

        #[cfg(not(windows))]
        {
            let mut perms = std::fs::metadata(dir.path()).unwrap().permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            let _ = std::fs::set_permissions(dir.path(), perms);
        }
    }
}
