// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum StopSignal {
    #[serde(alias = "SIGTERM", alias = "TERM", alias = "sigterm", alias = "term")]
    Term,
    #[serde(alias = "SIGINT", alias = "INT", alias = "sigint", alias = "int")]
    Int,
    #[serde(alias = "SIGQUIT", alias = "QUIT", alias = "sigquit", alias = "quit")]
    Quit,
    #[serde(alias = "SIGKILL", alias = "KILL", alias = "sigkill", alias = "kill")]
    Kill,
    #[serde(
        rename = "CTRL_BREAK",
        alias = "ctrl_break",
        alias = "ctrlbreak",
        alias = "CTRLBREAK"
    )]
    CtrlBreak,
    #[serde(rename = "CTRL_C", alias = "ctrl_c", alias = "ctrlc", alias = "CTRLC")]
    CtrlC,
}

impl Default for StopSignal {
    fn default() -> Self {
        crate::platform::native_platform().default_stop_signal()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProgramLogsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
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

impl Default for ProgramLogsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            stdout: None,
            stderr: None,
            max_bytes: None,
            backups: None,
            redirect_stderr: false,
        }
    }
}

impl ProgramLogsConfig {
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn is_stdout_disabled(&self) -> bool {
        if !self.enabled {
            return true;
        }
        if let Some(ref p) = self.stdout {
            let s = p.to_string_lossy().to_lowercase();
            s == "/dev/null" || s == "null" || s == "none" || s == "off"
        } else {
            false
        }
    }

    pub fn is_stderr_disabled(&self) -> bool {
        if !self.enabled {
            return true;
        }
        if let Some(ref p) = self.stderr {
            let s = p.to_string_lossy().to_lowercase();
            s == "/dev/null" || s == "null" || s == "none" || s == "off"
        } else {
            false
        }
    }
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
    #[serde(default)]
    pub health_check: Option<HealthCheckConfig>,
    #[serde(default)]
    pub group: String,
}

impl ProgramConfig {
    /// Returns the full name including group (e.g. "group:program") if a distinct group is assigned,
    /// or just the program name otherwise.
    pub fn full_name(&self) -> String {
        if !self.group.is_empty() && self.group != self.name {
            format!("{}:{}", self.group, self.name)
        } else {
            self.name.clone()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum HealthCheckType {
    Http {
        url: String,
        #[serde(default = "default_http_expected_status")]
        expected_status: u16,
    },
    Tcp {
        endpoint: String,
    },
    Exec {
        command: String,
    },
}

fn default_http_expected_status() -> u16 {
    200
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthCheckConfig {
    #[serde(flatten)]
    pub check_type: HealthCheckType,
    #[serde(default = "default_health_interval_secs")]
    pub interval_secs: u64,
    #[serde(default = "default_health_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_health_failure_threshold")]
    pub failure_threshold: u32,
    #[serde(default = "default_health_initial_delay_secs")]
    pub initial_delay_secs: u64,
}

fn default_health_interval_secs() -> u64 {
    10
}

fn default_health_timeout_secs() -> u64 {
    2
}

fn default_health_failure_threshold() -> u32 {
    3
}

fn default_health_initial_delay_secs() -> u64 {
    0
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
        let n = name.into();
        Self {
            group: n.clone(),
            name: n,
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
            health_check: None,
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
