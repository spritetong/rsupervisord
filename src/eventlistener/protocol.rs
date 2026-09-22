// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use std::sync::atomic::{AtomicU64, Ordering};

static GLOBAL_SERIAL: AtomicU64 = AtomicU64::new(0);

/// Allocates the next global monotonic serial number (wraps at u64::MAX).
pub fn next_global_serial() -> u64 {
    GLOBAL_SERIAL.fetch_add(1, Ordering::Relaxed)
}

#[doc(hidden)]
pub fn reset_global_serial_for_test() {
    GLOBAL_SERIAL.store(0, Ordering::Relaxed);
}

/// An immutable event envelope ready for serialization and transmission to listener processes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventEnvelope {
    pub global_serial: u64,
    pub pool_serial: u64,
    pub pool: String,
    pub event_name: String,
    pub payload: String,
}

impl EventEnvelope {
    pub fn new(
        global_serial: u64,
        pool_serial: u64,
        pool: impl Into<String>,
        event_name: impl Into<String>,
        payload: impl Into<String>,
    ) -> Self {
        Self {
            global_serial,
            pool_serial,
            pool: pool.into(),
            event_name: event_name.into(),
            payload: payload.into(),
        }
    }

    /// Formats the envelope into the standard supervisor 3.0 wire protocol representation:
    /// `ver:3.0 server:<id> serial:<serial> pool:<pool> poolserial:<poolserial> eventname:<name> len:<char_len>\n<payload>`
    pub fn format_wire(&self, server_identifier: &str) -> String {
        let char_len = self.payload.chars().count();
        format!(
            "ver:3.0 server:{} serial:{} pool:{} poolserial:{} eventname:{} len:{}\n{}",
            server_identifier,
            self.global_serial,
            self.pool,
            self.pool_serial,
            self.event_name,
            char_len,
            self.payload
        )
    }
}

/// State of a listener process in the `READY`/`RESULT` handshake state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ListenerState {
    /// Process is spawned or currently busy processing, waiting for `READY\n`.
    #[default]
    Acknowledged,
    /// Process宣告可接收事件 (`READY\n` received).
    Ready,
    /// Event has been delivered, awaiting `RESULT <n>\n<data>`.
    Busy,
    /// Protocol violation occurred. Process is permanently marked unknown and will receive no further events.
    Unknown,
}

/// Result of evaluating a `RESULT` response from a listener process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessResult {
    /// Process acknowledged successful processing ("OK").
    Ok,
    /// Process rejected the event (body was not "OK"), event must be re-queued.
    Rejected,
    /// Protocol error occurred while parsing `RESULT` header or body.
    ProtocolError,
}

/// Evaluates a `RESULT` payload against the default result handler (`supervisor.dispatchers:default_handler`).
/// Returns `ProcessResult::Ok` if body is "OK", `ProcessResult::Rejected` otherwise.
pub fn evaluate_result(body: &str) -> ProcessResult {
    if body.trim() == "OK" {
        ProcessResult::Ok
    } else {
        ProcessResult::Rejected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_envelope_formatting() {
        let env = EventEnvelope::new(0, 0, "listener", "TICK_5", "when:1700000000");
        let wire = env.format_wire("rsupervisord-compat");
        assert_eq!(
            wire,
            "ver:3.0 server:rsupervisord-compat serial:0 pool:listener poolserial:0 eventname:TICK_5 len:15\nwhen:1700000000"
        );
    }

    #[test]
    fn test_evaluate_result() {
        assert_eq!(evaluate_result("OK"), ProcessResult::Ok);
        assert_eq!(evaluate_result("OK\n"), ProcessResult::Ok);
        assert_eq!(evaluate_result("FAIL"), ProcessResult::Rejected);
    }
}
