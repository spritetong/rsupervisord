// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::consts::MAX_LIVE_LOG_LINE_BYTES;
use crate::error::ProgramError;
use crate::logging::backend::LogBackend;
use crate::logging::reader::{InstantLogReader, LogFileReader};
use crate::logging::types::{LogChannel, LogChunk};
use async_trait::async_trait;
use bytes::BytesMut;
use parking_lot::{Mutex, RwLock};
use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::broadcast;

/// An in-memory rotated log segment representing one generation of log records.
#[derive(Debug, Clone, Default)]
pub struct MemorySegment {
    pub data: Vec<u8>,
    pub line_offsets: Vec<usize>,
}

impl MemorySegment {
    pub fn new() -> Self {
        Self {
            data: Vec::new(),
            line_offsets: Vec::new(),
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            data: Vec::with_capacity(capacity),
            line_offsets: Vec::new(),
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    #[inline]
    pub fn line_count(&self) -> usize {
        self.line_offsets.len()
    }
}

/// Rotator for a single log channel (e.g. stdout or stderr) stored entirely in memory.
///
/// Lock order: acquire `active` before `backups_queue` in every method; never
/// reverse. `pending_line` is independent and must never be held while taking
/// either segment lock.
pub struct InMemoryChannelRotator {
    max_bytes: usize,
    backups: usize,
    active: RwLock<MemorySegment>,
    backups_queue: RwLock<VecDeque<MemorySegment>>,
    broadcast_tx: broadcast::Sender<String>,
    /// Incomplete line tail awaiting a newline for live broadcast.
    pending_line: Mutex<BytesMut>,
    /// Monotonic total cumulative byte count since startup/seeding (satisfies supervisor offset semantics).
    cumulative_bytes: AtomicU64,
}

/// Copies a slice of bytes `[start..start + length]` from consecutive slices into a new `Vec<u8>`.
fn extract_window(slices: &[&[u8]], mut start: usize, mut length: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(length);
    for slice in slices {
        if length == 0 {
            break;
        }
        if start < slice.len() {
            let take = (slice.len() - start).min(length);
            out.extend_from_slice(&slice[start..start + take]);
            length -= take;
            start = 0;
        } else {
            start -= slice.len();
        }
    }
    out
}

impl InMemoryChannelRotator {
    /// Creates a new in-memory channel rotator with segment size threshold and retained backups.
    pub fn new(max_bytes: usize, backups: usize) -> Self {
        let (broadcast_tx, _) = broadcast::channel(1024);
        Self {
            max_bytes: max_bytes.max(1),
            backups,
            active: RwLock::new(MemorySegment::with_capacity(max_bytes.min(64 * 1024))),
            backups_queue: RwLock::new(VecDeque::with_capacity(backups)),
            broadcast_tx,
            pending_line: Mutex::new(BytesMut::new()),
            cumulative_bytes: AtomicU64::new(0),
        }
    }

    /// Creates a new in-memory channel rotator bounded by total stream retention capacity.
    /// Each segment is clamped to at least 1024 bytes to prevent micro-segment thrashing.
    pub fn with_total_capacity(total_capacity: usize, backups: usize) -> Self {
        let segment_count = backups.saturating_add(1).max(1);
        let min_total = segment_count.saturating_mul(1024);
        let safe_capacity = total_capacity.max(min_total);
        let segment_bytes = (safe_capacity / segment_count).max(1024);
        Self::new(segment_bytes, backups)
    }

    /// Returns the maximum byte size of an individual segment.
    #[inline]
    pub fn segment_size(&self) -> usize {
        self.max_bytes
    }

    /// Assembles `data` into complete newline-delimited lines and broadcasts them.
    ///
    /// Bytes are buffered across chunk boundaries so multi-byte UTF-8 sequences
    /// and lines split across reads are never dropped or broken.
    fn broadcast_chunk(&self, data: &[u8]) {
        if data.is_empty() {
            return;
        }

        let has_receivers = self.broadcast_tx.receiver_count() > 0;
        let mut pending = self.pending_line.lock();
        pending.extend_from_slice(data);

        let mut start = 0usize;
        while start < pending.len() {
            let Some(rel) = pending[start..].iter().position(|&b| b == b'\n') else {
                break;
            };
            let mut line = &pending[start..start + rel];
            if line.last() == Some(&b'\r') {
                line = &line[..line.len() - 1];
            }
            if has_receivers {
                let text = String::from_utf8_lossy(line);
                let _ = self.broadcast_tx.send(text.into_owned());
            }
            start += rel + 1;
        }
        if start > 0 {
            let _ = pending.split_to(start);
        }

        // Force-flush overlong unterminated input to bound memory.
        if pending.len() > MAX_LIVE_LOG_LINE_BYTES {
            if has_receivers {
                let text = String::from_utf8_lossy(&pending);
                let _ = self.broadcast_tx.send(text.into_owned());
            }
            pending.clear();
        }
    }

    /// Broadcasts any incomplete pending line (e.g. on stream EOF) and clears it.
    pub fn flush_broadcast(&self) {
        let mut pending = self.pending_line.lock();
        if pending.is_empty() {
            return;
        }
        if self.broadcast_tx.receiver_count() > 0 {
            let text = String::from_utf8_lossy(&pending);
            let _ = self.broadcast_tx.send(text.into_owned());
        }
        pending.clear();
    }

    /// Appends a raw chunk of bytes, rotating segments when `max_bytes` is reached.
    /// Oversize chunks larger than `max_bytes` are split across segments.
    pub fn append_bytes(&self, data: &[u8]) {
        if data.is_empty() {
            return;
        }

        self.broadcast_chunk(data);

        let mut rem = data;
        while !rem.is_empty() {
            // Lock order: active before backups_queue.
            let mut active = self.active.write();

            if active.len() >= self.max_bytes && !active.is_empty() {
                let mut backups = self.backups_queue.write();
                if self.backups > 0 {
                    if backups.len() >= self.backups {
                        backups.pop_front();
                    }
                    let old_active = std::mem::replace(
                        &mut *active,
                        MemorySegment::with_capacity(self.max_bytes.min(64 * 1024)),
                    );
                    backups.push_back(old_active);
                } else {
                    active.data.clear();
                    active.line_offsets.clear();
                }
            }

            let space = self.max_bytes.saturating_sub(active.len()).max(1);
            let chunk_len = rem.len().min(space);
            let chunk = &rem[..chunk_len];

            let prev_len = active.data.len();
            active.data.extend_from_slice(chunk);

            for (i, &b) in chunk.iter().enumerate() {
                if b == b'\n' {
                    active.line_offsets.push(prev_len + i + 1);
                }
            }

            self.cumulative_bytes
                .fetch_add(chunk_len as u64, Ordering::Relaxed);
            rem = &rem[chunk_len..];
        }
    }

    /// Returns the total cumulative bytes written to this channel since inception/seeding.
    #[inline]
    pub fn cumulative_bytes(&self) -> u64 {
        self.cumulative_bytes.load(Ordering::SeqCst)
    }

    /// Seeds this in-memory channel with the trailing chunk of an existing disk file,
    /// sets the initial cumulative byte offset to the disk file size, and rotates
    /// the on-disk file (renaming to `<path>.1`) to prevent subsequent disk writes.
    pub fn seed_from_file(&self, path: &Path, max_seed_bytes: usize) -> std::io::Result<u64> {
        let bounded_seed = max_seed_bytes.min(self.max_bytes);
        match LogFileReader::seed_and_archive(path, bounded_seed, Some("1"))? {
            Some((bytes, file_size)) => {
                if !bytes.is_empty() {
                    let mut active = self.active.write();
                    let prev_len = active.data.len();
                    active.data.extend_from_slice(&bytes);
                    for (i, &b) in bytes.iter().enumerate() {
                        if b == b'\n' {
                            active.line_offsets.push(prev_len + i + 1);
                        }
                    }
                }
                self.cumulative_bytes.store(file_size, Ordering::SeqCst);
                Ok(file_size)
            }
            None => Ok(0),
        }
    }

    /// Appends a text line with an added newline delimiter.
    ///
    /// Broadcasts exactly once via [`Self::append_bytes`].
    pub fn append_line(&self, line: &str) {
        let mut bytes = Vec::with_capacity(line.len() + 1);
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
        self.append_bytes(&bytes);
    }

    /// Returns a flat snapshot of all active and backup segment bytes.
    pub fn snapshot_bytes(&self) -> Vec<u8> {
        // Lock order: active before backups_queue (data order is still
        // backups first, then active, for chronological layout).
        let active = self.active.read();
        let backups = self.backups_queue.read();

        let total_size: usize = active.len() + backups.iter().map(|s| s.len()).sum::<usize>();
        let mut out = Vec::with_capacity(total_size);

        for seg in backups.iter() {
            out.extend_from_slice(&seg.data);
        }
        out.extend_from_slice(&active.data);
        out
    }

    /// Reads bytes starting from monotonic offset with length, returning `(content, total_size, overflow)`.
    /// Slices directly across segment windows to eliminate O(N) buffer clones and data/offset races.
    pub fn read_bytes(&self, offset: i64, length: i64) -> (String, i64, bool) {
        let active = self.active.read();
        let backups = self.backups_queue.read();
        let total_sz = self.cumulative_bytes.load(Ordering::SeqCst) as i64;

        let retained_sz: usize = backups.iter().map(|s| s.len()).sum::<usize>() + active.len();
        let oldest_available_offset = total_sz.saturating_sub(retained_sz as i64);

        let mut overflow = false;
        let mut len = length;

        if len < 0 {
            len = 0;
        }

        let (start_off, to_read) = if offset < 0 {
            let target = total_sz + offset;
            let actual_start = if target < oldest_available_offset {
                overflow = true;
                oldest_available_offset
            } else {
                target
            };
            let rem = (total_sz - actual_start) as usize;
            let r = if len == 0 {
                rem
            } else {
                (len as usize).min(rem)
            };
            (actual_start, r)
        } else {
            if offset < oldest_available_offset {
                overflow = true;
                (oldest_available_offset, (len as usize).min(retained_sz))
            } else if offset >= total_sz {
                return (String::new(), total_sz, false);
            } else {
                let rem = (total_sz - offset) as usize;
                let r = if len == 0 {
                    rem
                } else {
                    (len as usize).min(rem)
                };
                (offset, r)
            }
        };

        let local_start = (start_off - oldest_available_offset) as usize;
        let local_end = (local_start + to_read).min(retained_sz);
        let slice_len = local_end.saturating_sub(local_start);

        let slices: Vec<&[u8]> = backups
            .iter()
            .map(|s| s.data.as_slice())
            .chain(std::iter::once(active.data.as_slice()))
            .collect();
        let window = extract_window(&slices, local_start, slice_len);

        (
            String::from_utf8_lossy(&window).to_string(),
            total_sz,
            overflow,
        )
    }

    /// Tails bytes backwards from buffer end or monotonic offset, matching supervisor XML-RPC semantics.
    /// Slices directly across segment windows to eliminate O(N) buffer clones and data/offset races.
    pub fn tail_bytes(&self, offset: i64, length: i64) -> (String, i64, bool) {
        let active = self.active.read();
        let backups = self.backups_queue.read();
        let total_sz = self.cumulative_bytes.load(Ordering::SeqCst) as i64;

        let retained_sz: usize = backups.iter().map(|s| s.len()).sum::<usize>() + active.len();
        let oldest_available_offset = total_sz.saturating_sub(retained_sz as i64);

        let mut len = length;
        if len < 0 {
            len = 0;
        }

        let slices: Vec<&[u8]> = backups
            .iter()
            .map(|s| s.data.as_slice())
            .chain(std::iter::once(active.data.as_slice()))
            .collect();

        if offset == 0 {
            let actual_len = if len == 0 {
                retained_sz
            } else {
                (len as usize).min(retained_sz)
            };
            let start = retained_sz.saturating_sub(actual_len);
            let window = extract_window(&slices, start, actual_len);
            let has_overflow = total_sz > actual_len as i64;
            return (
                String::from_utf8_lossy(&window).to_string(),
                total_sz,
                has_overflow,
            );
        }

        let mut overflow = false;
        let mut off = offset;

        if off < oldest_available_offset {
            overflow = true;
            off = oldest_available_offset;
        }

        if off >= total_sz {
            return (String::new(), total_sz, false);
        }

        let local_start = (off - oldest_available_offset) as usize;
        let to_read = if len == 0 {
            retained_sz - local_start
        } else {
            (len as usize).min(retained_sz - local_start)
        };
        let local_end = local_start + to_read;
        let slice_len = local_end.saturating_sub(local_start);
        let window = extract_window(&slices, local_start, slice_len);

        let new_offset = off + slice_len as i64;
        (
            String::from_utf8_lossy(&window).to_string(),
            new_offset,
            overflow,
        )
    }

    /// Returns the most recent `max_lines` lines.
    pub fn read_lines(&self, max_lines: Option<usize>) -> Vec<String> {
        let active = self.active.read();
        let backups = self.backups_queue.read();

        let slices: Vec<&[u8]> = backups
            .iter()
            .map(|s| s.data.as_slice())
            .chain(std::iter::once(active.data.as_slice()))
            .collect();

        let mut lines = Vec::new();
        let mut cur_line_bytes = Vec::new();
        for slice in slices {
            for &b in slice {
                if b == b'\n' {
                    if cur_line_bytes.last() == Some(&b'\r') {
                        cur_line_bytes.pop();
                    }
                    lines.push(String::from_utf8_lossy(&cur_line_bytes).into_owned());
                    cur_line_bytes.clear();
                } else {
                    cur_line_bytes.push(b);
                }
            }
        }
        if !cur_line_bytes.is_empty() {
            if cur_line_bytes.last() == Some(&b'\r') {
                cur_line_bytes.pop();
            }
            lines.push(String::from_utf8_lossy(&cur_line_bytes).into_owned());
        }

        match max_lines {
            Some(n) if n < lines.len() => lines[lines.len() - n..].to_vec(),
            _ => lines,
        }
    }

    /// Clears both active and backup segments, any pending broadcast line, and resets cumulative bytes.
    pub fn clear(&self) {
        {
            // Lock order: active before backups_queue.
            let mut active = self.active.write();
            let mut backups = self.backups_queue.write();
            active.data.clear();
            active.line_offsets.clear();
            backups.clear();
        }
        self.cumulative_bytes.store(0, Ordering::SeqCst);
        self.pending_line.lock().clear();
    }

    /// Subscribes to real-time incoming lines.
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.broadcast_tx.subscribe()
    }

    /// Returns the total bytes across active and backup segments.
    pub fn byte_size(&self) -> usize {
        // Lock order: active before backups_queue.
        let active = self.active.read();
        let backups = self.backups_queue.read();
        active.len() + backups.iter().map(|s| s.len()).sum::<usize>()
    }

    /// Returns the total line count across active and backup segments.
    pub fn line_count(&self) -> usize {
        // Lock order: active before backups_queue.
        let active = self.active.read();
        let backups = self.backups_queue.read();
        active.line_count() + backups.iter().map(|s| s.line_count()).sum::<usize>()
    }
}

#[async_trait]
impl LogBackend for InMemoryChannelRotator {
    async fn write_chunk(&self, chunk: &LogChunk) -> Result<(), ProgramError> {
        self.append_bytes(&chunk.data);
        Ok(())
    }

