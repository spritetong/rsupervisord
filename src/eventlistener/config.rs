// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::consts::{
    DEFAULT_EVENTLISTENER_PRIORITY, DEFAULT_EVENT_BUFFER_SIZE, default_result_handler, i32_value,
    usize_value,
};
use crate::error::ProgramError;
use crate::program::config::{AutoRestartPolicy, StopSignal};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::LazyLock;

/// Standard known supervisor event types and families (Python supervisor 4.2.5).
pub static KNOWN_EVENT_TYPES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    let mut s = HashSet::new();
    // Root family
    s.insert("EVENT");
    // Process state family
    s.insert("PROCESS_STATE");
    s.insert("PROCESS_STATE_STARTING");
    s.insert("PROCESS_STATE_RUNNING");
    s.insert("PROCESS_STATE_BACKOFF");
    s.insert("PROCESS_STATE_STOPPING");
    s.insert("PROCESS_STATE_EXITED");
    s.insert("PROCESS_STATE_STOPPED");
    s.insert("PROCESS_STATE_FATAL");
    s.insert("PROCESS_STATE_UNKNOWN");
    // Process log family
    s.insert("PROCESS_LOG");
    s.insert("PROCESS_LOG_STDOUT");
    s.insert("PROCESS_LOG_STDERR");
    // Process communication family
    s.insert("PROCESS_COMMUNICATION");
    s.insert("PROCESS_COMMUNICATION_STDOUT");
    s.insert("PROCESS_COMMUNICATION_STDERR");
    // Remote communication
    s.insert("REMOTE_COMMUNICATION");
    // Tick family
    s.insert("TICK");
    s.insert("TICK_5");
    s.insert("TICK_60");
    s.insert("TICK_3600");
    // Process group family
    s.insert("PROCESS_GROUP");
    s.insert("PROCESS_GROUP_ADDED");
    s.insert("PROCESS_GROUP_REMOVED");
    // Supervisor state change family
    s.insert("SUPERVISOR_STATE_CHANGE");
    s.insert("SUPERVISOR_STATE_CHANGE_RUNNING");
    s.insert("SUPERVISOR_STATE_CHANGE_STOPPING");
    s
});

/// Validates whether an event name is a known supervisor event type or family.
pub fn is_valid_event_type(name: &str) -> bool {
    let upper = name.trim().to_ascii_uppercase();
    KNOWN_EVENT_TYPES.contains(upper.as_str())
}

/// Validates a list of event subscriptions.
pub fn validate_event_list(events: &[String]) -> Result<(), ProgramError> {
    if events.is_empty() {
        return Err(ProgramError::ConfigError(
            "Event listener must declare at least one event subscription in 'events'".to_string(),
        ));
    }
    for event in events {
        let clean = event.trim().to_ascii_uppercase();
        if clean.is_empty() || !is_valid_event_type(&clean) {
            return Err(ProgramError::ConfigError(format!(
                "Unknown supervisor event type '{}'",
                event
            )));
        }
    }
    Ok(())
}

/// Raw representation of `[eventlistener:x]` from INI or YAML configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventListenerConfigRaw {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub events: Vec<String>,
    #[serde(default = "usize_value::<DEFAULT_EVENT_BUFFER_SIZE>")]
    pub buffer_size: usize,
    #[serde(default = "default_result_handler")]
    pub result_handler: String,
    #[serde(default = "i32_value::<DEFAULT_EVENTLISTENER_PRIORITY>")]
    pub priority: i32,
    #[serde(default)]
    pub numprocs: Option<usize>,
    #[serde(default)]
    pub numprocs_start: Option<usize>,
    #[serde(default)]
    pub process_name: Option<String>,
    #[serde(default)]
    pub autostart: Option<bool>,
    #[serde(default)]
    pub autorestart: Option<AutoRestartPolicy>,
    #[serde(default)]
    pub start_secs: Option<u64>,
    #[serde(default)]
    pub start_retries: Option<u32>,
    #[serde(default)]
    pub stop_signal: Option<StopSignal>,
    #[serde(default)]
    pub stop_wait_secs: Option<u64>,
    #[serde(default)]
    pub directory: Option<PathBuf>,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub environment: HashMap<String, String>,
    #[serde(default)]
    pub umask: Option<u32>,
    #[serde(default)]
    pub stdout_logfile: Option<PathBuf>,
    #[serde(default)]
    pub stderr_logfile: Option<PathBuf>,
    #[serde(default)]
    pub redirect_stderr: Option<bool>,
}

/// Resolved event listener configuration attached to a program or pool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventListenerConfig {
    pub pool_name: String,
    pub events: Vec<String>,
    pub buffer_size: usize,
    pub result_handler: String,
}

impl EventListenerConfig {
    pub fn new(
        pool_name: impl Into<String>,
        events: Vec<String>,
        buffer_size: usize,
        result_handler: impl Into<String>,
    ) -> Self {
        Self {
            pool_name: pool_name.into(),
            events,
            buffer_size: buffer_size.max(1),
            result_handler: result_handler.into(),
        }
    }
}
