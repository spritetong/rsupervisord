// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ProgramError {
    #[error("Program '{name}' failed to start: {source}")]
    StartFailed {
        name: String,
        #[source]
        source: std::io::Error,
    },

    #[error("Program '{name}' failed to stop gracefully: {source}")]
    StopFailed {
        name: String,
        #[source]
        source: std::io::Error,
    },

    #[error("Program '{name}' is already running with PID {pid}")]
    AlreadyRunning { name: String, pid: u32 },

    #[error("Program '{name}' is not running")]
    NotRunning { name: String },

    #[error("Program '{name}' is in an invalid state for this operation: {state:?}")]
    InvalidState {
        name: String,
        state: crate::program::ProgramState,
    },

    #[error("Command channel closed unexpectedly for program '{name}'")]
    ChannelClosed { name: String },

    #[error("Operation on program '{name}' timed out after {timeout_secs}s")]
    Timeout { name: String, timeout_secs: u64 },

    #[error("Program '{name}' is not registered")]
    NotFound { name: String },

    #[error("Platform-specific error: {0}")]
    PlatformError(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Entity '{name}' is currently shutting down")]
    ShuttingDown { name: String },

    #[error("Program '{name}' pre_start hook failed: {reason}")]
    PreStartHookFailed { name: String, reason: String },

    #[error("Invalid cron expression for program '{name}': '{expression}' ({reason})")]
    InvalidCronExpression {
        name: String,
        expression: String,
        reason: String,
    },
}