    async fn flush(&self) -> Result<(), ProgramError> {
        self.flush_broadcast();
        Ok(())
    }

    fn clear(&self) -> Result<(), ProgramError> {
        InMemoryChannelRotator::clear(self);
        Ok(())
    }
}

/// In-memory log rotator managing stdout and stderr channels.
///
/// Functions as both a built-in `LogBackend` and an `InstantLogReader`.
pub struct InMemoryLogRotator {
    stdout: Arc<InMemoryChannelRotator>,
    stderr: Arc<InMemoryChannelRotator>,
}

impl InMemoryLogRotator {
    /// Creates a new in-memory rotator for both stdout and stderr.
    pub fn new(max_bytes: usize, backups: usize) -> Self {
        Self {
            stdout: Arc::new(InMemoryChannelRotator::new(max_bytes, backups)),
            stderr: Arc::new(InMemoryChannelRotator::new(max_bytes, backups)),
        }
    }

    /// Creates a new in-memory rotator bounded by total retained buffer capacity for each channel.
    pub fn with_total_capacity(total_capacity: usize, backups: usize) -> Self {
        Self {
            stdout: Arc::new(InMemoryChannelRotator::with_total_capacity(
                total_capacity,
                backups,
            )),
            stderr: Arc::new(InMemoryChannelRotator::with_total_capacity(
                total_capacity,
                backups,
            )),
        }
    }

