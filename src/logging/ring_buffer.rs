// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::logging::continuous_ring::ContinuousRingBuffer;
use bytestring::ByteString;
use parking_lot::Mutex;
use tokio::sync::broadcast;

/// In-memory ring buffer with fixed capacity and live broadcast capability.
///
/// Backed by [`ContinuousRingBuffer`] for zero-allocation contiguous storage
/// and zero-copy [`ByteString`] slice retrieval.
pub struct RingBuffer {
    capacity_lines: usize,
    inner: Mutex<ContinuousRingBuffer>,
    broadcast_tx: broadcast::Sender<String>,
}

impl RingBuffer {
    /// Creates a new RingBuffer with the specified line capacity.
    /// Allocates an underlying contiguous byte buffer proportional to line capacity.
    pub fn new(capacity_lines: usize) -> Self {
        let lines = capacity_lines.max(1);
        let byte_cap = (lines * 512).max(64 * 1024);
        let (broadcast_tx, _) = broadcast::channel(1024);
        Self {
            capacity_lines: lines,
            inner: Mutex::new(ContinuousRingBuffer::with_max_lines(byte_cap, lines)),
            broadcast_tx,
        }
    }

    /// Creates a new RingBuffer with a fixed byte capacity and optional line limit.
    pub fn with_byte_capacity(capacity_bytes: usize, max_lines: Option<usize>) -> Self {
        let cap = capacity_bytes.max(1024);
        let lines = max_lines.unwrap_or(2000);
        let (broadcast_tx, _) = broadcast::channel(1024);
        let inner = match max_lines {
            Some(m) => ContinuousRingBuffer::with_max_lines(cap, m),
            None => ContinuousRingBuffer::new(cap),
        };
        Self {
            capacity_lines: lines,
            inner: Mutex::new(inner),
            broadcast_tx,
        }
    }

    /// Appends a new line to the ring buffer and broadcasts it to live subscribers.
    /// Incurs zero cloning overhead when no live broadcast receivers are active.
    pub fn push(&self, line: impl AsRef<str>) {
        let line_ref = line.as_ref();
        if self.broadcast_tx.receiver_count() > 0 {
            let _ = self.broadcast_tx.send(line_ref.to_string());
        }
        self.inner.lock().write_line(line_ref);
    }

    /// Appends raw byte content as a line, automatically ensuring line termination.
    /// Copies bytes directly into the contiguous buffer with zero string allocation.
    pub fn push_bytes(&self, data: &[u8]) {
        if self.broadcast_tx.receiver_count() > 0 {
            let line = String::from_utf8_lossy(data);
            let _ = self.broadcast_tx.send(line.into_owned());
        }
        let mut guard = self.inner.lock();
        guard.write_bytes(data);
        if !data.ends_with(b"\n") {
            guard.write_bytes(b"\n");
        }
    }

    /// Appends raw bytes with a stream/channel prefix, ensuring line termination.
    pub fn push_prefixed(&self, prefix: &str, data: &[u8]) {
        if self.broadcast_tx.receiver_count() > 0 {
            let line = String::from_utf8_lossy(data);
            let _ = self.broadcast_tx.send(format!("{}: {}", prefix, line));
        }
        let mut guard = self.inner.lock();
        guard.write_bytes(prefix.as_bytes());
        guard.write_bytes(b": ");
        guard.write_bytes(data);
        if !data.ends_with(b"\n") {
            guard.write_bytes(b"\n");
        }
    }

    /// Retrieves up to `max_lines` most recent log lines as `String`s.
    pub fn get_lines(&self, max_lines: Option<usize>) -> Vec<String> {
        self.inner.lock().read_lines(max_lines)
    }

    /// Retrieves up to `max_lines` most recent log lines as zero-copy `ByteString` slices.
    pub fn get_line_slices(&self, max_lines: Option<usize>) -> Vec<ByteString> {
        self.inner.lock().read_line_slices(max_lines)
    }

    /// Retrieves a complete snapshot of all lines currently residing in the buffer.
    #[inline]
    pub fn snapshot(&self) -> Vec<String> {
        self.get_lines(None)
    }

    /// Clears all lines currently stored in the ring buffer.
    pub fn clear(&self) {
        self.inner.lock().clear();
    }

    /// Subscribes to real-time incoming log lines.
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.broadcast_tx.subscribe()
    }

    /// Returns the current number of active streaming subscribers.
    #[inline]
    pub fn subscriber_count(&self) -> usize {
        self.broadcast_tx.receiver_count()
    }

    /// Returns the current number of lines stored in the buffer.
    pub fn len(&self) -> usize {
        self.inner.lock().line_count()
    }

    /// Returns true if the buffer contains no lines.
    pub fn is_empty(&self) -> bool {
        self.inner.lock().is_empty()
    }

    /// Returns the maximum line capacity of the buffer.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity_lines
    }
}

impl Default for RingBuffer {
    fn default() -> Self {
        Self::new(2000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ring_buffer_capacity_eviction() {
        let buffer = RingBuffer::new(3);
        assert_eq!(buffer.capacity(), 3);
        assert!(buffer.is_empty());

        buffer.push("line 1");
        buffer.push("line 2");
        buffer.push("line 3");
        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer.get_lines(None), vec!["line 1", "line 2", "line 3"]);
        assert_eq!(buffer.snapshot(), vec!["line 1", "line 2", "line 3"]);

        // Push 4th line, line 1 should be evicted
        buffer.push("line 4");
        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer.get_lines(None), vec!["line 2", "line 3", "line 4"]);

        // Partial fetch
        assert_eq!(buffer.get_lines(Some(2)), vec!["line 3", "line 4"]);
        assert_eq!(
            buffer.get_lines(Some(5)),
            vec!["line 2", "line 3", "line 4"]
        );

        buffer.clear();
        assert!(buffer.is_empty());
        assert_eq!(buffer.len(), 0);
    }

    #[tokio::test]
    async fn test_ring_buffer_broadcast() {
        let buffer = RingBuffer::new(10);
        let mut rx1 = buffer.subscribe();
        let mut rx2 = buffer.subscribe();

        assert_eq!(buffer.subscriber_count(), 2);

        buffer.push("hello world");

        assert_eq!(rx1.recv().await.unwrap(), "hello world");
        assert_eq!(rx2.recv().await.unwrap(), "hello world");
    }
}
