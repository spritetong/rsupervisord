// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use crate::eventlistener::events::{
    event_matches_subscription, format_process_log_event, format_process_state_event,
    format_remote_comm_event, format_tick_event,
};
use crate::eventlistener::protocol::{EventEnvelope, next_global_serial};
use crate::manager::event::{EventHub, SystemEvent};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// Shared inner state of an event listener pool.
pub struct EventListenerPoolInner {
    pub pool_name: String,
    pub server_identifier: String,
    pub events: Vec<String>,
    pub buffer_size: usize,
    pub pool_serial: AtomicU64,
    pub buffer: Mutex<VecDeque<EventEnvelope>>,
    pub dispatch_notify: Notify,
    pub cancel_token: CancellationToken,
}

impl EventListenerPoolInner {
    /// Pushes a newly generated event into the pool's buffer if it matches any event subscription.
    pub fn accept_event(&self, event_name: &str, payload: &str) {
        if !event_matches_subscription(event_name, &self.events) {
            return;
        }

        let global_serial = next_global_serial();
        let pool_serial = self.pool_serial.fetch_add(1, Ordering::Relaxed);
        let envelope = EventEnvelope::new(
            global_serial,
            pool_serial,
            &self.pool_name,
            event_name,
            payload,
        );

        {
            let mut buf = self.buffer.lock();
            if buf.len() >= self.buffer_size
                && let Some(discarded) = buf.pop_front()
            {
                tracing::error!(
                    "pool {} event buffer overflowed, discarding event {}",
                    self.pool_name,
                    discarded.global_serial
                );
            }
            buf.push_back(envelope);
        }

        self.dispatch_notify.notify_waiters();
    }

    /// Re-queues an event at the head of the buffer after a process rejects it (e.g. RejectEvent).
    pub fn requeue_rejected(&self, envelope: EventEnvelope) {
        {
            let mut buf = self.buffer.lock();
            if buf.len() >= self.buffer_size
                && let Some(discarded) = buf.pop_back()
            {
                tracing::error!(
                    "pool {} event buffer overflowed, discarding event {}",
                    self.pool_name,
                    discarded.global_serial
                );
            }
            buf.push_front(envelope);
        }
        self.dispatch_notify.notify_waiters();
    }

    /// Pops the next event from the buffer if available.
    pub fn pop_next_event(&self) -> Option<EventEnvelope> {
        let mut buf = self.buffer.lock();
        buf.pop_front()
    }
}

/// An Event Listener Pool managing event subscription, bounded buffering, and dispatching.
#[derive(Clone)]
pub struct EventListenerPool {
    inner: Arc<EventListenerPoolInner>,
}

impl EventListenerPool {
    pub fn new(
        pool_name: impl Into<String>,
        server_identifier: impl Into<String>,
        events: Vec<String>,
        buffer_size: usize,
        cancel_token: CancellationToken,
    ) -> Self {
        let inner = Arc::new(EventListenerPoolInner {
            pool_name: pool_name.into(),
            server_identifier: server_identifier.into(),
            events,
            buffer_size: buffer_size.max(1),
            pool_serial: AtomicU64::new(0),
            buffer: Mutex::new(VecDeque::new()),
            dispatch_notify: Notify::new(),
            cancel_token,
        });

        Self { inner }
    }

    pub fn inner(&self) -> &Arc<EventListenerPoolInner> {
        &self.inner
    }

    pub fn pool_name(&self) -> &str {
        &self.inner.pool_name
    }

    pub fn server_identifier(&self) -> &str {
        &self.inner.server_identifier
    }

    /// Spawns the background task listening for `SystemEvent` and `LogEntry` broadcasts from `EventHub`.
    pub fn spawn_event_hub_listener(&self, event_hub: EventHub) {
        let pool = self.clone();
        let cancel = self.inner.cancel_token.clone();

        tokio::spawn(async move {
            let mut sys_rx = event_hub.subscribe_system();
            let mut log_rx = event_hub.subscribe_logs();

            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    sys_msg = sys_rx.recv() => {
                        match sys_msg {
                            Ok(event) => pool.handle_system_event(&event),
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                                tracing::warn!(
                                    pool = %pool.pool_name(),
                                    skipped,
                                    "EventListenerPool lagged on SystemEvent broadcast"
                                );
                            }
                        }
                    }
                    log_msg = log_rx.recv() => {
                        match log_msg {
                            Ok(entry) => {
                                pool.handle_log_entry(&entry);
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                                tracing::warn!(
                                    pool = %pool.pool_name(),
                                    skipped,
                                    "EventListenerPool lagged on LogEntry broadcast"
                                );
                            }
                        }
                    }
                }
            }
        });
    }

    fn handle_system_event(&self, event: &SystemEvent) {
        match event {
            SystemEvent::StateChanged {
                name,
                group,
                old_state,
                new_state,
                pid,
                exit_code,
                tries,
                ..
            } => {
                let actual_group = if group.is_empty() {
                    name.split_once(':').map(|(g, _)| g).unwrap_or(name)
                } else {
                    group.as_str()
                };
                let prog_name = name.split_once(':').map(|(_, p)| p).unwrap_or(name);
                let expected = exit_code.map(|c| c == 0).unwrap_or(true);
                let (evt_name, payload) = format_process_state_event(
                    prog_name,
                    actual_group,
                    *old_state,
                    *new_state,
                    *pid,
                    expected,
                    *tries,
                );
                self.inner.accept_event(evt_name, &payload);
            }
            SystemEvent::Tick { interval, when } => {
                let (evt_name, payload) = format_tick_event(*interval, *when);
                self.inner.accept_event(&evt_name, &payload);
            }
            SystemEvent::RemoteCommunication { type_str, data } => {
                let (evt_name, payload) = format_remote_comm_event(type_str, data);
                self.inner.accept_event(evt_name, &payload);
            }
            SystemEvent::DaemonLifecycle { action, .. } => {
                if action == "running" {
                    self.inner
                        .accept_event("SUPERVISOR_STATE_CHANGE_RUNNING", "");
                } else if action == "shutting_down" {
                    self.inner
                        .accept_event("SUPERVISOR_STATE_CHANGE_STOPPING", "");
                }
            }
            SystemEvent::ConfigReloaded { added, removed, .. } => {
                for g in added {
                    self.inner
                        .accept_event("PROCESS_GROUP_ADDED", &format!("groupname:{}\n", g));
                }
                for g in removed {
                    self.inner
                        .accept_event("PROCESS_GROUP_REMOVED", &format!("groupname:{}\n", g));
                }
            }
            SystemEvent::ProcessGroupAdded { group } => {
                self.inner
                    .accept_event("PROCESS_GROUP_ADDED", &format!("groupname:{}\n", group));
            }
            SystemEvent::ProcessGroupRemoved { group } => {
                self.inner
                    .accept_event("PROCESS_GROUP_REMOVED", &format!("groupname:{}\n", group));
            }
            _ => {}
        }
    }

    fn handle_log_entry(&self, entry: &crate::manager::event::LogEntry) {
        if !entry.events_enabled {
            return;
        }
        let prog_name = entry
            .program
            .split_once(':')
            .map(|(_, p)| p)
            .unwrap_or(&entry.program);
        let group = entry.group.as_deref().unwrap_or_else(|| {
            entry
                .program
                .split_once(':')
                .map(|(g, _)| g)
                .unwrap_or(&entry.program)
        });

        let (evt_name, payload) =
            format_process_log_event(prog_name, group, entry.pid, &entry.stream, &entry.line);
        self.inner.accept_event(evt_name, &payload);
    }
}
