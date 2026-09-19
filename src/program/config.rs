use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AutoRestartPolicy {
    Always,
    #[default]
    Unexpected,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "UPPERCASE")]
pub enum StopSignal {
    #[cfg_attr(unix, default)]
    Term,
    Int,
    Quit,
    Kill,
    #[cfg_attr(windows, default)]
    #[serde(rename = "CTRL_BREAK")]
    CtrlBreak,
    #[serde(rename = "CTRL_C")]
    CtrlC,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProgramLogsConfig {
    #[serde(default)]
    pub stdout: Option<PathBuf>,
    #[serde(default)]
    pub stderr: Option<PathBuf>,
    #[serde(default)]
    pub max_bytes: Option<String>,
    #[serde(default)]
    pub backups: Option<usize>,
    #[serde(default)]
    pub redirect_stderr: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub directory: Option<PathBuf>,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub environment: HashMap<String, String>,
    #[serde(default = "default_priority")]
    pub priority: u8,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default = "default_true")]
    pub autostart: bool,
    #[serde(default)]
    pub autorestart: AutoRestartPolicy,
    #[serde(default = "default_start_secs")]
    pub start_secs: u64,
    #[serde(default = "default_start_retries")]
    pub start_retries: u32,
    #[serde(default)]
    pub stop_signal: StopSignal,
    #[serde(default = "default_stop_wait_secs")]
    pub stop_wait_secs: u64,
    #[serde(default = "default_exit_codes")]
    pub exit_codes: Vec<i32>,
    #[serde(default)]
    pub umask: Option<u32>,
    #[serde(default)]
    pub logs: ProgramLogsConfig,
}

fn default_priority() -> u8 {
    50
}

fn default_true() -> bool {
    true
}

fn default_start_secs() -> u64 {
    3
}

fn default_start_retries() -> u32 {
    3
}

fn default_stop_wait_secs() -> u64 {
    10
}

fn default_exit_codes() -> Vec<i32> {
    vec![0]
}

impl ProgramConfig {
    pub fn new(name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            args: Vec::new(),
            directory: None,
            user: None,
            environment: HashMap::new(),
            priority: default_priority(),
            depends_on: Vec::new(),
            autostart: default_true(),
            autorestart: AutoRestartPolicy::default(),
            start_secs: default_start_secs(),
            start_retries: default_start_retries(),
            stop_signal: StopSignal::default(),
            stop_wait_secs: default_stop_wait_secs(),
            exit_codes: default_exit_codes(),
            umask: None,
            logs: ProgramLogsConfig::default(),
        }
    }

    pub fn validate(&self) -> Result<(), crate::error::ProgramError> {
        if self.priority > 99 {
            return Err(crate::error::ProgramError::ConfigError(format!(
                "Program '{}' priority {} must be in range [0, 99]",
                self.name, self.priority
            )));
        }
        if self.command.trim().is_empty() {
            return Err(crate::error::ProgramError::ConfigError(format!(
                "Program '{}' command cannot be empty",
                self.name
            )));
        }
        if let Some(ref mb) = self.logs.max_bytes {
            crate::logging::parse_byte_size(mb)?;
        }
        Ok(())
    }
}
