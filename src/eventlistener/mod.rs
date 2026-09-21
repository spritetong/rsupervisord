// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

pub mod config;
pub mod events;
pub mod pool;
pub mod program;
pub mod protocol;

pub use config::{
    EventListenerConfig, EventListenerConfigRaw, is_valid_event_type, validate_event_list,
};
pub use events::{
    event_matches_subscription, format_process_log_event, format_process_state_event,
};
pub use pool::{EventListenerPool, EventListenerPoolInner};
pub use program::EventListenerProgram;
pub use protocol::{
    EventEnvelope, ListenerState, ProcessResult, evaluate_result, next_global_serial,
};
