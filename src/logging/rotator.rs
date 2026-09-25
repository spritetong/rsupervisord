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
    Fallback {
        file: File,
        writes_since_fallback: usize,
    },
}

static PANIC_HOOK_INSTALLED: std::sync::Once = std::sync::Once::new();

thread_local! {
    static SILENCE_PANIC_HOOK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn ensure_panic_hook_installed() {
    PANIC_HOOK_INSTALLED.call_once(|| {
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if SILENCE_PANIC_HOOK.with(|s| s.get()) {
                return;
            }
            prev_hook(info);
        }));
    });
}

struct ScopedSilenceGuard;
impl Drop for ScopedSilenceGuard {
    fn drop(&mut self) {
        SILENCE_PANIC_HOOK.with(|s| s.set(false));
    }
}

fn with_silenced_panic<F, R>(f: F) -> std::thread::Result<R>
where
    F: FnOnce() -> R + std::panic::UnwindSafe,
{
    ensure_panic_hook_installed();
    SILENCE_PANIC_HOOK.with(|s| s.set(true));
    let _guard = ScopedSilenceGuard;
    std::panic::catch_unwind(f)
}

fn catch_file_rotate_panic<F: FnOnce() -> std::io::Result<usize> + std::panic::UnwindSafe>(
    f: F,
) -> std::io::Result<usize> {
    match with_silenced_panic(f) {
        Ok(io_res) => io_res,
        Err(_) => Err(std::io::Error::other("file_rotate panicked during write")),
    }
}

fn catch_file_rotate_flush<F: FnOnce() -> std::io::Result<()> + std::panic::UnwindSafe>(
    f: F,
) -> std::io::Result<()> {
    match with_silenced_panic(f) {
        Ok(io_res) => io_res,
        Err(_) => Err(std::io::Error::other("file_rotate panicked during flush")),
    }
}

impl Write for RotatorInner {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Count(w) => {
                catch_file_rotate_panic(std::panic::AssertUnwindSafe(|| w.write(buf)))
            }
            Self::Timestamp(w) => {
                catch_file_rotate_panic(std::panic::AssertUnwindSafe(|| w.write(buf)))
            }
            Self::Append(w) => w.write(buf),
            Self::Fallback { file, .. } => file.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Count(w) => catch_file_rotate_flush(std::panic::AssertUnwindSafe(|| w.flush())),
            Self::Timestamp(w) => {
                catch_file_rotate_flush(std::panic::AssertUnwindSafe(|| w.flush()))
            }
            Self::Append(w) => w.flush(),
            Self::Fallback { file, .. } => file.flush(),
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

    /// Writes a raw byte buffer to the rotating log file with automatic recovery.
    pub fn write_all(&self, mut data: &[u8]) -> std::io::Result<()> {
        let mut guard = self.inner.lock();

        // Check if we can recover from Fallback back to standard FileRotate
        if let RotatorInner::Fallback {
            ref mut writes_since_fallback,
            ..
        } = *guard
        {
            *writes_since_fallback += 1;
            if *writes_since_fallback >= 50 {
                let recovered_res = with_silenced_panic(std::panic::AssertUnwindSafe(|| {
                    Self::build_inner(
                        &self.path,
                        self.max_bytes,
                        self.backups,
                        self.timestamp_suffix,
                    )
                }));
                match recovered_res {
                    Ok(Ok(recovered)) => {
                        *guard = recovered;
                    }
                    _ => {
                        *writes_since_fallback = 0;
                    }
                }
            }
        }

        while !data.is_empty() {
            match guard.write(data) {
                Ok(0) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "failed to write whole buffer to log rotator",
                    ));
                }
                Ok(n) => {
                    data = &data[n..];
                }
                Err(err) => {
                    // Rate-limit warnings to stderr (at most once every 5 seconds)
                    let now_epoch = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let last = self
                        .last_fallback_warn_epoch
                        .load(std::sync::atomic::Ordering::Relaxed);
                    if now_epoch.saturating_sub(last) >= 5 {
                        self.last_fallback_warn_epoch
                            .store(now_epoch, std::sync::atomic::Ordering::Relaxed);
                        eprintln!(
                            "[rsupervisord] Warning: log write/rotation failed for '{}': {}; falling back to append-only mode",
                            self.path.display(),
                            err
                        );
                    }
                    match std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&self.path)
                    {
                        Ok(mut file) => {
                            file.write_all(data)?;
                            *guard = RotatorInner::Fallback {
                                file,
                                writes_since_fallback: 0,
                            };
                            return Ok(());
                        }
                        Err(fallback_err) => {
                            return Err(fallback_err);
                        }
                    }
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

        // 3. Verify inner state has recovered back to RotatorInner::Count
        {
            let guard = rotator.inner.lock();
            assert!(
                matches!(*guard, RotatorInner::Count(_)),
                "Expected rotator to recover back to Count variant"
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
    fn test_thread_isolated_panic_silencing() {
        // Test that with_silenced_panic catches a panic without propagating
        let res = with_silenced_panic(std::panic::AssertUnwindSafe(|| {
            panic!("test panic that should be caught and silenced");
        }));
        assert!(res.is_err());

        // Verify that SILENCE_PANIC_HOOK is false afterwards
        assert!(!SILENCE_PANIC_HOOK.with(|s| s.get()));
    }
}
