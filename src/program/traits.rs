// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::error::ProgramError;
use crate::program::config::StopSignal;
use crate::program::state::ProgramStatus;
use async_trait::async_trait;
use std::time::Duration;

/// Trait defining the lifecycle management of a supervised program.
#[async_trait]
pub trait Program: Send + Sync {
    /// Unique program name.
    fn name(&self) -> &str;

    /// Priority within range [0, 999]; lower numbers indicate higher startup priority.
    fn priority(&self) -> u32;

    /// List of program names this program strongly depends on.
    fn dependencies(&self) -> &[String];

    /// Retrieves current state snapshot instantly without channel or lock contention.
    fn status(&self) -> ProgramStatus;

    /// Asynchronously triggers program startup.
    async fn start(&self) -> Result<(), ProgramError>;

    /// Asynchronously triggers graceful shutdown within grace_period.
    async fn stop(&self, grace_period: Duration) -> Result<(), ProgramError>;

    /// Asynchronously restarts the program.
    async fn restart(&self, grace_period: Duration) -> Result<(), ProgramError>;

    /// Cooperatively shuts down and waits for the internal actor task to terminate cleanly.
    async fn shutdown(&mut self) -> Result<(), ProgramError>;

    /// Retrieves up to `max_lines` buffered log lines. If `max_lines` is None, returns all stored lines.
    fn read_logs(&self, max_lines: Option<usize>) -> Vec<String>;

    /// Subscribes to real-time incoming log events.
    fn subscribe_logs(&self) -> tokio::sync::broadcast::Receiver<String>;

    /// Asynchronously sends a signal to the running program process tree.
    async fn signal(&self, signal: StopSignal) -> Result<(), ProgramError>;

    /// Asynchronously sends input data to the process's standard input.
    async fn send_stdin(&self, data: Vec<u8>) -> Result<(), ProgramError>;
}
