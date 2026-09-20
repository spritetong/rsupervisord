// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use parking_lot::RwLock;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Tracks client activity (CLI, REST API, Web UI) to auto-pause background
/// resource utilization metrics (CPU/MEM) collection when no clients are observing.
#[derive(Debug, Clone)]
pub struct ActivityTracker {
    last_activity: Arc<RwLock<Instant>>,
    idle_timeout_secs: u64,
    enabled: bool,
}

impl ActivityTracker {
    /// Creates a new ActivityTracker.
    /// If `idle_timeout_secs` is 0, the tracker is always active (never times out).
    /// If `enabled` is false, metrics collection is globally disabled.
    pub fn new(idle_timeout_secs: u64, enabled: bool) -> Self {
        Self {
            last_activity: Arc::new(RwLock::new(Instant::now())),
            idle_timeout_secs,
            enabled,
        }
    }

    /// Records that an external client performed an action (CLI command, Web UI polling, SSE stream).
    pub fn record_activity(&self) {
        *self.last_activity.write() = Instant::now();
    }

    /// Checks whether metrics collection should be active right now.
    pub fn is_metrics_active(&self) -> bool {
        if !self.enabled {
            return false;
        }
        if self.idle_timeout_secs == 0 {
            return true;
        }
        self.last_activity.read().elapsed() < Duration::from_secs(self.idle_timeout_secs)
    }

    /// Returns the configured idle timeout in seconds.
    pub fn idle_timeout_secs(&self) -> u64 {
        self.idle_timeout_secs
    }

    /// Returns whether metrics are globally enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
}

impl Default for ActivityTracker {
    fn default() -> Self {
        Self::new(30, true)
    }
}
