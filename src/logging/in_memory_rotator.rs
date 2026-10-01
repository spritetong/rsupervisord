// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::consts::MAX_LIVE_LOG_LINE_BYTES;
use crate::error::ProgramError;
use crate::logging::backend::LogBackend;
use crate::logging::continuous_ring::ContinuousRingBuffer;
use crate::logging::reader::{InstantLogReader, LogFileReader};
use crate::logging::types::{LogChannel, LogChunk};
use async_trait::async_trait;
use bytes::BytesMut;
use bytestring::ByteString;
use parking_lot::{Mutex, RwLock};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::broadcast;

/// Rotator for a single log channel (e.g. stdout or stderr) stored entirely in memory.
///
/// Backed by a contiguous [`ContinuousRingBuffer`] for zero-allocation runtime writes
/// and zero-copy [`ByteString`] slice reads.
pub struct InMemoryChannelRotator {
    segment_bytes: usize,
    backups: usize,
    ring: RwLock<ContinuousRingBuffer>,
    broadcast_tx: broadcast::Sender<String>,
    /// Incomplete line tail awaiting a newline for live broadcast.
    pending_line: Mutex<BytesMut>,
}

impl InMemoryChannelRotator {
    /// Creates a new in-memory channel rotator with segment size threshold and retained backups.
    /// Total retention capacity is `max_bytes * (backups + 1)`.
    pub fn new(max_bytes: usize, backups: usize) -> Self {
        let segment_bytes = max_bytes.max(1);
        let total_capacity = segment_bytes.saturating_mul(backups.saturating_add(1));
        let (broadcast_tx, _) = broadcast::channel(1024);
        Self {
            segment_bytes,
            backups,
            ring: RwLock::new(ContinuousRingBuffer::new(total_capacity)),
            broadcast_tx,
            pending_line: Mutex::new(BytesMut::new()),
        }
    }

    /// Creates a new in-memory channel rotator bounded by total stream retention capacity.
    /// Each segment size is calculated as `total_capacity / (backups + 1)`.
    pub fn with_total_capacity(total_capacity: usize, backups: usize) -> Self {
        let segment_count = backups.saturating_add(1).max(1);
        let min_total = segment_count.saturating_mul(1024);
        let safe_capacity = total_capacity.max(min_total);
        let segment_bytes = (safe_capacity / segment_count).max(1024);
        let (broadcast_tx, _) = broadcast::channel(1024);
        Self {
            segment_bytes,
            backups,
            ring: RwLock::new(ContinuousRingBuffer::new(safe_capacity)),
            broadcast_tx,
            pending_line: Mutex::new(BytesMut::new()),
        }
    }

    /// Returns the logical byte size of an individual segment.
    #[inline]
    pub fn segment_size(&self) -> usize {
        self.segment_bytes
    }

    /// Returns the configured backup segment count.
    #[inline]
    pub fn backups(&self) -> usize {
        self.backups
    }

    /// Assembles `data` into complete newline-delimited lines and broadcasts them.
    /// Only incurs allocation overhead if live receivers are actively subscribed.
    fn broadcast_chunk(&self, data: &[u8]) {
        if data.is_empty() {
            return;
        }

        let has_receivers = self.broadcast_tx.receiver_count() > 0;
        let mut pending = self.pending_line.lock();
        if !has_receivers && pending.is_empty() {
            return;
        }

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

    /// Appends a raw chunk of bytes to the contiguous in-memory ring buffer.
    /// Zero heap allocations when writing.
    pub fn append_bytes(&self, data: &[u8]) {
        if data.is_empty() {
            return;
        }

        self.broadcast_chunk(data);
        self.ring.write().write_bytes(data);
    }

    /// Returns the total cumulative bytes written to this channel since inception/seeding.
    #[inline]
    pub fn cumulative_bytes(&self) -> u64 {
        self.ring.read().cumulative_bytes()
    }

    /// Seeds this in-memory channel with the trailing chunk of an existing disk file,
    /// sets the initial cumulative byte offset to the disk file size, and rotates
    /// the on-disk file (renaming to `<path>.1`) to prevent subsequent disk writes.
    pub fn seed_from_file(&self, path: &Path, max_seed_bytes: usize) -> std::io::Result<u64> {
        let bounded_seed = max_seed_bytes.min(self.segment_bytes);
        match LogFileReader::seed_and_archive(path, bounded_seed, Some("1"))? {
            Some((bytes, file_size)) => {
                self.ring.write().seed(&bytes, file_size);
                Ok(file_size)
            }
            None => Ok(0),
        }
    }

    /// Appends a text line with an added newline delimiter.
    /// Broadcasts exactly once via live broadcast channel.
    pub fn append_line(&self, line: &str) {
        if self.broadcast_tx.receiver_count() > 0 {
            let _ = self.broadcast_tx.send(line.to_string());
        }
        self.ring.write().write_line(line);
    }

    /// Returns a flat snapshot of all currently retained bytes.
    pub fn snapshot_bytes(&self) -> Vec<u8> {
        self.ring.read().snapshot_bytes()
    }

    /// Reads bytes starting from monotonic offset with length, returning `(content, total_size, overflow)`.
    pub fn read_bytes(&self, offset: i64, length: i64) -> (String, i64, bool) {
        self.ring.read().read_bytes(offset, length)
    }

    /// Tails bytes backwards from buffer end or monotonic offset, matching supervisor XML-RPC semantics.
    pub fn tail_bytes(&self, offset: i64, length: i64) -> (String, i64, bool) {
        self.ring.read().tail_bytes(offset, length)
    }

    /// Returns the most recent `max_lines` lines as standard `String`s.
    pub fn read_lines(&self, max_lines: Option<usize>) -> Vec<String> {
        self.ring.read().read_lines(max_lines)
    }

    /// Returns the most recent `max_lines` lines as zero-copy `ByteString` slices.
    pub fn read_line_slices(&self, max_lines: Option<usize>) -> Vec<ByteString> {
        self.ring.read().read_line_slices(max_lines)
    }

    /// Clears the ring buffer, any pending broadcast line, and resets cumulative bytes.
    pub fn clear(&self) {
        self.ring.write().clear();
        self.pending_line.lock().clear();
    }

    /// Subscribes to real-time incoming lines.
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.broadcast_tx.subscribe()
    }

    /// Returns the total bytes currently retained in the ring buffer.
    pub fn byte_size(&self) -> usize {
        self.ring.read().byte_size()
    }

    /// Returns the total line count currently retained in the ring buffer.
    pub fn line_count(&self) -> usize {
        self.ring.read().line_count()
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
        // max 20 bytes per segment, 2 backups = 60 bytes total capacity
        let rotator = InMemoryChannelRotator::new(20, 2);

        rotator.append_line("12345"); // ~6 bytes
        rotator.append_line("67890"); // ~6 bytes
        rotator.append_line("abcde"); // ~6 bytes -> total ~18 bytes

        assert_eq!(rotator.line_count(), 3);

        rotator.append_line("fghij");
        rotator.append_line("klmno");
        rotator.append_line("pqrst");
        rotator.append_line("uvwxy");

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
        // max 20 bytes per segment, 3 backups = 80 bytes total capacity
        let rotator = InMemoryChannelRotator::new(20, 3);

        // Append a 50-byte chunk in one call
        let large_payload = b"0123456789abcdefghij0123456789abcdefghij0123456789";
        assert_eq!(large_payload.len(), 50);

        rotator.append_bytes(large_payload);

        // Total retained bytes must equal full 50 bytes
        assert_eq!(rotator.byte_size(), 50);

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
        assert_eq!(clamped.segment_size(), 1024);
    }
}
