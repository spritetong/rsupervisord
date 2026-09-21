// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use thiserror::Error;

/// Domain errors associated with individual program lifecycle operations.
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

    #[error("Writing to stdin of program '{name}' timed out after {timeout_secs}s")]
    StdinWriteTimeout { name: String, timeout_secs: u64 },

    #[error("Failed to write to stdin of program '{name}': {error}")]
    StdinWriteFailed { name: String, error: String },
}

impl ProgramError {
    /// Returns true if the error represents a program not found.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound { .. })
    }

    /// Returns true if the program is already running.
    pub fn is_already_running(&self) -> bool {
        matches!(self, Self::AlreadyRunning { .. })
    }

    /// Returns true if the program is not currently running.
    pub fn is_not_running(&self) -> bool {
        matches!(self, Self::NotRunning { .. })
    }

    /// Returns true if the error was due to a timeout.
    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout { .. } | Self::StdinWriteTimeout { .. })
    }

    /// Returns the name of the program associated with this error, if applicable.
    pub fn program_name(&self) -> Option<&str> {
        match self {
            Self::StartFailed { name, .. }
            | Self::StopFailed { name, .. }
            | Self::AlreadyRunning { name, .. }
            | Self::NotRunning { name }
            | Self::InvalidState { name, .. }
            | Self::ChannelClosed { name }
            | Self::Timeout { name, .. }
            | Self::NotFound { name }
            | Self::ShuttingDown { name }
            | Self::PreStartHookFailed { name, .. }
            | Self::InvalidCronExpression { name, .. }
            | Self::StdinWriteTimeout { name, .. }
            | Self::StdinWriteFailed { name, .. } => Some(name),
            Self::PlatformError(_) | Self::ConfigError(_) => None,
        }
    }
}

/// Domain errors associated with supervisor topology, orchestration, and daemon management.
#[derive(Error, Debug)]
pub enum SupervisorError {
    #[error("Program error: {0}")]
    Program(#[from] ProgramError),

    #[error("Configuration error: {0}")]
    Config(#[from] ConfigError),

    #[error("Cyclic dependency detected: {cycle:?}")]
    DependencyCycle { cycle: Vec<String> },

    #[error("Program '{program}' depends on unknown program '{depends_on}'")]
    DependencyNotFound { program: String, depends_on: String },

    #[error("Group '{group}' not found")]
    GroupNotFound { group: String },

    #[error("Supervisor manager has stopped")]
    ManagerStopped,

    #[error("Internal supervisor error: {message}")]
    Internal { message: String },
}

/// Domain errors associated with configuration parsing, validation, and expansion.
#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("IO error reading configuration: {0}")]
    Io(#[from] std::io::Error),

    #[error("YAML parse error: {0}")]
    ParseYaml(#[from] serde_yaml::Error),

    #[error("JSON parse error: {0}")]
    ParseJson(#[from] serde_json::Error),

    #[error("Configuration validation error: {message}")]
    Validation { message: String },

    #[error("Duplicate program name '{name}'")]
    DuplicateProgram { name: String },

    #[error("Duplicate group name '{name}'")]
    DuplicateGroup { name: String },

    #[error("Environment macro expansion error for '{var}': {reason}")]
    EnvironmentExpansion { var: String, reason: String },

    #[error("Invalid value for field '{field}': {message}")]
    InvalidValue { field: String, message: String },

    #[error("Invalid byte size format '{input}': {reason}")]
    InvalidByteSize { input: String, reason: String },

    #[error("Invalid cron expression '{expression}': {reason}")]
    InvalidCronExpression { expression: String, reason: String },

    #[error("{0}")]
    Custom(String),
}

impl From<ConfigError> for ProgramError {
    fn from(err: ConfigError) -> Self {
        ProgramError::ConfigError(err.to_string())
    }
}

/// Domain errors associated with OS system service installation and management.
#[derive(Error, Debug)]
pub enum ServiceError {
    #[error("Service management not supported on platform: {platform}")]
    PlatformNotSupported { platform: String },

    #[error("Service installation failed: {message}")]
    InstallationFailed { message: String },

    #[error("Service uninstallation failed: {message}")]
    UninstallationFailed { message: String },

    #[error("Service control '{action}' failed: {message}")]
    ControlFailed { action: String, message: String },

    #[error("Service execution context has not been initialized")]
    ContextNotInitialized,

    #[error("Service execution context has already been initialized")]
    AlreadyInitialized,

    #[error("Service IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Custom(String),
}

/// Domain errors associated with the CLI client, transport, and remote protocol.
#[derive(Error, Debug)]
pub enum CliError {
    #[error("Transport error: {0}")]
    Transport(String),

    #[error("Daemon server returned error: {0}")]
    Server(String),

    #[error("Program or entity not found: {0}")]
    NotFound(String),

    #[error("Invalid server response: {0}")]
    InvalidResponse(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Custom(String),
}
