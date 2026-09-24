// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::error::ProgramError;
use crate::logging::types::LogChannel;
use std::process::Stdio;
use tokio::io::{AsyncRead, AsyncWrite};

/// OS standard I/O handles configured for a child process.
pub struct ProcessStdioHandles {
    pub stdout: Option<Stdio>,
    pub stderr: Option<Stdio>,
}

/// Asynchronous reading streams captured from a child process transport.
pub struct TransportStreams {
    pub stdout: Option<Box<dyn AsyncRead + Send + Unpin>>,
    pub stderr: Option<Box<dyn AsyncRead + Send + Unpin>>,
}

/// Abstract transport bridge between an OS subprocess and the supervisor logging engine.
pub trait LogTransport: Send + Sync + 'static {
    /// Takes the child process stdio handles. Can only be successfully called once.
    fn take_child_stdio(&mut self) -> Result<ProcessStdioHandles, ProgramError>;

    /// Consumes the transport and returns the asynchronous reading streams for controller consumption.
    fn into_streams(self: Box<Self>) -> Result<TransportStreams, ProgramError>;

    /// Optional direct asynchronous writer for tests, mocked streams, or in-memory injection.
    fn direct_writer(&self, _channel: LogChannel) -> Option<Box<dyn AsyncWrite + Send + Unpin>> {
        None
    }
}
