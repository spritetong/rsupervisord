// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use crate::platform::traits::ProcessMetrics;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HealthStatus {
    #[default]
    None,
    Starting,
    Healthy,
    Unhealthy,
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HealthStatus::None => write!(f, "-"),
            HealthStatus::Starting => write!(f, "STARTING"),
            HealthStatus::Healthy => write!(f, "HEALTHY"),
            HealthStatus::Unhealthy => write!(f, "UNHEALTHY"),
        }
    }
}

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProgramStatus {
    pub name: String,
    #[serde(default)]
    pub group: String,
    pub state: ProgramState,
    pub pid: Option<u32>,
    pub uptime_secs: Option<u64>,
    pub exit_code: Option<i32>,
    pub is_healthy: bool,
    pub health: HealthStatus,
    pub metrics: Option<ProcessMetrics>,
    pub description: String,
}

impl ProgramStatus {
    pub fn new_stopped(name: impl Into<String>) -> Self {
        let n = name.into();
        Self {
            group: n.clone(),
            name: n,
            state: ProgramState::Stopped,
            pid: None,
            uptime_secs: None,
            exit_code: None,
            is_healthy: false,
            health: HealthStatus::None,
            metrics: None,
            description: "Stopped".to_string(),
        }
    }

    pub fn new_stopped_with_group(name: impl Into<String>, group: impl Into<String>) -> Self {
        let n = name.into();
        let g = group.into();
        Self {
            group: if g.is_empty() { n.clone() } else { g },
            name: n,
            state: ProgramState::Stopped,
            pid: None,
            uptime_secs: None,
            exit_code: None,
            is_healthy: false,
            health: HealthStatus::None,
            metrics: None,
            description: "Stopped".to_string(),
        }
    }

    /// Full name formatted as "group:name" if group is distinct from name, or "name" otherwise.
    pub fn full_name(&self) -> String {
        if !self.group.is_empty() && self.group != self.name {
            format!("{}:{}", self.group, self.name)
        } else {
            self.name.clone()
        }
    }
}
