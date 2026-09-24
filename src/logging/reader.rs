// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::logging::types::LogChannel;
use tokio::sync::broadcast;

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
