// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

//! Global constants: config defaults, runtime timeouts, permissions, and
//! generic serde-default helpers.
//!
//! Field types on config structs follow the on-disk format and are not changed
//! here; only default values are centralized. Runtime code should prefer
//! `Duration` constants below over scattered magic numbers.

use std::time::Duration;

// ---------------------------------------------------------------------------
// Generic serde `default = "..."` helpers
// ---------------------------------------------------------------------------
// Usage: #[serde(default = "u64_value::<DEFAULT_PRIORITY>")]
// Import the helper and the constant at the top of the defining module.

/// Returns the const `bool` (serde default for `bool` fields).
pub const fn bool_value<const B: bool>() -> bool {
    B
}

/// Returns the const `u64` (serde default for `u64` fields).
pub const fn u64_value<const N: u64>() -> u64 {
    N
}

/// Returns the const `u32` (serde default for `u32` fields).
pub const fn u32_value<const N: u32>() -> u32 {
    N
}

/// Returns the const `u16` (serde default for `u16` fields).
pub const fn u16_value<const N: u16>() -> u16 {
    N
}

/// Returns the const `i32` (serde default for `i32` fields).
pub const fn i32_value<const N: i32>() -> i32 {
    N
}

/// Returns the const `usize` (serde default for `usize` fields).
pub const fn usize_value<const N: usize>() -> usize {
    N
}

/// Builds a serde default function for types that cannot be expressed with
/// const generics (`String`, `Vec<T>`, etc.). Invoke only inside this module
/// so every non-generic default lives in one place.
macro_rules! default_fn {
    ($name:ident: $ty:ty = $expr:expr) => {
        pub fn $name() -> $ty {
            $expr
        }
    };
}

default_fn!(default_log_level: String = "info".into());
default_fn!(
    default_result_handler: String = "supervisor.dispatchers:default_handler".into()
);
default_fn!(default_exit_codes: Vec<i32> = vec![0]);

// ---------------------------------------------------------------------------
// Runtime: await / oneshot timeouts
// ---------------------------------------------------------------------------
// How long the caller waits for a oneshot reply from the actor. Tiers reflect
// operation scope; sync actors are local so these are upper bounds, not typical
// latencies.

/// Single-program action (start, default API/CLI action timeout).
pub const AWAIT_ACTION: Duration = Duration::from_secs(30);
/// Group-scoped action (start/stop group, reload, add group).
pub const AWAIT_GROUP: Duration = Duration::from_secs(60);
/// Daemon-wide / bulk action (start/stop all, restart group, shutdown).
pub const AWAIT_BULK: Duration = Duration::from_secs(120);
/// Read-only query (status, logs, config).
pub const AWAIT_QUERY: Duration = Duration::from_secs(10);
/// Stdin forward with backpressure wait.
pub const AWAIT_STDIN: Duration = Duration::from_secs(15);

// ---------------------------------------------------------------------------
// Runtime: stop / grace / drain
// ---------------------------------------------------------------------------

/// Default graceful-stop wait when `stop_wait_secs` is absent.
pub const DEFAULT_STOP_WAIT_SECS: u64 = 10;
/// Manager-level extra headroom on top of grace for stop oneshot await.
pub const STOP_GRACE_EXTRA: Duration = Duration::from_secs(15);
/// Manager-level extra headroom on top of grace for restart oneshot await.
pub const RESTART_GRACE_EXTRA: Duration = Duration::from_secs(25);
/// Program-level extra headroom on top of grace for stop oneshot await.
pub const PROCESS_STOP_GRACE_EXTRA: Duration = Duration::from_secs(5);
/// Program-level extra headroom on top of grace for restart oneshot await.
pub const PROCESS_RESTART_GRACE_EXTRA: Duration = Duration::from_secs(10);
/// Upper bound for any computed operation timeout.
pub const MAX_TIMEOUT: Duration = Duration::from_secs(86400);
/// Hard wait for pump/task drain after kill or shutdown (also post-kill reap wait).
pub const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
/// Short retry delay (stdin flush, file readable, autorestart pacing).
pub const SHORT_RETRY_DELAY: Duration = Duration::from_millis(500);
/// SSE keep-alive ping interval.
pub const SSE_KEEPALIVE: Duration = Duration::from_secs(15);