    /// Creates a new in-memory rotator wrapping existing channel rotators.
    pub fn new_with_channels(
        stdout: Arc<InMemoryChannelRotator>,
        stderr: Arc<InMemoryChannelRotator>,
    ) -> Self {
        Self { stdout, stderr }
    }

    /// Returns a reference to the stdout channel rotator.
    #[inline]
    pub fn stdout(&self) -> &Arc<InMemoryChannelRotator> {
        &self.stdout
    }

    /// Returns a reference to the stderr channel rotator.
    #[inline]
    pub fn stderr(&self) -> &Arc<InMemoryChannelRotator> {
        &self.stderr
    }

    /// Seeds stdout and stderr channels from existing on-disk log files and rotates them.
    pub fn seed_from_files(
        &self,
        stdout_path: Option<&Path>,
        stderr_path: Option<&Path>,
        max_seed_bytes: usize,
    ) {
        if let Some(p) = stdout_path {
            let _ = self.stdout.seed_from_file(p, max_seed_bytes);
        }
        if let Some(p) = stderr_path {
            let _ = self.stderr.seed_from_file(p, max_seed_bytes);
        }
    }

    /// Clears logs from one or both channels.
    pub fn clear(&self, channel: Option<LogChannel>) {
        match channel {
            Some(LogChannel::Stdout) => self.stdout.clear(),
            Some(LogChannel::Stderr) => self.stderr.clear(),
            None => {
                self.stdout.clear();
                self.stderr.clear();
            }
        }
    }
}

impl Default for InMemoryLogRotator {
    fn default() -> Self {
        Self::with_total_capacity(
            crate::consts::DEFAULT_IN_MEMORY_LOG_BUFFER_SIZE,
            crate::consts::DEFAULT_LOG_BACKUPS,
        )
    }
}

#[async_trait]
impl LogBackend for InMemoryLogRotator {
    async fn write_chunk(&self, chunk: &LogChunk) -> Result<(), ProgramError> {
        match chunk.channel {
            LogChannel::Stdout => self.stdout.append_bytes(&chunk.data),
            LogChannel::Stderr => self.stderr.append_bytes(&chunk.data),
        }
        Ok(())
    }

