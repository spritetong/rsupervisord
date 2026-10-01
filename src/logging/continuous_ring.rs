// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use bytes::BytesMut;
use bytestring::ByteString;
use std::collections::VecDeque;

/// A contiguous, zero-allocation in-memory circular byte ring buffer with monotonic line indexing.
///
/// Log data is written directly into a fixed-capacity byte slice. Complete line intervals
/// are tracked in a `VecDeque<(u64, u64)>` storing monotonic byte offsets `(start, end)`.
///
/// As the buffer wraps around, `log_start_offset` advances and expired line indices
/// are popped in O(1) time without any memory reallocations. Reads extract the requested
/// byte window in at most two slice copies into a `ByteString`, producing zero-copy
/// sub-slices for individual lines.
#[derive(Debug)]
pub struct ContinuousRingBuffer {
    /// Fixed-size contiguous byte storage.
    buffer: Box<[u8]>,
    /// Capacity of the buffer in bytes.
    capacity: usize,
    /// Total cumulative bytes written since inception/seeding (monotonic write offset).
    cumulative_bytes: u64,
    /// Monotonic byte offset of the oldest byte currently retained in the buffer.
    log_start_offset: u64,
    /// Monotonic indices of complete lines: `(start_offset, end_offset)`.
    line_indices: VecDeque<(u64, u64)>,
    /// Monotonic offset marking the start of the line currently being written.
    current_line_start: u64,
    /// Optional limit on the maximum number of lines retained in the index.
    max_line_index: Option<usize>,
}

