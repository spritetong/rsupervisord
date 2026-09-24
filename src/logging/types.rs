// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use bytes::Bytes;
use std::fmt;
use std::time::SystemTime;

/// Standard stream channel for a supervised process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum LogChannel {
    Stdout,
    Stderr,
}

impl fmt::Display for LogChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stdout => write!(f, "stdout"),
            Self::Stderr => write!(f, "stderr"),
        }
    }
}

/// An immutable chunk of log data produced by a process stream.
#[derive(Debug, Clone)]
pub struct LogChunk {
    pub channel: LogChannel,
    pub timestamp: SystemTime,
    pub program: String,
    pub pid: Option<u32>,
    pub data: Bytes,
}

impl LogChunk {
    /// Creates a new log chunk with the current system timestamp.
    pub fn new(
        channel: LogChannel,
        program: impl Into<String>,
        pid: Option<u32>,
        data: impl Into<Bytes>,
    ) -> Self {
        Self {
            channel,
            timestamp: SystemTime::now(),
            program: program.into(),
            pid,
            data: data.into(),
        }
    }

    /// Returns the length in bytes of the log data payload.
    #[inline]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Returns true if the payload is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}
