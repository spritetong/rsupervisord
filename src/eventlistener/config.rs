// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::consts::*;
use crate::error::ProgramError;
use crate::program::config::{AutoRestartPolicy, StopSignal};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;

/// Standard known supervisor event types and families (Python supervisor 4.2.5).
static KNOWN_EVENT_TYPES: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    let mut v = vec![
        // Root family
        "EVENT",
        // Process state family
        "PROCESS_STATE",
        "PROCESS_STATE_STARTING",
        "PROCESS_STATE_RUNNING",
        "PROCESS_STATE_BACKOFF",
        "PROCESS_STATE_STOPPING",
        "PROCESS_STATE_EXITED",
        "PROCESS_STATE_STOPPED",
        "PROCESS_STATE_FATAL",
        "PROCESS_STATE_UNKNOWN",
        // Process log family
        "PROCESS_LOG",
        "PROCESS_LOG_STDOUT",
        "PROCESS_LOG_STDERR",
        // Process communication family
        "PROCESS_COMMUNICATION",
        "PROCESS_COMMUNICATION_STDOUT",
        "PROCESS_COMMUNICATION_STDERR",
        // Remote communication
        "REMOTE_COMMUNICATION",
        // Tick family
        "TICK",
        "TICK_5",
        "TICK_60",
        "TICK_3600",
        // Process group family
        "PROCESS_GROUP",
        "PROCESS_GROUP_ADDED",
        "PROCESS_GROUP_REMOVED",
        // Supervisor state change family
        "SUPERVISOR_STATE_CHANGE",
        "SUPERVISOR_STATE_CHANGE_RUNNING",
        "SUPERVISOR_STATE_CHANGE_STOPPING",
    ];
    v.sort();
    v
});

/// Validates whether an event name is a known supervisor event type or family.
pub fn is_valid_event_type(name: &str) -> bool {
    KNOWN_EVENT_TYPES
        .binary_search_by(|&probe| {
            let it_probe = probe.bytes().map(|b| b.to_ascii_lowercase());
            let it_target = name.bytes().map(|b| b.to_ascii_lowercase());
            it_probe.cmp(it_target)
        })
        .is_ok()
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
    #[serde(default = "default_event_buffer_size")]
    pub buffer_size: usize,
    #[serde(default = "default_result_handler")]
    pub result_handler: String,
    #[serde(default = "default_eventlistener_priority")]
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
    #[serde(default, with = "crate::serde_util::option_duration_secs")]
    pub start_secs: Option<std::time::Duration>,
    #[serde(default)]
    pub start_retries: Option<u32>,
    #[serde(default)]
    pub stop_signal: Option<StopSignal>,
    #[serde(default, with = "crate::serde_util::option_duration_secs")]
    pub stop_wait_secs: Option<std::time::Duration>,
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