    async fn flush(&self) -> Result<(), ProgramError> {
        self.stdout.flush_broadcast();
        self.stderr.flush_broadcast();
        Ok(())
    }

    fn clear(&self) -> Result<(), ProgramError> {
        self.stdout.clear();
        self.stderr.clear();
        Ok(())
    }
}

impl InstantLogReader for InMemoryLogRotator {
    fn read_bytes(&self, channel: LogChannel, offset: i64, length: i64) -> (String, i64, bool) {
        match channel {
            LogChannel::Stdout => self.stdout.read_bytes(offset, length),
            LogChannel::Stderr => self.stderr.read_bytes(offset, length),
        }
    }

    fn tail_bytes(&self, channel: LogChannel, offset: i64, length: i64) -> (String, i64, bool) {
        match channel {
            LogChannel::Stdout => self.stdout.tail_bytes(offset, length),
            LogChannel::Stderr => self.stderr.tail_bytes(offset, length),
        }
    }

    fn read_lines(&self, channel: LogChannel, max_lines: Option<usize>) -> Vec<String> {
        match channel {
            LogChannel::Stdout => self.stdout.read_lines(max_lines),
            LogChannel::Stderr => self.stderr.read_lines(max_lines),
        }
    }

    fn subscribe(&self, channel: LogChannel) -> broadcast::Receiver<String> {
        match channel {
            LogChannel::Stdout => self.stdout.subscribe(),
            LogChannel::Stderr => self.stderr.subscribe(),
        }
    }

