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

    #[error("Platform-specific error: {0}")]
    PlatformError(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),
}