impl ContinuousRingBuffer {
    /// Creates a new continuous ring buffer with the specified byte capacity.
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.max(1);
        Self {
            buffer: vec![0u8; cap].into_boxed_slice(),
            capacity: cap,
            cumulative_bytes: 0,
            log_start_offset: 0,
            line_indices: VecDeque::new(),
            current_line_start: 0,
            max_line_index: None,
        }
    }

    /// Creates a new continuous ring buffer with both byte capacity and line retention limits.
    pub fn with_max_lines(capacity: usize, max_lines: usize) -> Self {
        let mut rb = Self::new(capacity);
        rb.max_line_index = Some(max_lines.max(1));
        rb
    }

    /// Returns the maximum byte capacity of the ring buffer.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns the current number of valid bytes stored in the buffer.
    #[inline]
    pub fn byte_size(&self) -> usize {
        (self.cumulative_bytes.saturating_sub(self.log_start_offset)) as usize
    }

    /// Returns the total cumulative bytes written to this buffer since inception/seeding.
    #[inline]
    pub fn cumulative_bytes(&self) -> u64 {
        self.cumulative_bytes
    }

    /// Returns the monotonic byte offset of the oldest byte retained in this buffer.
    #[inline]
    pub fn log_start_offset(&self) -> u64 {
        self.log_start_offset
    }

    /// Returns the number of readable lines (complete lines plus optional incomplete tail).
    #[inline]
    pub fn line_count(&self) -> usize {
        self.line_indices.len()
            + if self.current_line_start < self.cumulative_bytes {
                1
            } else {
                0
            }
    }

    /// Returns true if the buffer contains no readable data.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.cumulative_bytes == self.log_start_offset
    }

    /// Appends a raw byte slice into the ring buffer.
    ///
    /// Copies bytes directly into the circular buffer with zero heap allocations.
    /// Updates line indices and advances `log_start_offset`, evicting overwritten lines.
    pub fn write_bytes(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }

        let data_len = data.len();
        let start_w = self.cumulative_bytes;

        // 1. Scan and register line indices for newlines
        for (i, &b) in data.iter().enumerate() {
            if b == b'\n' {
                let line_end = start_w + i as u64 + 1;
                self.line_indices
                    .push_back((self.current_line_start, line_end));
                self.current_line_start = line_end;
            }
        }

        // 2. Write data into the circular byte array
        if data_len >= self.capacity {
            // Data exceeds capacity: only the trailing `capacity` bytes remain in the ring
            let to_copy = &data[data_len - self.capacity..];
            let start_pos =
                ((start_w + (data_len - self.capacity) as u64) % self.capacity as u64) as usize;
            let first = self.capacity - start_pos;
            self.buffer[start_pos..].copy_from_slice(&to_copy[..first]);
            self.buffer[..start_pos].copy_from_slice(&to_copy[first..]);
        } else {
            let phys_start = (start_w % self.capacity as u64) as usize;
            if phys_start + data_len <= self.capacity {
                self.buffer[phys_start..phys_start + data_len].copy_from_slice(data);
            } else {
                let first = self.capacity - phys_start;
                self.buffer[phys_start..].copy_from_slice(&data[..first]);
                self.buffer[..data_len - first].copy_from_slice(&data[first..]);
            }
        }

        // 3. Advance cumulative write offset and oldest available byte offset
        self.cumulative_bytes += data_len as u64;
        if self.cumulative_bytes > self.capacity as u64 {
            self.log_start_offset = self.cumulative_bytes - self.capacity as u64;
        } else {
            self.log_start_offset = 0;
        }

        // 4. Pop expired line indices whose start has been overwritten by the ring wrap
        while let Some(&(line_start, _)) = self.line_indices.front() {
            if line_start < self.log_start_offset {
                self.line_indices.pop_front();
            } else {
                break;
            }
        }

        if self.current_line_start < self.log_start_offset {
            self.current_line_start = self.log_start_offset;
        }

        // 5. Enforce optional max line retention count
        if let Some(max_l) = self.max_line_index {
            while self.line_indices.len() > max_l {
                self.line_indices.pop_front();
            }
        }
    }

    /// Appends a text line, automatically ensuring a newline delimiter.
    pub fn write_line(&mut self, line: &str) {
        let bytes = line.as_bytes();
        if bytes.ends_with(b"\n") {
            self.write_bytes(bytes);
        } else {
            self.write_bytes(bytes);
            self.write_bytes(b"\n");
        }
    }

    /// Copies a monotonic byte range `[start, end)` from the ring buffer into `out`.
    /// Handles circular wrap-around with at most two `extend_from_slice` calls.
    pub fn copy_range(&self, start: u64, end: u64, out: &mut BytesMut) {
        let eff_start = start.max(self.log_start_offset);
        let eff_end = end.min(self.cumulative_bytes);
        if eff_start >= eff_end {
            return;
        }

        let len = (eff_end - eff_start) as usize;
        let phys_start = (eff_start % self.capacity as u64) as usize;
        let phys_end = (eff_end % self.capacity as u64) as usize;

        if phys_start < phys_end {
            out.extend_from_slice(&self.buffer[phys_start..phys_end]);
        } else if phys_start == phys_end && len == self.capacity {
            out.extend_from_slice(&self.buffer[phys_start..]);
            out.extend_from_slice(&self.buffer[..phys_start]);
        } else {
            out.extend_from_slice(&self.buffer[phys_start..]);
            out.extend_from_slice(&self.buffer[..phys_end]);
        }
    }

    /// Seeds this buffer from trailing bytes of an existing disk file, preserving monotonic total size.
    pub fn seed(&mut self, bytes: &[u8], total_file_size: u64) {
        self.clear();
        if total_file_size > 0 && !bytes.is_empty() {
            let seed_start = total_file_size.saturating_sub(bytes.len() as u64);
            self.cumulative_bytes = seed_start;
            self.current_line_start = seed_start;
            self.write_bytes(bytes);
            self.cumulative_bytes = total_file_size;
        } else {
            self.cumulative_bytes = total_file_size;
            self.log_start_offset = total_file_size;
            self.current_line_start = total_file_size;
        }
    }

    /// Reads up to `max_lines` most recent log lines as zero-copy `ByteString` slices.
    ///
    /// Copies the requested range once from the ring buffer into an underlying `ByteString`,
    /// then produces `ByteString` slices for each line with zero additional string allocations.
    pub fn read_line_slices(&self, max_lines: Option<usize>) -> Vec<ByteString> {
        let has_trailing = self.current_line_start < self.cumulative_bytes;
        let total_lines = self.line_indices.len() + if has_trailing { 1 } else { 0 };
        if total_lines == 0 {
            return Vec::new();
        }

        let take = max_lines.unwrap_or(total_lines).min(total_lines);
        let skip = total_lines.saturating_sub(take);

        let mut selected: Vec<(u64, u64)> = Vec::with_capacity(take);
        let complete_skip = skip.min(self.line_indices.len());
        for &(s, e) in self.line_indices.iter().skip(complete_skip) {
            if selected.len() < take {
                selected.push((s, e));
            }
        }
        if has_trailing && selected.len() < take {
            selected.push((self.current_line_start, self.cumulative_bytes));
        }

        if selected.is_empty() {
            return Vec::new();
        }

        let first_start = selected[0].0;
        let last_end = selected.last().unwrap().1;
        let total_len = (last_end.saturating_sub(first_start)) as usize;
        if total_len == 0 {
            return Vec::new();
        }

        let mut buf = BytesMut::with_capacity(total_len);
        self.copy_range(first_start, last_end, &mut buf);

        let frozen = buf.freeze();
        let byte_str = match std::str::from_utf8(&frozen) {
            Ok(_) => unsafe { ByteString::from_bytes_unchecked(frozen) },
            Err(_) => {
                // Fallback for non-UTF8 input: re-split lines lossy
                let lossy = String::from_utf8_lossy(&frozen);
                return lossy
                    .lines()
                    .map(|l| ByteString::from(l.to_string()))
                    .collect();
            }
        };

        let mut result = Vec::with_capacity(selected.len());
        for &(s, e) in &selected {
            let rel_s = (s.saturating_sub(first_start)) as usize;
            let mut rel_e = (e.saturating_sub(first_start)) as usize;
            rel_e = rel_e.min(byte_str.len());
            if rel_s > rel_e {
                continue;
            }

            let slice_bytes = &byte_str.as_bytes()[rel_s..rel_e];
            if slice_bytes.ends_with(b"\n") {
                rel_e -= 1;
                if rel_e > rel_s && byte_str.as_bytes()[rel_e - 1] == b'\r' {
                    rel_e -= 1;
                }
            }

            let sub = &byte_str[rel_s..rel_e];
            result.push(byte_str.slice_ref(sub));
        }

        result
    }

    /// Reads up to `max_lines` lines as standard `String`s.
    pub fn read_lines(&self, max_lines: Option<usize>) -> Vec<String> {
        self.read_line_slices(max_lines)
            .into_iter()
            .map(|bs| bs.to_string())
            .collect()
    }

    /// Reads bytes starting from monotonic offset with length, returning `(content, total_size, overflow)`.
    /// Fully compliant with Supervisor XML-RPC `readLog` semantics.
    pub fn read_bytes(&self, offset: i64, length: i64) -> (String, i64, bool) {
        let total_sz = self.cumulative_bytes as i64;
        let retained_sz = (self.cumulative_bytes.saturating_sub(self.log_start_offset)) as usize;
        let oldest_available_offset = self.log_start_offset as i64;

        let mut overflow = false;
        let len = length.max(0);

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
        } else if offset < oldest_available_offset {
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
        };

        let mut buf = BytesMut::with_capacity(to_read);
        self.copy_range(
            start_off as u64,
            (start_off + to_read as i64) as u64,
            &mut buf,
        );
        let s = String::from_utf8_lossy(&buf).to_string();
        (s, total_sz, overflow)
    }

    /// Tails bytes backwards from buffer end or monotonic offset, matching Supervisor XML-RPC semantics.
    pub fn tail_bytes(&self, offset: i64, length: i64) -> (String, i64, bool) {
        let total_sz = self.cumulative_bytes as i64;
        let retained_sz = (self.cumulative_bytes.saturating_sub(self.log_start_offset)) as usize;
        let oldest_available_offset = self.log_start_offset as i64;

        let len = length.max(0);

        if offset == 0 {
            let actual_len = if len == 0 {
                retained_sz
            } else {
                (len as usize).min(retained_sz)
            };
            let start = total_sz
                .saturating_sub(actual_len as i64)
                .max(oldest_available_offset);
            let mut buf = BytesMut::with_capacity(actual_len);
            self.copy_range(start as u64, total_sz as u64, &mut buf);
            let has_overflow = total_sz > actual_len as i64;
            let s = String::from_utf8_lossy(&buf).to_string();
            return (s, total_sz, has_overflow);
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

        let to_read = if len == 0 {
            (total_sz - off) as usize
        } else {
            (len as usize).min((total_sz - off) as usize)
        };

        let mut buf = BytesMut::with_capacity(to_read);
        self.copy_range(off as u64, (off + to_read as i64) as u64, &mut buf);
        let new_offset = off + to_read as i64;
        let s = String::from_utf8_lossy(&buf).to_string();
        (s, new_offset, overflow)
    }

    /// Returns a flat snapshot of all currently retained bytes.
    pub fn snapshot_bytes(&self) -> Vec<u8> {
        let retained_sz = (self.cumulative_bytes.saturating_sub(self.log_start_offset)) as usize;
        let mut buf = BytesMut::with_capacity(retained_sz);
        self.copy_range(self.log_start_offset, self.cumulative_bytes, &mut buf);
        buf.to_vec()
    }

    /// Clears all retained bytes, line indices, and resets monotonic counters.
    pub fn clear(&mut self) {
        self.cumulative_bytes = 0;
        self.log_start_offset = 0;
        self.current_line_start = 0;
        self.line_indices.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_continuous_ring_buffer_basic() {
        let mut rb = ContinuousRingBuffer::new(50);
        rb.write_line("hello");
        rb.write_line("world");

        assert_eq!(rb.line_count(), 2);
        assert_eq!(rb.read_lines(None), vec!["hello", "world"]);
        let slices = rb.read_line_slices(None);
        assert_eq!(slices.len(), 2);
        assert_eq!(&slices[0][..], "hello");
        assert_eq!(&slices[1][..], "world");
    }

    #[test]
    fn test_continuous_ring_buffer_wrap_and_eviction() {
        // Buffer of 20 bytes
        let mut rb = ContinuousRingBuffer::new(20);
        rb.write_line("12345"); // 6 bytes: "12345\n"
        rb.write_line("67890"); // 6 bytes: "67890\n"
        rb.write_line("abcde"); // 6 bytes: "abcde\n" -> total 18 bytes

        assert_eq!(rb.line_count(), 3);
        assert_eq!(rb.log_start_offset(), 0);

        // Next line pushes cumulative to 24 bytes (> 20 capacity)
        rb.write_line("fghij"); // 6 bytes -> total 24 bytes, start_offset = 4
        // Line "12345\n" started at 0 (< 4) so it is evicted
        assert_eq!(rb.log_start_offset(), 4);
        assert_eq!(rb.read_lines(None), vec!["67890", "abcde", "fghij"]);
    }

    #[test]
    fn test_continuous_ring_buffer_xmlrpc_semantics() {
        let mut rb = ContinuousRingBuffer::new(100);
        rb.write_bytes(b"hello world\nsecond line\n");

        let (data, new_off, overflow) = rb.tail_bytes(0, 5);
        assert_eq!(data, "line\n");
        assert_eq!(new_off, 24);
        assert!(overflow);

        let (read_data, _, _) = rb.read_bytes(0, 5);
        assert_eq!(read_data, "hello");
    }
}
