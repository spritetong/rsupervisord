// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::compat::xmlrpc::wire::Value;
use crate::program::state::{ProgramState, ProgramStatus};
use chrono::{Local, TimeZone};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// Supervisor process state codes (Python ProcessStates).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ProcessStateCode {
    Stopped = 0,
    Starting = 10,
    Running = 20,
    Backoff = 30,
    Stopping = 40,
    Exited = 100,
    Fatal = 200,
    Unknown = 1000,
}

impl ProcessStateCode {
    pub const fn from_program_state(state: ProgramState) -> (i32, &'static str) {
        match state {
            ProgramState::Stopped => (0, "STOPPED"),
            ProgramState::Starting => (10, "STARTING"),
            ProgramState::Running => (20, "RUNNING"),
            ProgramState::Backoff => (30, "BACKOFF"),
            ProgramState::Stopping => (40, "STOPPING"),
            ProgramState::Exited => (100, "EXITED"),
            ProgramState::Fatal => (200, "FATAL"),
        }
    }
}

/// Saturated integer conversion for Unix epoch seconds (protecting 32-bit i4 values).
#[inline]
pub fn saturate_i4(timestamp: u64) -> i32 {
    if timestamp > (i32::MAX as u64) {
        i32::MAX
    } else {
        timestamp as i32
    }
}

/// Formats description string according to Python Supervisor `_interpretProcessInfo`.
pub fn format_description(
    state: ProgramState,
    pid: Option<u32>,
    uptime_secs: Option<u64>,
    now_sec: u64,
) -> String {
    match state {
        ProgramState::Running => {
            let pid_val = pid.unwrap_or(0);
            let uptime = uptime_secs.unwrap_or(0);
            format!("pid {}, uptime {}", pid_val, format_uptime(uptime))
        }
        ProgramState::Fatal | ProgramState::Backoff => "unknown error (try \"tail\")".to_string(),
        ProgramState::Stopped | ProgramState::Exited => {
            if let Some(up) = uptime_secs {
                let start_sec = now_sec.saturating_sub(up) as i64;
                if let Some(dt) = Local.timestamp_opt(start_sec, 0).single() {
                    dt.format("%b %d %I:%M %p").to_string()
                } else {
                    "Not started".to_string()
                }
            } else {
                "Not started".to_string()
            }
        }
        _ => String::new(),
    }
}

fn format_uptime(uptime_secs: u64) -> String {
    let days = uptime_secs / 86400;
    let rem = uptime_secs % 86400;
    let hours = rem / 3600;
    let mins = (rem % 3600) / 60;
    let secs = rem % 60;

    if days > 0 {
        format!("{} days, {}:{:02}:{:02}", days, hours, mins, secs)
    } else {
        format!("{}:{:02}:{:02}", hours, mins, secs)
    }
}

/// Converts an internal ProgramStatus into the standard Python Supervisor getProcessInfo struct.
pub fn program_status_to_process_info(
    status: &ProgramStatus,
    stdout_logfile: Option<&str>,
    stderr_logfile: Option<&str>,
) -> Value {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let (state_code, state_name) = ProcessStateCode::from_program_state(status.state);

    let start_sec = if let Some(up) = status.uptime_secs {
        saturate_i4(now.saturating_sub(up))
    } else {
        0
    };

    let stop_sec = if status.state.is_running() {
        0
    } else {
        saturate_i4(now)
    };

    let stdout_path = stdout_logfile.unwrap_or("").to_string();
    let stderr_path = stderr_logfile.unwrap_or("").to_string();
    let spawnerr = String::new();

    let desc = format_description(status.state, status.pid, status.uptime_secs, now);

    let mut map = BTreeMap::new();
    map.insert("name".to_string(), Value::String(status.name.clone()));
    map.insert("group".to_string(), Value::String(status.group.clone()));
    map.insert("description".to_string(), Value::String(desc));
    map.insert("start".to_string(), Value::Int(start_sec));
    map.insert("stop".to_string(), Value::Int(stop_sec));
    map.insert("now".to_string(), Value::Int(saturate_i4(now)));
    map.insert("state".to_string(), Value::Int(state_code));
    map.insert(
        "statename".to_string(),
        Value::String(state_name.to_string()),
    );
    map.insert("spawnerr".to_string(), Value::String(spawnerr));
    map.insert(
        "exitstatus".to_string(),
        Value::Int(status.exit_code.unwrap_or(0)),
    );
    map.insert("logfile".to_string(), Value::String(stdout_path.clone()));
    map.insert("stdout_logfile".to_string(), Value::String(stdout_path));
    map.insert("stderr_logfile".to_string(), Value::String(stderr_path));
    map.insert(
        "pid".to_string(),
        Value::Int(status.pid.unwrap_or(0) as i32),
    );

    Value::Struct(map)
}

/// Process status response struct used for batch operations.
pub fn make_process_status_struct(
    name: &str,
    group: &str,
    status_code: i32,
    description: &str,
) -> Value {
    let mut map = BTreeMap::new();
    map.insert("name".to_string(), Value::String(name.to_string()));
    map.insert("group".to_string(), Value::String(group.to_string()));
    map.insert("status".to_string(), Value::Int(status_code));
    map.insert(
        "description".to_string(),
        Value::String(description.to_string()),
    );
    Value::Struct(map)
}

/// Parses a Supervisor namespec (e.g. `group:name` or `name` or `group:*`).
pub fn parse_namespec(spec: &str) -> (Option<&str>, &str) {
    let trimmed = spec.trim();
    if let Some((group, prog)) = trimmed.split_once(':') {
        (Some(group), prog)
    } else {
        (None, trimmed)
    }
}
