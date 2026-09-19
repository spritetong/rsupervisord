use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProgramState {
    Stopped,
    Starting,
    Running,
    Backoff,
    Stopping,
    Exited,
    Fatal,
}

impl ProgramState {
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            ProgramState::Starting | ProgramState::Running | ProgramState::Stopping
        )
    }

    pub fn is_stopped_or_fatal(&self) -> bool {
        matches!(
            self,
            ProgramState::Stopped | ProgramState::Exited | ProgramState::Fatal
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProgramStatus {
    pub name: String,
    pub state: ProgramState,
    pub pid: Option<u32>,
    pub uptime_secs: Option<u64>,
    pub exit_code: Option<i32>,
    pub is_healthy: bool,
    pub description: String,
}

impl ProgramStatus {
    pub fn new_stopped(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            state: ProgramState::Stopped,
            pid: None,
            uptime_secs: None,
            exit_code: None,
            is_healthy: false,
            description: "Stopped".to_string(),
        }
    }
}
