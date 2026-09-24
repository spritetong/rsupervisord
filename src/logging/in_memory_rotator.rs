// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::consts::MAX_LIVE_LOG_LINE_BYTES;
use crate::error::ProgramError;
use crate::logging::backend::LogBackend;
use crate::logging::reader::InstantLogReader;
use crate::logging::types::{LogChannel, LogChunk};
use async_trait::async_trait;
use bytes::BytesMut;
use parking_lot::{Mutex, RwLock};
use std::collections::VecDeque;
use std::sync::Arc;
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
}

impl InMemoryChannelRotator {
    /// Creates a new in-memory channel rotator with rotation threshold and retained backups.
    pub fn new(max_bytes: usize, backups: usize) -> Self {
        let (broadcast_tx, _) = broadcast::channel(1024);
        Self {
            max_bytes: max_bytes.max(1),
            backups,
            active: RwLock::new(MemorySegment::with_capacity(max_bytes.min(64 * 1024))),
            backups_queue: RwLock::new(VecDeque::with_capacity(backups)),
            broadcast_tx,
            pending_line: Mutex::new(BytesMut::new()),
        }
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

    /// Appends a raw chunk of bytes, rotating the segment if `max_bytes` is reached.
    pub fn append_bytes(&self, data: &[u8]) {
        if data.is_empty() {
            return;
        }

        self.broadcast_chunk(data);

        // Lock order: active before backups_queue.
        let mut active = self.active.write();

        // Check if current active segment needs rotation
        if active.len() + data.len() > self.max_bytes && !active.is_empty() {
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

        let prev_len = active.data.len();
        active.data.extend_from_slice(data);

        // Record newline byte offsets
        for (i, &b) in data.iter().enumerate() {
            if b == b'\n' {
                active.line_offsets.push(prev_len + i + 1);
            }
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

    /// Reads bytes starting from offset with length, returning `(content, total_size, overflow)`.
    pub fn read_bytes(&self, offset: i64, length: i64) -> (String, i64, bool) {
        let full = self.snapshot_bytes();
        let sz = full.len() as i64;

        let mut overflow = false;
        let mut off = offset;
        let mut len = length;

        if off < 0 {
            off += sz;
            if off < 0 {
                overflow = true;
                off = 0;
            }
        }

        if len < 0 {
            len = 0;
        }

        let start = off.min(sz) as usize;
        let end = (off + len).min(sz) as usize;

        let slice = if start < end { &full[start..end] } else { &[] };

        (String::from_utf8_lossy(slice).to_string(), sz, overflow)
    }

    /// Tails bytes backwards from buffer end, matching supervisor XML-RPC semantics.
    pub fn tail_bytes(&self, offset: i64, length: i64) -> (String, i64, bool) {
        let full = self.snapshot_bytes();
        let sz = full.len() as i64;

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

        let slice = if len == 0 || off >= sz {
            &[]
        } else {
            let end = (off + len).min(sz) as usize;
            &full[off as usize..end]
        };

        (String::from_utf8_lossy(slice).to_string(), sz, overflow)
    }

    /// Returns the most recent `max_lines` lines.
    pub fn read_lines(&self, max_lines: Option<usize>) -> Vec<String> {
        let bytes = self.snapshot_bytes();
        let text = String::from_utf8_lossy(&bytes);
        let all_lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();

        match max_lines {
            Some(n) if n < all_lines.len() => all_lines[all_lines.len() - n..].to_vec(),
            _ => all_lines,
        }
    }

    /// Clears both active and backup segments and any pending broadcast line.
    pub fn clear(&self) {
        {
            // Lock order: active before backups_queue.
            let mut active = self.active.write();
            let mut backups = self.backups_queue.write();
            active.data.clear();
            active.line_offsets.clear();
            backups.clear();
        }
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
}

impl Default for InMemoryLogRotator {
    fn default() -> Self {
        Self::new(50 * 1024 * 1024, 10)
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
}
