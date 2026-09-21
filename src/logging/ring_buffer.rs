// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use parking_lot::Mutex;
use std::collections::VecDeque;
use tokio::sync::broadcast;

/// In-memory ring buffer with fixed capacity and live broadcast capability.
pub struct RingBuffer {
    capacity: usize,
    lines: Mutex<VecDeque<String>>,
    broadcast_tx: broadcast::Sender<String>,
}

impl RingBuffer {
    /// Creates a new RingBuffer with the specified line capacity.
    pub fn new(capacity: usize) -> Self {
        let actual_capacity = capacity.max(1);
        let (broadcast_tx, _) = broadcast::channel(1024);
        Self {
            capacity: actual_capacity,
            lines: Mutex::new(VecDeque::with_capacity(actual_capacity)),
            broadcast_tx,
        }
    }

    /// Appends a new line to the ring buffer and broadcasts it to live subscribers.
    /// Incurs zero cloning overhead when no live broadcast receivers are active.
    pub fn push(&self, line: impl Into<String>) {
        let line_str = line.into();
        let should_broadcast = self.broadcast_tx.receiver_count() > 0;
        let to_broadcast = if should_broadcast {
            Some(line_str.clone())
        } else {
            None
        };

        {
            let mut guard = self.lines.lock();
            if guard.len() >= self.capacity {
                guard.pop_front();
            }
            guard.push_back(line_str);
        }

        if let Some(msg) = to_broadcast {
            let _ = self.broadcast_tx.send(msg);
        }
    }

    /// Retrieves up to `max_lines` most recent log lines from the ring buffer.
    /// If `max_lines` is None, all stored lines are returned.
    pub fn get_lines(&self, max_lines: Option<usize>) -> Vec<String> {
        let guard = self.lines.lock();
        match max_lines {
            Some(n) if n < guard.len() => guard.iter().skip(guard.len() - n).cloned().collect(),
            _ => guard.iter().cloned().collect(),
        }
    }

    /// Retrieves a complete snapshot of all lines currently residing in the buffer.
    #[inline]
    pub fn snapshot(&self) -> Vec<String> {
        self.get_lines(None)
    }

    /// Clears all lines currently stored in the ring buffer.
    pub fn clear(&self) {
        let mut guard = self.lines.lock();
        guard.clear();
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
        self.lines.lock().len()
    }

    /// Returns true if the buffer contains no lines.
    pub fn is_empty(&self) -> bool {
        self.lines.lock().is_empty()
    }

    /// Returns the maximum capacity of the buffer.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity
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