// ---------------------------------------------------------------------------
// Config defaults (values shared by serde defaults and runtime fallbacks)
// ---------------------------------------------------------------------------

/// Default program priority (also the fallback when a name is missing).
pub const DEFAULT_PRIORITY: u32 = 50;
/// Default group priority (sorts groups after specific programs).
pub const DEFAULT_GROUP_PRIORITY: u32 = 999;
/// Maximum accepted priority (validation bound).
pub const MAX_PRIORITY: u32 = 999;
/// Startup success window in seconds.
pub const DEFAULT_START_SECS: u64 = 1;
/// Startup retry count before fatal.
pub const DEFAULT_START_RETRIES: u32 = 3;
/// pre_start / pre_stop hook timeout in seconds.
pub const DEFAULT_HOOK_TIMEOUT_SECS: u64 = 15;
/// File-change restart debounce in seconds.
pub const DEFAULT_RESTART_DEBOUNCE_SECS: u64 = 5;
/// Health probe interval in seconds.
pub const DEFAULT_HEALTH_INTERVAL_SECS: u64 = 10;
/// Single health probe timeout in seconds.
pub const DEFAULT_HEALTH_TIMEOUT_SECS: u64 = 2;
/// Consecutive health failures before marking unhealthy.
pub const DEFAULT_HEALTH_FAILURE_THRESHOLD: u32 = 3;
/// Delay before the first health probe in seconds.
pub const DEFAULT_HEALTH_INITIAL_DELAY_SECS: u64 = 0;
/// Expected HTTP status for HTTP health checks.
pub const DEFAULT_HTTP_EXPECTED_STATUS: u16 = 200;
/// Metrics idle timeout in seconds (0 = never).
pub const DEFAULT_METRICS_IDLE_TIMEOUT_SECS: u64 = 30;
/// Metrics sampling interval in seconds.
pub const DEFAULT_METRICS_INTERVAL_SECS: u64 = 2;
/// Default action timeout (seconds) for API/CLI/JSON-RPC lifecycle ops.
pub const DEFAULT_ACTION_TIMEOUT_SECS: u64 = 30;
/// Default event-listener result buffer size.
pub const DEFAULT_EVENT_BUFFER_SIZE: usize = 10;
/// Default event-listener priority (runs before program events).
pub const DEFAULT_EVENTLISTENER_PRIORITY: i32 = -1;
/// Default line count for `tail`-style log APIs.
pub const DEFAULT_LOG_LINES: usize = 100;

// ---------------------------------------------------------------------------
// Logging rotation defaults (aligned with Python supervisor)
// ---------------------------------------------------------------------------

/// Default max log file size before rotation (50MB, Python parity).
pub const DEFAULT_LOG_MAX_BYTES: usize = 50 * 1024 * 1024;
/// Human-readable form of [`DEFAULT_LOG_MAX_BYTES`] for string-typed fields.
pub const DEFAULT_LOG_MAX_BYTES_HUMAN: &str = "50MB";
/// Default number of rotated log backups (Python parity).
pub const DEFAULT_LOG_BACKUPS: usize = 10;

// ---------------------------------------------------------------------------
// Permissions / umask
// ---------------------------------------------------------------------------

/// IPC mode when `allow_unelevated` requires world access for local CLI.
pub const UDS_CHMOD_UNELEVATED: u32 = 0o777;
/// Default IPC mode on Unix (owner only).
#[cfg(windows)]
pub const UDS_CHMOD: u32 = 0o770; // owner + Administrators on Windows
#[cfg(not(windows))]
pub const UDS_CHMOD: u32 = 0o700; // owner only on Unix
/// Permission bits accepted by `parse_chmod` (mode + setuid/setgid/sticky).
pub const CHMOD_MASK: u32 = 0o7777;
/// umask applied while binding the Unix socket to close the bind→chmod race.
pub const BIND_UMASK: u32 = 0o077;
