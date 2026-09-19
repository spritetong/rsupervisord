use crate::program::state::ProgramState;
use serde::{Deserialize, Serialize};
use tabled::Tabled;

/// Universal REST API JSON response envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

impl<T> ApiResponse<T> {
    pub fn ok(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn err(message: impl Into<String>) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(message.into()),
        }
    }
}

/// Data transfer object representing a supervised program's status.
#[derive(Debug, Clone, Serialize, Deserialize, Tabled)]
pub struct ProgramStatusDto {
    #[tabled(rename = "PROGRAM")]
    pub name: String,

    #[tabled(rename = "STATUS")]
    pub state: String,

    #[tabled(rename = "PID")]
    pub pid: String,

    #[tabled(rename = "PRIORITY")]
    pub priority: u8,

    #[tabled(rename = "DESCRIPTION")]
    pub description: String,
}

/// Request parameters for lifecycle operations (start, stop, restart).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionRequest {
    #[serde(default = "default_true")]
    pub sync: bool,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_true() -> bool {
    true
}

fn default_timeout() -> u64 {
    30
}

impl Default for ActionRequest {
    fn default() -> Self {
        Self {
            sync: true,
            timeout_secs: 30,
        }
    }
}

/// Result returned from a program lifecycle action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResponse {
    pub name: String,
    pub state: ProgramState,
    pub pid: Option<u32>,
    pub description: String,
    pub elapsed_ms: u64,
}

/// Result summary returned from a hot configuration reload.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReloadResponse {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub modified: Vec<String>,
    pub unchanged: Vec<String>,
}

/// Response containing buffered historical log lines.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogLinesResponse {
    pub name: String,
    pub lines: Vec<String>,
}
