// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    Default,
    strum::EnumString,
    strum::Display,
    strum::AsRefStr,
    strum::IntoStaticStr,
    strum::EnumIter,
)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum AutoRestartPolicy {
    Always,
    #[default]
    Unexpected,
    Never,
}

impl AutoRestartPolicy {
    /// Determines whether a process should restart given its exit code and list of expected exit codes.
    #[inline]
    pub fn should_restart(&self, exit_code: i32, expected_codes: &[i32]) -> bool {
        match self {
            Self::Always => true,
            Self::Never => false,
            Self::Unexpected => !expected_codes.contains(&exit_code),
        }
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    strum::EnumString,
    strum::Display,
    strum::AsRefStr,
    strum::IntoStaticStr,
    strum::EnumIter,
)]
#[strum(serialize_all = "UPPERCASE")]
#[serde(rename_all = "UPPERCASE")]
pub enum StopSignal {
    #[strum(
        serialize = "TERM",
        serialize = "SIGTERM",
        serialize = "sigterm",
        serialize = "term"
    )]
    #[serde(alias = "SIGTERM", alias = "TERM", alias = "sigterm", alias = "term")]
    Term,
    #[strum(
        serialize = "INT",
        serialize = "SIGINT",
        serialize = "sigint",
        serialize = "int"
    )]
    #[serde(alias = "SIGINT", alias = "INT", alias = "sigint", alias = "int")]
    Int,
    #[strum(
        serialize = "QUIT",
        serialize = "SIGQUIT",
        serialize = "sigquit",
        serialize = "quit"
    )]
    #[serde(alias = "SIGQUIT", alias = "QUIT", alias = "sigquit", alias = "quit")]
    Quit,
    #[strum(
        serialize = "KILL",
        serialize = "SIGKILL",
        serialize = "sigkill",
        serialize = "kill"
    )]
    #[serde(alias = "SIGKILL", alias = "KILL", alias = "sigkill", alias = "kill")]
    Kill,
    #[strum(
        serialize = "HUP",
        serialize = "SIGHUP",
        serialize = "sighup",
        serialize = "hup"
    )]
    #[serde(alias = "SIGHUP", alias = "HUP", alias = "sighup", alias = "hup")]
    Hup,
    #[strum(
        serialize = "CTRL_BREAK",
        serialize = "ctrl_break",
        serialize = "ctrlbreak",
        serialize = "CTRLBREAK"
    )]
    #[serde(
        rename = "CTRL_BREAK",
        alias = "ctrl_break",
        alias = "ctrlbreak",
        alias = "CTRLBREAK"
    )]
    CtrlBreak,
    #[strum(
        serialize = "CTRL_C",
        serialize = "ctrl_c",
        serialize = "ctrlc",
        serialize = "CTRLC"
    )]
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
    #[serde(default)]
    pub stdout_events_enabled: bool,
    #[serde(default)]
    pub stderr_events_enabled: bool,
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
            stdout_events_enabled: false,
            stderr_events_enabled: false,
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
    pub priority: u32,
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
    #[serde(default = "default_group_priority")]
    pub group_priority: u32,
    #[serde(default)]
    pub cron: Option<String>,
    #[serde(default)]
    pub cron_stop: Option<String>,
    #[serde(default)]
    pub pre_start: Option<String>,
    #[serde(default)]
    pub pre_stop: Option<String>,
    #[serde(default)]
    pub pre_start_ignore_failure: bool,
    #[serde(default = "default_hook_timeout_secs")]
    pub hook_timeout_secs: u64,
    #[serde(default)]
    pub restart_when_binary_changed: bool,
    #[serde(default)]
    pub restart_signal_when_binary_changed: Option<StopSignal>,
    #[serde(default)]
    pub restart_cmd_when_binary_changed: Option<String>,
    #[serde(default)]
    pub restart_directory_monitor: Option<PathBuf>,
    #[serde(default)]
    pub restart_file_pattern: Option<String>,
    #[serde(default)]
    pub restart_signal_when_file_changed: Option<StopSignal>,
    #[serde(default)]
    pub restart_cmd_when_file_changed: Option<String>,
    #[serde(default = "default_restart_debounce_secs")]
    pub restart_debounce_secs: u64,
    #[serde(default)]
    pub event_listener: Option<crate::eventlistener::EventListenerConfig>,
}

fn default_group_priority() -> u32 {
    999
}

fn default_restart_debounce_secs() -> u64 {
    5
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

fn default_priority() -> u32 {
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

pub fn default_hook_timeout_secs() -> u64 {
    15
}

impl ProgramConfig {
    pub fn new(name: impl Into<String>, command: impl Into<String>) -> Self {
        let n = name.into();
        Self {
            group: n.clone(),
            group_priority: default_group_priority(),
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
            cron: None,
            cron_stop: None,
            pre_start: None,
            pre_stop: None,
            pre_start_ignore_failure: false,
            hook_timeout_secs: default_hook_timeout_secs(),
            restart_when_binary_changed: false,
            restart_signal_when_binary_changed: None,
            restart_cmd_when_binary_changed: None,
            restart_directory_monitor: None,
            restart_file_pattern: None,
            restart_signal_when_file_changed: None,
            restart_cmd_when_file_changed: None,
            restart_debounce_secs: default_restart_debounce_secs(),
            event_listener: None,
        }
    }

    /// Returns the full name including group (e.g. "group:program") if a distinct group is assigned,
    /// or just the program name otherwise.
    pub fn full_name(&self) -> String {
        if !self.group.is_empty() && self.group != self.name {
            format!("{}:{}", self.group, self.name)
        } else {
            self.name.clone()
        }
    }

    /// Returns true if a pre-start lifecycle hook is configured.
    #[inline]
    pub fn has_pre_start(&self) -> bool {
        self.pre_start
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
    }

    /// Returns true if a pre-stop lifecycle hook is configured.
    #[inline]
    pub fn has_pre_stop(&self) -> bool {
        self.pre_stop
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
    }

    /// Returns true if cron schedule execution is configured.
    #[inline]
    pub fn has_cron(&self) -> bool {
        self.cron
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
    }

    /// Returns true if active health checking is configured.
    #[inline]
    pub fn has_health_check(&self) -> bool {
        self.health_check.is_some()
    }

    /// Returns a diagnostic formatted command string including arguments.
    pub fn full_command(&self) -> String {
        if self.args.is_empty() {
            self.command.clone()
        } else {
            format!("{} {}", self.command, self.args.join(" "))
        }
    }

    pub fn validate(&self) -> Result<(), crate::error::ProgramError> {
        if self.priority > 999 {
            return Err(crate::error::ProgramError::ConfigError(format!(
                "Program '{}' priority {} must be in range [0, 999]",
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
        if let Some(ref expr) = self.cron
            && let Err(e) = expr.parse::<croner::Cron>()
        {
            return Err(crate::error::ProgramError::InvalidCronExpression {
                name: self.name.clone(),
                expression: expr.clone(),
                reason: e.to_string(),
            });
        }
        if let Some(ref expr) = self.cron_stop
            && let Err(e) = expr.parse::<croner::Cron>()
        {
            return Err(crate::error::ProgramError::InvalidCronExpression {
                name: self.name.clone(),
                expression: expr.clone(),
                reason: e.to_string(),
            });
        }
        Ok(())
    }
}
