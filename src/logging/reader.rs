// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::logging::in_memory_rotator::InMemoryChannelRotator;
use crate::logging::types::LogChannel;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::broadcast;

/// Errors that can occur during log reading operations.
#[derive(Debug, thiserror::Error)]
pub enum LogReadError {
    #[error("Log file does not exist: {0}")]
    NoFile(String),
    #[error("Invalid log read arguments: {0}")]
    BadArguments(String),
    #[error("I/O error reading log: {0}")]
    Io(#[from] std::io::Error),
}

/// Interface for querying, tailing, and subscribing to instant in-memory log caches.
pub trait InstantLogReader: Send + Sync + 'static {
    /// Reads log data starting from a byte offset for a specific channel.
    ///
    /// Returns `(content, new_offset, overflow)` matching supervisor XML-RPC semantics.
    fn read_bytes(&self, channel: LogChannel, offset: i64, length: i64) -> (String, i64, bool);

    /// Tails log data backwards from the end of the buffer.
    ///
    /// Returns `(content, new_offset, overflow)` matching supervisor XML-RPC semantics.
    fn tail_bytes(&self, channel: LogChannel, offset: i64, length: i64) -> (String, i64, bool);

    /// Reads up to `max_lines` recent log lines. If `max_lines` is None, all cached lines are returned.
    fn read_lines(&self, channel: LogChannel, max_lines: Option<usize>) -> Vec<String>;

    /// Subscribes to real-time incoming lines for the specified channel.
    fn subscribe(&self, channel: LogChannel) -> broadcast::Receiver<String>;

    /// Clears the in-memory log buffer. If channel is None, clears all channels.
    fn clear(&self, channel: Option<LogChannel>);

    /// Returns the total number of lines currently cached in memory for the given channel.
    fn line_count(&self, channel: LogChannel) -> usize;

    /// Returns the total size in bytes of the cached log data for the given channel.
    fn byte_size(&self, channel: LogChannel) -> usize;
}

/// Centralized utility helper for reading, tailing, and pre-reading disk log files.
pub struct LogFileReader;

impl LogFileReader {
    /// Reads a slice of a log file according to supervisor `readLog` semantics:
    /// - If `offset < 0`, reads `abs(offset)` bytes ending at the end of the file (`length` must be 0).
    /// - If `offset >= 0`, reads `length` bytes starting from `offset`. If `length == 0`, reads to EOF.
    pub fn read_bytes(path: &Path, offset: i64, length: i64) -> Result<String, LogReadError> {
        if !path.exists() {
            return Err(LogReadError::NoFile(path.display().to_string()));
        }

        let mut file = std::fs::File::open(path)?;
        let file_size = file.metadata()?.len() as i64;

        let (pos, to_read) = Self::compute_read_window(file_size, offset, length)?;
        if to_read == 0 {
            return Ok(String::new());
        }

        use std::io::{Read, Seek, SeekFrom};
        file.seek(SeekFrom::Start(pos as u64))?;
        let mut buf = Vec::with_capacity(to_read as usize);
        std::io::Read::take(&mut file, to_read as u64).read_to_end(&mut buf)?;

        Ok(String::from_utf8_lossy(&buf).to_string())
    }

    /// Reads a slice from an in-memory byte buffer according to supervisor `readLog` semantics.
    pub fn read_bytes_from_slice(
        data: &[u8],
        offset: i64,
        length: i64,
    ) -> Result<String, LogReadError> {
        let total_len = data.len() as i64;
        let (pos, to_read) = Self::compute_read_window(total_len, offset, length)?;
        if to_read == 0 {
            return Ok(String::new());
        }
        let end = (pos + to_read) as usize;
        let slice = &data[pos as usize..end.min(data.len())];
        Ok(String::from_utf8_lossy(slice).to_string())
    }

    /// Tails a slice from an in-memory byte buffer according to supervisor `tailProcessLog` semantics.
    pub fn tail_bytes_from_slice(data: &[u8], offset: i64, length: i64) -> (String, i64, bool) {
        let sz = data.len() as i64;
        let (off, len, overflow) = Self::compute_tail_window(sz, offset, length);

        let slice = if len == 0 || off >= sz {
            &[]
        } else {
            let end = (off + len).min(sz) as usize;
            &data[off as usize..end]
        };

        (String::from_utf8_lossy(slice).to_string(), sz, overflow)
    }

