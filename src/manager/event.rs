// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use crate::program::state::{HealthStatus, ProgramState};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

/// Strongly-typed system lifecycle and status mutation events.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, strum::AsRefStr, strum::IntoStaticStr,
)]
#[serde(tag = "type", content = "data")]
pub enum SystemEvent {
    /// Fired whenever a managed program transitions across states.
    StateChanged {
        name: String,
        #[serde(default)]
        group: String,
        old_state: ProgramState,
        new_state: ProgramState,
        pid: Option<u32>,
        exit_code: Option<i32>,
        description: String,
        #[serde(default)]
        tries: u32,
    },
    /// Fired when a periodic tick interval fires (5, 60, 3600 seconds).
    Tick { interval: u32, when: u64 },
    /// Fired when an XML-RPC sendRemoteCommEvent is called.
    RemoteCommunication { type_str: String, data: String },
    /// Fired whenever a managed program's active health probe changes state.
    HealthChanged {
        name: String,
        healthy: bool,
        status: HealthStatus,
        reason: Option<String>,
    },
    /// Fired when an incremental configuration hot reload completes.
    ConfigReloaded {
        added: Vec<String>,
        removed: Vec<String>,
        modified: Vec<String>,
        unchanged: Vec<String>,
    },
    /// Fired when a process group is added at runtime via addProcessGroup.
    ProcessGroupAdded { group: String },
    /// Fired when a process group is removed at runtime via removeProcessGroup.
    ProcessGroupRemoved { group: String },
    /// Fired on daemon-level lifecycle milestones (startup, shutdown).
    DaemonLifecycle { action: String, timestamp_secs: u64 },
    /// Fired when a program is scheduled or triggered by a cron expression.
    CronTriggered {
        name: String,
        group: String,
        action: String,
        expression: String,
    },
    /// Fired immediately before a program's pre_start hook script begins execution.
    ProcessPreStart {
        name: String,
        group: String,
        command: String,
    },
    /// Fired when a program's pre_start hook fails or times out.
    ProcessPreStartFailed {
        name: String,
        group: String,
        error: String,
    },
    /// Fired immediately before a program's pre_stop hook script begins execution.
    ProcessPreStop {
        name: String,
        group: String,
        command: String,
    },
    /// Fired when a program's pre_stop hook fails or times out.
    ProcessPreStopFailed {
        name: String,
        group: String,
        error: String,
    },
}

impl SystemEvent {
    /// Returns the static event variant name as a string slice without allocation.
    #[inline]
    pub fn event_type(&self) -> &'static str {
        self.into()
    }

    /// Returns the program name associated with this event, if applicable.
    pub fn program_name(&self) -> Option<&str> {
        match self {
            Self::StateChanged { name, .. }
            | Self::HealthChanged { name, .. }
            | Self::CronTriggered { name, .. }
            | Self::ProcessPreStart { name, .. }
            | Self::ProcessPreStartFailed { name, .. }
            | Self::ProcessPreStop { name, .. }
            | Self::ProcessPreStopFailed { name, .. } => Some(name),
            Self::ConfigReloaded { .. }
            | Self::DaemonLifecycle { .. }
            | Self::Tick { .. }
            | Self::RemoteCommunication { .. }
            | Self::ProcessGroupAdded { .. }
            | Self::ProcessGroupRemoved { .. } => None,
        }
    }

    /// Returns true if this event indicates an operational failure or error.
    pub fn is_failure(&self) -> bool {
        match self {
            Self::ProcessPreStartFailed { .. } | Self::ProcessPreStopFailed { .. } => true,
            Self::HealthChanged { healthy, .. } => !healthy,
            Self::StateChanged { new_state, .. } => {
                matches!(new_state, ProgramState::Fatal | ProgramState::Backoff)
            }
            _ => false,
        }
    }
}

/// A structured log record emitted by a managed program's stdout or stderr.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    pub program: String,
    pub stream: String,
    pub line: String,
    pub timestamp_millis: u64,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub events_enabled: bool,
}

impl LogEntry {
    pub fn with_details(
        program: impl Into<String>,
        group: Option<String>,
        pid: Option<u32>,
        stream: impl Into<String>,
        line: impl Into<String>,
        events_enabled: bool,
    ) -> Self {
        let timestamp_millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        Self {
            program: program.into(),
            stream: stream.into(),
            line: line.into(),
            timestamp_millis,
            group,
            pid,
            events_enabled,
        }
    }

    pub fn new(
        program: impl Into<String>,
        stream: impl Into<String>,
        line: impl Into<String>,
    ) -> Self {
        Self::with_details(program, None, None, stream, line, false)
    }

    /// Returns true if this log line was captured from stderr.
    #[inline]
    pub fn is_stderr(&self) -> bool {
        self.stream == "stderr"
    }
}