    fn clear(&self, channel: Option<LogChannel>) {
        match channel {
            Some(LogChannel::Stdout) => self.stdout.clear(),
            Some(LogChannel::Stderr) => self.stderr.clear(),
            None => {
                self.stdout.clear();
                self.stderr.clear();
            }
        }
    }

    fn line_count(&self, channel: LogChannel) -> usize {
        match channel {
            LogChannel::Stdout => self.stdout.line_count(),
            LogChannel::Stderr => self.stderr.line_count(),
        }
    }

    fn byte_size(&self, channel: LogChannel) -> usize {
        match channel {
            LogChannel::Stdout => self.stdout.byte_size(),
            LogChannel::Stderr => self.stderr.byte_size(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_in_memory_channel_rotation() {
        // max 20 bytes per segment, 2 backups
        let rotator = InMemoryChannelRotator::new(20, 2);

        rotator.append_line("12345"); // ~6 bytes
        rotator.append_line("67890"); // ~6 bytes
        rotator.append_line("abcde"); // ~6 bytes -> total ~18 bytes

        assert_eq!(rotator.backups_queue.read().len(), 0);

        // Next line triggers rotation of first segment to backups
        rotator.append_line("fghij");
        assert_eq!(rotator.backups_queue.read().len(), 1);

        rotator.append_line("klmno");
        rotator.append_line("pqrst");
        rotator.append_line("uvwxy"); // triggers another rotation (18 + 6 > 20)
        assert_eq!(rotator.backups_queue.read().len(), 2);

        // Snapshot still contains lines within retention
        let lines = rotator.read_lines(None);
        assert!(lines.contains(&"fghij".to_string()) || lines.contains(&"uvwxy".to_string()));
    }

    #[test]
    fn test_xmlrpc_tail_bytes_semantics() {
        let rotator = InMemoryChannelRotator::new(1024, 2);
        rotator.append_bytes(b"hello world\n");

        let (data, new_off, overflow) = rotator.tail_bytes(0, 5);
        assert_eq!(data, "orld\n");
        assert_eq!(new_off, 12);
        assert!(overflow);
    }

    #[test]
    fn test_append_line_broadcasts_exactly_once() {
        let rotator = InMemoryChannelRotator::new(1024, 2);
        let mut rx = rotator.subscribe();

        rotator.append_line("only-once");

        assert_eq!(
            rx.try_recv().expect("first broadcast"),
            "only-once",
            "append_line must broadcast the line once"
        );
        assert!(
            rx.try_recv().is_err(),
            "append_line must not double-broadcast through append_bytes"
        );
        assert_eq!(rotator.read_lines(None), vec!["only-once"]);
    }

    #[test]
    fn test_broadcast_assembles_lines_across_chunks() {
        let rotator = InMemoryChannelRotator::new(1024, 2);
        let mut rx = rotator.subscribe();

        rotator.append_bytes(b"hel");
        assert!(
            rx.try_recv().is_err(),
            "partial line must not be broadcast before newline"
        );

        rotator.append_bytes(b"lo\nwor");
        assert_eq!(rx.try_recv().expect("complete line"), "hello");
        assert!(
            rx.try_recv().is_err(),
            "trailing partial line must wait for newline"
        );

        rotator.append_bytes(b"ld\n");
        assert_eq!(rx.try_recv().expect("second line"), "world");
        assert_eq!(rotator.read_lines(None), vec!["hello", "world"]);
    }

    #[test]
    fn test_broadcast_preserves_utf8_across_chunk_boundary() {
        let rotator = InMemoryChannelRotator::new(1024, 2);
        let mut rx = rotator.subscribe();

        // "héllo\n" is 8 bytes; é is 2 bytes (0xC3 0xA9) — split inside it.
        let full = "héllo\n".as_bytes();
        let split = full.iter().position(|&b| b == 0xC3).unwrap() + 1;
        rotator.append_bytes(&full[..split]);
        rotator.append_bytes(&full[split..]);

        assert_eq!(
            rx.try_recv().expect("utf-8 line"),
            "héllo",
            "multi-byte char split across chunks must reassemble"
        );
        assert_eq!(rotator.read_lines(None), vec!["héllo"]);
    }

    #[test]
    fn test_concurrent_append_and_snapshot_no_deadlock() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        // Small max_bytes forces frequent rotation (active→backups write path)
        // while readers hold backups→active in the old code — AB-BA deadlock.
        let rotator = Arc::new(InMemoryChannelRotator::new(32, 2));
        let stop = Arc::new(AtomicBool::new(false));

        let writer = {
            let rotator = Arc::clone(&rotator);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    rotator.append_line("0123456789");
                }
            })
        };

