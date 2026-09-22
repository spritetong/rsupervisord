// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::program::state::ProgramState;

/// Converts a `ProgramState` enum into its standard supervisor state string.
pub fn state_to_supervisor_name(state: ProgramState) -> &'static str {
    match state {
        ProgramState::Stopped => "STOPPED",
        ProgramState::Starting => "STARTING",
        ProgramState::Running => "RUNNING",
        ProgramState::Backoff => "BACKOFF",
        ProgramState::Stopping => "STOPPING",
        ProgramState::Exited => "EXITED",
        ProgramState::Fatal => "FATAL",
    }
}

/// Generates the standard supervisor event name and payload for a process state transition.
pub fn format_process_state_event(
    process_name: &str,
    group_name: &str,
    old_state: ProgramState,
    new_state: ProgramState,
    pid: Option<u32>,
    expected: bool,
    tries: u32,
) -> (&'static str, String) {
    let from_state = state_to_supervisor_name(old_state);
    let effective_pid = pid.unwrap_or(0);

    match new_state {
        ProgramState::Starting => (
            "PROCESS_STATE_STARTING",
            format!(
                "processname:{} groupname:{} from_state:{} tries:{}",
                process_name, group_name, from_state, tries
            ),
        ),
        ProgramState::Backoff => (
            "PROCESS_STATE_BACKOFF",
            format!(
                "processname:{} groupname:{} from_state:{} tries:{}",
                process_name, group_name, from_state, tries
            ),
        ),
        ProgramState::Running => (
            "PROCESS_STATE_RUNNING",
            format!(
                "processname:{} groupname:{} from_state:{} pid:{}",
                process_name, group_name, from_state, effective_pid
            ),
        ),
        ProgramState::Stopping => (
            "PROCESS_STATE_STOPPING",
            format!(
                "processname:{} groupname:{} from_state:{} pid:{}",
                process_name, group_name, from_state, effective_pid
            ),
        ),
        ProgramState::Stopped => (
            "PROCESS_STATE_STOPPED",
            format!(
                "processname:{} groupname:{} from_state:{} pid:{}",
                process_name, group_name, from_state, effective_pid
            ),
        ),
        ProgramState::Exited => (
            "PROCESS_STATE_EXITED",
            format!(
                "processname:{} groupname:{} from_state:{} expected:{} pid:{}",
                process_name,
                group_name,
                from_state,
                if expected { 1 } else { 0 },
                effective_pid
            ),
        ),
        ProgramState::Fatal => (
            "PROCESS_STATE_FATAL",
            format!(
                "processname:{} groupname:{} from_state:{}",
                process_name, group_name, from_state
            ),
        ),
    }
}

/// Generates the standard supervisor event name and payload for a process log line.
pub fn format_process_log_event(
    process_name: &str,
    group_name: &str,
    pid: Option<u32>,
    stream: &str,
    data: &str,
) -> (&'static str, String) {
    let effective_pid = pid.unwrap_or(0);
    let channel = if stream == "stderr" {
        "stderr"
    } else {
        "stdout"
    };
    let event_name = if channel == "stderr" {
        "PROCESS_LOG_STDERR"
    } else {
        "PROCESS_LOG_STDOUT"
    };

    let payload = format!(
        "processname:{} groupname:{} pid:{} channel:{}\n{}",
        process_name, group_name, effective_pid, channel, data
    );

    (event_name, payload)
}

/// Generates the standard supervisor event name and payload for a tick event.
pub fn format_tick_event(interval: u32, when: u64) -> (String, String) {
    let event_name = format!("TICK_{}", interval);
    let payload = format!("when:{}", when);
    (event_name, payload)
}

/// Generates the standard supervisor event name and payload for a remote communication event.
pub fn format_remote_comm_event(type_str: &str, data: &str) -> (&'static str, String) {
    (
        "REMOTE_COMMUNICATION",
        format!("type:{}\n{}", type_str, data),
    )
}

/// Tests whether an event matches any subscription in the listener pool's subscription set.
pub fn event_matches_subscription(event_name: &str, subscribed: &[String]) -> bool {
    let event_upper = event_name.to_ascii_uppercase();
    for sub in subscribed {
        let sub_upper = sub.trim().to_ascii_uppercase();
        if sub_upper == "EVENT" || sub_upper == event_upper {
            return true;
        }
        let prefix = format!("{}_", sub_upper);
        if event_upper.starts_with(&prefix) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subscription_matching() {
        let subs = vec!["PROCESS_STATE".to_string(), "TICK_5".to_string()];
        assert!(event_matches_subscription("PROCESS_STATE_EXITED", &subs));
        assert!(event_matches_subscription("PROCESS_STATE_RUNNING", &subs));
        assert!(event_matches_subscription("TICK_5", &subs));
        assert!(!event_matches_subscription("TICK_60", &subs));
        assert!(!event_matches_subscription("PROCESS_LOG_STDOUT", &subs));

        let subs_event = vec!["EVENT".to_string()];
        assert!(event_matches_subscription("ANY_EVENT", &subs_event));
    }

    #[test]
    fn test_format_process_state_event() {
        let (name, payload) = format_process_state_event(
            "echo",
            "services",
            ProgramState::Running,
            ProgramState::Exited,
            Some(1234),
            true,
            0,
        );
        assert_eq!(name, "PROCESS_STATE_EXITED");
        assert_eq!(
            payload,
            "processname:echo groupname:services from_state:RUNNING expected:1 pid:1234"
        );
    }
}
