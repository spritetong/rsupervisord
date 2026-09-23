// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::platform::traits::ProcessMetrics;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, strum::Display)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HealthStatus {
    #[default]
    #[strum(to_string = "-")]
    None,
    Starting,
    Healthy,
    Unhealthy,
}

impl HealthStatus {
    /// Returns true if the probe confirmed the process is healthy.
    #[inline]
    pub fn is_healthy(&self) -> bool {
        matches!(self, Self::Healthy)
    }

    /// Returns true if the probe reported an unhealthy condition.
    #[inline]
    pub fn is_unhealthy(&self) -> bool {
        matches!(self, Self::Unhealthy)
    }

    /// Returns true if the probe is in the initial starting/grace phase.
    #[inline]
    pub fn is_starting(&self) -> bool {
        matches!(self, Self::Starting)
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display, strum::IntoStaticStr,
)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
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
    /// Returns the standard supervisor state name (e.g. `RUNNING`, `STOPPED`).
    #[inline]
    pub fn supervisor_name(self) -> &'static str {
        self.into()
    }

    /// Returns true if the process is in an active lifecycle state (Starting, Running, Stopping).
    #[inline]
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Stopping)
    }

    /// Returns true if the process is actively in the Running state.
    #[inline]
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running)
    }

    /// Returns true if the process is completely stopped.
    #[inline]
    pub fn is_stopped(&self) -> bool {
        matches!(self, Self::Stopped)
    }

    /// Returns true if the process is in a non-running terminal or stopped state.
    #[inline]
    pub fn is_stopped_or_fatal(&self) -> bool {
        matches!(self, Self::Stopped | Self::Exited | Self::Fatal)
    }

    /// Returns true if the process has permanently stopped without recovery (Exited or Fatal).
    #[inline]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Exited | Self::Fatal)
    }

    /// Returns true if the program can be transitioned to Starting from this state.
    #[inline]
    pub fn can_start(&self) -> bool {
        matches!(self, Self::Stopped | Self::Exited | Self::Fatal)
    }

    /// Returns true if the program can accept a graceful stop request.
    #[inline]
    pub fn can_stop(&self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Backoff)
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
    #[serde(default)]
    pub cron: Option<String>,
    #[serde(default)]
    pub next_cron_run: Option<String>,
}

impl ProgramStatus {
    /// Creates an initial stopped status record for a standalone program.
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
            cron: None,
            next_cron_run: None,
        }
    }

    /// Creates an initial stopped status record for a grouped program.
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
            cron: None,
            next_cron_run: None,
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

    /// Returns true if the process is currently running.
    #[inline]
    pub fn is_running(&self) -> bool {
        self.state.is_running()
    }
}