    /// Helper to compute `(pos, to_read)` for XML-RPC read semantics.
    fn compute_read_window(
        total_len: i64,
        offset: i64,
        length: i64,
    ) -> Result<(i64, i64), LogReadError> {
        if offset < 0 {
            if length != 0 {
                return Err(LogReadError::BadArguments(
                    "length must be 0 when offset is negative".to_string(),
                ));
            }
            let abs_offset = offset.unsigned_abs() as i64;
            let pos = (total_len - abs_offset).max(0);
            let to_read = (total_len - pos).min(abs_offset);
            Ok((pos, to_read))
        } else {
            if length < 0 {
                return Err(LogReadError::BadArguments(
                    "length cannot be negative".to_string(),
                ));
            }
            let pos = offset.min(total_len);
            let remaining = total_len - pos;
            let to_read = if length == 0 {
                remaining
            } else {
                length.min(remaining)
            };
            Ok((pos, to_read))
        }
    }

    /// Helper to compute `(offset, length, overflow)` for XML-RPC tail semantics.
    pub fn compute_tail_window(sz: i64, offset: i64, length: i64) -> (i64, i64, bool) {
        let mut overflow = false;
        let mut off = offset;
        let mut len = length;

        if sz > (off + len) {
            overflow = true;
            off = sz - 1;
        }

        if (off + len) > sz {
            if off > (sz - 1) {
                len = 0;
            }
            off = sz - len;
        }

        if off < 0 {
            off = 0;
        }
        if len < 0 {
            len = 0;
        }

        (off, len, overflow)
    }

    /// Tails a slice of a log file according to supervisor `tailProcessLog` semantics:
    /// - Reads up to `length` bytes starting at `offset`.
    /// - Detects file truncation or rotation, adjusting `offset` and returning `overflow = true`.
    /// - Returns `(content, new_offset, overflow)`.
    pub fn tail_bytes(path: &Path, offset: i64, length: i64) -> (String, i64, bool) {
        use std::fs::File;
        use std::io::{Read, Seek, SeekFrom};

        let mut file = match File::open(path) {
            Ok(f) => f,
            Err(_) => return (String::new(), offset, false),
        };
        let sz = match file.metadata() {
            Ok(m) => m.len() as i64,
            Err(_) => return (String::new(), offset, false),
        };

        let (off, len, overflow) = Self::compute_tail_window(sz, offset, length);

        let data = if len == 0 {
            Vec::new()
        } else {
            if file.seek(SeekFrom::Start(off as u64)).is_err() {
                return (String::new(), offset, false);
            }
            let mut buf = Vec::with_capacity(len as usize);
            match std::io::Read::take(&mut file, len as u64).read_to_end(&mut buf) {
                Ok(_) => buf,
                Err(_) => Vec::new(),
            }
        };

        (String::from_utf8_lossy(&data).to_string(), sz, overflow)
    }

    /// Pre-reads the trailing bytes of a log file up to `max_seed_bytes`, trimming any
    /// leading partial line up to '\n', and renames the file to an archive.
    ///
    /// Returns `Ok(Some((seeded_bytes, file_size)))` if the file exists, or `Ok(None)` if absent.
    pub fn seed_and_archive(
        path: &Path,
        max_seed_bytes: usize,
        archive_suffix: Option<&str>,
    ) -> std::io::Result<Option<(Vec<u8>, u64)>> {
        if !path.is_file() {
            return Ok(None);
        }

        use std::io::{Read, Seek, SeekFrom};
        let mut file = std::fs::File::open(path)?;
        let file_size = file.metadata()?.len();

        let seeded_bytes = if file_size > 0 {
            let seed_cap = max_seed_bytes.max(1);
            let start_pos = file_size.saturating_sub(seed_cap as u64);
            file.seek(SeekFrom::Start(start_pos))?;

            let mut buf = Vec::with_capacity((file_size - start_pos) as usize);
            file.read_to_end(&mut buf)?;

            // If we seeked into the middle of the file, trim any leading partial line up to '\n'
            if start_pos > 0 {
                if let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                    buf[pos + 1..].to_vec()
                } else {
                    buf
                }
            } else {
                buf
            }
        } else {
            Vec::new()
        };

        // Archive disk file if requested (e.g. app.log -> app.log.1)
        if let Some(suffix) = archive_suffix {
            let rotated_path = format!("{}.{}", path.to_string_lossy(), suffix);
            let dest = Path::new(&rotated_path);
            #[cfg(windows)]
            if dest.exists() {
                let _ = std::fs::remove_file(dest);
            }
            let _ = std::fs::rename(path, dest);
        }

        Ok(Some((seeded_bytes, file_size)))
    }
}

/// An adapter that implements `std::io::Write` and `tracing_subscriber::fmt::MakeWriter`
/// by appending incoming byte slices directly into an `InMemoryChannelRotator`.
#[derive(Clone)]
pub struct InMemoryLogWriter(pub Arc<InMemoryChannelRotator>);

impl std::io::Write for InMemoryLogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.append_bytes(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for InMemoryLogWriter {
    type Writer = InMemoryLogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}
