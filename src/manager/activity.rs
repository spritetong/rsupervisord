// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use parking_lot::RwLock;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// RAII guard representing an active client stream (e.g. SSE).
/// While any stream guard is alive, metrics sampling remains active.
#[derive(Debug)]
pub struct SseStreamGuard(Arc<AtomicUsize>);

impl Drop for SseStreamGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Tracks client activity (CLI, REST API, Web UI) to auto-pause background
/// resource utilization metrics (CPU/MEM) collection when no clients are observing.
#[derive(Debug, Clone)]
pub struct ActivityTracker {
    last_activity: Arc<RwLock<Instant>>,
    active_observers: Arc<AtomicUsize>,
    idle_timeout_secs: u64,
    interval_secs: u64,
    enabled: bool,
}

impl ActivityTracker {
    /// Creates a new ActivityTracker.
    /// If `idle_timeout_secs` is 0, the tracker is always active (never times out).
    /// If `enabled` is false, metrics collection is globally disabled.
    pub fn new(idle_timeout_secs: u64, enabled: bool) -> Self {
        Self::with_interval(idle_timeout_secs, 2, enabled)
    }

    /// Creates an ActivityTracker with custom sampling interval in seconds.
    pub fn with_interval(idle_timeout_secs: u64, interval_secs: u64, enabled: bool) -> Self {
        Self {
            last_activity: Arc::new(RwLock::new(Instant::now())),
            active_observers: Arc::new(AtomicUsize::new(0)),
            idle_timeout_secs,
            interval_secs: interval_secs.max(1),
            enabled,
        }
    }

    /// Records that an external client performed an action (CLI command, Web UI polling, SSE stream).
    pub fn record_activity(&self) {
        *self.last_activity.write() = Instant::now();
    }

    /// Enters an active SSE stream session, returning a RAII guard that keeps metrics active.
    pub fn enter_stream(&self) -> SseStreamGuard {
        self.active_observers.fetch_add(1, Ordering::SeqCst);
        self.record_activity();
        SseStreamGuard(self.active_observers.clone())
    }

    /// Returns the duration elapsed since last recorded activity.
    pub fn idle_duration(&self) -> Duration {
        self.last_activity.read().elapsed()
    }

    /// Returns true if the tracker has exceeded its configured idle timeout.
    pub fn is_idle(&self) -> bool {
        if self.idle_timeout_secs == 0 {
            false
        } else {
            self.idle_duration() >= Duration::from_secs(self.idle_timeout_secs)
        }
    }

    /// Returns the current number of active observer streams.
    #[inline]
    pub fn observer_count(&self) -> usize {
        self.active_observers.load(Ordering::Relaxed)
    }

    /// Checks whether metrics collection should be active right now.
    pub fn is_metrics_active(&self) -> bool {
        if !self.enabled {
            return false;
        }
        if self.active_observers.load(Ordering::Relaxed) > 0 {
            return true;
        }
        if self.idle_timeout_secs == 0 {
            return true;
        }
        self.last_activity.read().elapsed() < Duration::from_secs(self.idle_timeout_secs)
    }

    /// Returns the configured metrics sampling interval in seconds.
    #[inline]
    pub fn interval_secs(&self) -> u64 {
        self.interval_secs
    }

    /// Returns the configured idle timeout in seconds.
    #[inline]
    pub fn idle_timeout_secs(&self) -> u64 {
        self.idle_timeout_secs
    }

    /// Returns whether metrics are globally enabled.
    #[inline]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
}

impl Default for ActivityTracker {
    fn default() -> Self {
        Self::with_interval(30, 2, true)
    }
}