        let reader = {
            let rotator = Arc::clone(&rotator);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let _ = rotator.snapshot_bytes();
                    let _ = rotator.byte_size();
                    let _ = rotator.line_count();
                }
            })
        };

        std::thread::sleep(std::time::Duration::from_millis(200));
        stop.store(true, Ordering::Relaxed);

        // Join with timeout so a deadlock fails fast instead of hanging CI.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = writer.join();
            let _ = reader.join();
            let _ = tx.send(());
        });
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("deadlock detected: concurrent append/snapshot did not complete");
    }

    #[test]
    fn test_oversize_chunk_splits_across_segments() {
        // max 20 bytes per segment, 3 backups
        let rotator = InMemoryChannelRotator::new(20, 3);

        // Append a 50-byte chunk in one call (2.5x the segment limit)
        let large_payload = b"0123456789abcdefghij0123456789abcdefghij0123456789";
        assert_eq!(large_payload.len(), 50);

        rotator.append_bytes(large_payload);

        let active = rotator.active.read();
        let backups = rotator.backups_queue.read();

        // Active segment must never exceed max_bytes (20)
        assert!(active.len() <= 20, "active.len() {} > 20", active.len());
        // All backup segments must never exceed max_bytes (20)
        for (idx, seg) in backups.iter().enumerate() {
            assert!(seg.len() <= 20, "backup[{}] len {} > 20", idx, seg.len());
        }

        // Total retained bytes across active and backups must equal full 50 bytes (20 + 20 + 10)
        let total: usize = active.len() + backups.iter().map(|s| s.len()).sum::<usize>();
        assert_eq!(total, 50);

        // Cumulative monotonic bytes must be exactly 50
        assert_eq!(rotator.cumulative_bytes(), 50);

        // Window read should be completely intact
        let (data, tot, overflow) = rotator.read_bytes(0, 50);
        assert_eq!(tot, 50);
        assert!(!overflow);
        assert_eq!(data, std::str::from_utf8(large_payload).unwrap());
    }

    #[test]
    fn test_with_total_capacity_retention_bound() {
        // Total capacity 10000 bytes, 4 backups -> 5 segments of 2000 bytes each
        let rotator = InMemoryChannelRotator::with_total_capacity(10000, 4);
        assert_eq!(rotator.segment_size(), 2000);

        // Append 20000 bytes
        for _ in 0..10 {
            rotator.append_bytes(&[b'x'; 2000]); // 2000 bytes each
        }

        // Cumulative bytes must be 20000
        assert_eq!(rotator.cumulative_bytes(), 20000);

        // Retained bytes must not exceed total_capacity (10000)
        assert!(
            rotator.byte_size() <= 10000,
            "retained {} exceeds total capacity 10000",
            rotator.byte_size()
        );

        // Verify min safe segment clamping (at least 1024 bytes per segment)
        let clamped = InMemoryChannelRotator::with_total_capacity(100, 3);
        // 4 segments * 1024 = 4096 min capacity -> 1024 per segment
        assert_eq!(clamped.segment_size(), 1024);
    }
}