/// Centralized star-topology event hub distributing system events and aggregated logs.
#[derive(Clone)]
pub struct EventHub {
    system_tx: broadcast::Sender<SystemEvent>,
    log_tx: broadcast::Sender<LogEntry>,
}

impl Default for EventHub {
    fn default() -> Self {
        Self::new(256, 2048)
    }
}

impl EventHub {
    /// Creates a new EventHub with bounded broadcast capacities for system events and logs.
    pub fn new(system_capacity: usize, log_capacity: usize) -> Self {
        let (system_tx, _) = broadcast::channel(system_capacity.max(16));
        let (log_tx, _) = broadcast::channel(log_capacity.max(64));
        Self { system_tx, log_tx }
    }

    /// Publishes a system lifecycle event to all active subscribers.
    /// Incurs zero cloning overhead if no subscribers are active.
    pub fn publish_system(&self, event: SystemEvent) {
        if self.system_tx.receiver_count() > 0 {
            let _ = self.system_tx.send(event);
        }
    }

    /// Alias for publish_system.
    #[inline]
    pub fn publish(&self, event: SystemEvent) {
        self.publish_system(event);
    }

    /// Publishes an aggregated log entry to all active subscribers.
    /// Incurs zero cloning overhead if no subscribers are active.
    pub fn publish_log(&self, entry: LogEntry) {
        if self.log_tx.receiver_count() > 0 {
            let _ = self.log_tx.send(entry);
        }
    }

    /// Subscribes to the global system events broadcast stream.
    pub fn subscribe_system(&self) -> broadcast::Receiver<SystemEvent> {
        self.system_tx.subscribe()
    }

    /// Alias for subscribe_system.
    #[inline]
    pub fn subscribe(&self) -> broadcast::Receiver<SystemEvent> {
        self.subscribe_system()
    }

    /// Subscribes to the global aggregated logs broadcast stream.
    pub fn subscribe_logs(&self) -> broadcast::Receiver<LogEntry> {
        self.log_tx.subscribe()
    }

    /// Returns the current number of active system event subscribers.
    pub fn system_subscriber_count(&self) -> usize {
        self.system_tx.receiver_count()
    }

    /// Returns the current number of active aggregated log subscribers.
    pub fn log_subscriber_count(&self) -> usize {
        self.log_tx.receiver_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_hub_zero_subscribers_noop() {
        let hub = EventHub::default();
        assert_eq!(hub.system_subscriber_count(), 0);
        assert_eq!(hub.log_subscriber_count(), 0);

        hub.publish_system(SystemEvent::DaemonLifecycle {
            action: "startup".to_string(),
            timestamp_secs: 1000,
        });

        hub.publish_log(LogEntry::new("test", "stdout", "hello"));
    }

    #[tokio::test]
    async fn test_event_hub_broadcast_delivery() {
        let hub = EventHub::default();
        let mut sys_rx = hub.subscribe_system();
        let mut log_rx = hub.subscribe_logs();

        assert_eq!(hub.system_subscriber_count(), 1);
        assert_eq!(hub.log_subscriber_count(), 1);

        hub.publish_system(SystemEvent::StateChanged {
            name: "api".to_string(),
            group: "api".to_string(),
            old_state: ProgramState::Stopped,
            new_state: ProgramState::Running,
            pid: Some(1234),
            exit_code: None,
            description: "Started".to_string(),
            tries: 0,
        });

        hub.publish_log(LogEntry::new("api", "stdout", "ready on 8080"));

        let sys_evt = sys_rx.recv().await.expect("receive system event");
        match sys_evt {
            SystemEvent::StateChanged { name, pid, .. } => {
                assert_eq!(name, "api");
                assert_eq!(pid, Some(1234));
            }
            _ => panic!("unexpected event variant"),
        }

        let log_evt = log_rx.recv().await.expect("receive log event");
        assert_eq!(log_evt.program, "api");
        assert_eq!(log_evt.stream, "stdout");
        assert_eq!(log_evt.line, "ready on 8080");
        assert!(log_evt.timestamp_millis > 0);
    }

    #[test]
    fn test_system_event_metadata() {
        let evt = SystemEvent::StateChanged {
            name: "worker".to_string(),
            group: "default".to_string(),
            old_state: ProgramState::Starting,
            new_state: ProgramState::Running,
            pid: Some(42),
            exit_code: None,
            description: "Ready".to_string(),
            tries: 0,
        };

        assert_eq!(evt.event_type(), "StateChanged");
        assert_eq!(evt.program_name(), Some("worker"));
        assert!(!evt.is_failure());

        let failed_evt = SystemEvent::ProcessPreStartFailed {
            name: "worker".to_string(),
            group: "default".to_string(),
            error: "hook timeout".to_string(),
        };
        assert!(failed_evt.is_failure());
    }
}
