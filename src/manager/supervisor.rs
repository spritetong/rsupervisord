// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::config::diff::ConfigDiff;
use crate::config::schema::SupervisorConfig;
use crate::consts::*;
use crate::error::ProgramError;
use crate::eventlistener::pool::EventListenerPool;
use crate::eventlistener::program::EventListenerProgram;
use crate::logging::InMemoryChannelRotator;
use crate::manager::dag::DependencyGraph;
use crate::program::config::{ProgramConfig, StopSignal};
use crate::program::process::ProcessProgram;
use crate::program::state::{ProgramState, ProgramStatus};
use crate::program::traits::Program;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReloadSummary {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub modified: Vec<String>,
    pub unchanged: Vec<String>,
}

#[derive(Debug)]
pub enum ManagerCommand {
    StartProgram {
        name: String,
        reply: oneshot::Sender<Result<(), ProgramError>>,
    },
    StopProgram {
        name: String,
        grace_period: Option<Duration>,
        reply: oneshot::Sender<Result<(), ProgramError>>,
    },
    RestartProgram {
        name: String,
        grace_period: Option<Duration>,
        reply: oneshot::Sender<Result<(), ProgramError>>,
    },
    SignalProgram {
        name: String,
        signal: StopSignal,
        reply: oneshot::Sender<Result<(), ProgramError>>,
    },
    StartGroup {
        group: String,
        reply: oneshot::Sender<Result<Vec<String>, ProgramError>>,
    },
    StopGroup {
        group: String,
        grace_period: Option<Duration>,
        reply: oneshot::Sender<Result<Vec<String>, ProgramError>>,
    },
    RestartGroup {
        group: String,
        grace_period: Option<Duration>,
        reply: oneshot::Sender<Result<Vec<String>, ProgramError>>,
    },
    StartAll {
        reply: oneshot::Sender<Result<(), ProgramError>>,
    },
    StopAll {
        grace_period: Option<Duration>,
        reply: oneshot::Sender<Result<(), ProgramError>>,
    },
    ReloadConfig {
        new_config: Box<SupervisorConfig>,
        reply: oneshot::Sender<Result<ReloadSummary, ProgramError>>,
    },
    RestartDaemon {
        new_config: Box<SupervisorConfig>,
        reply: oneshot::Sender<Result<(), ProgramError>>,
    },
    AddProcessGroup {
        name: String,
        reply: oneshot::Sender<Result<bool, ProgramError>>,
    },
    RemoveProcessGroup {
        name: String,
        reply: oneshot::Sender<Result<bool, ProgramError>>,
    },
    GetStatus {
        name: String,
        reply: oneshot::Sender<Result<ProgramStatus, ProgramError>>,
    },
    GetAllStatus {
        reply: oneshot::Sender<Vec<ProgramStatus>>,
    },
    ReadLogs {
        name: String,
        lines: Option<usize>,
        reply: oneshot::Sender<Result<Vec<String>, ProgramError>>,
    },
    SubscribeLogs {
        name: String,
        reply: oneshot::Sender<Result<tokio::sync::broadcast::Receiver<String>, ProgramError>>,
    },
    ReadLog {
        name: String,
        channel: crate::logging::LogChannel,
        offset: i64,
        length: i64,
        reply: oneshot::Sender<Result<(String, i64, bool), ProgramError>>,
    },
    TailLog {
        name: String,
        channel: crate::logging::LogChannel,
        offset: i64,
        length: i64,
        reply: oneshot::Sender<Result<(String, i64, bool), ProgramError>>,
    },
    ClearProcessLogs {
        name: String,
        reply: oneshot::Sender<Result<(), ProgramError>>,
    },
    ReadMainLog {
        offset: i64,
        length: i64,
        reply: oneshot::Sender<Result<String, ProgramError>>,
    },
    TailMainLog {
        offset: i64,
        length: i64,
        reply: oneshot::Sender<Result<(String, i64, bool), ProgramError>>,
    },
    ClearMainLog {
        reply: oneshot::Sender<Result<(), ProgramError>>,
    },
    SendStdin {
        name: String,
        data: Vec<u8>,
        reply: oneshot::Sender<Result<(), ProgramError>>,
    },
    GetConfig {
        name: String,
        reply: oneshot::Sender<Result<ProgramConfig, ProgramError>>,
    },
    GetAllConfigs {
        reply: oneshot::Sender<HashMap<String, ProgramConfig>>,
    },
    Shutdown {
        reply: oneshot::Sender<Result<(), ProgramError>>,
    },
}

#[derive(Clone)]
pub struct ManagerHandle {
    command_tx: mpsc::Sender<ManagerCommand>,
    cancel_token: CancellationToken,
    activity_tracker: crate::manager::ActivityTracker,
    event_hub: crate::manager::EventHub,
    server_identifier: String,
}

impl ManagerHandle {
    #[doc(hidden)]
    pub fn new_mock_for_test(
        command_tx: mpsc::Sender<ManagerCommand>,
        cancel_token: CancellationToken,
    ) -> Self {
        Self {
            command_tx,
            cancel_token,
            activity_tracker: crate::manager::ActivityTracker::default(),
            event_hub: crate::manager::EventHub::default(),
            server_identifier: "rsupervisord-compat".to_string(),
        }
    }

    pub fn server_identifier(&self) -> &str {
        &self.server_identifier
    }

    pub fn activity_tracker(&self) -> &crate::manager::ActivityTracker {
        &self.activity_tracker
    }

    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel_token.clone()
    }

    pub fn event_hub(&self) -> &crate::manager::EventHub {
        &self.event_hub
    }

    pub fn subscribe_events(
        &self,
    ) -> tokio::sync::broadcast::Receiver<crate::manager::SystemEvent> {
        self.event_hub.subscribe_system()
    }

    pub fn subscribe_all_logs(&self) -> tokio::sync::broadcast::Receiver<crate::manager::LogEntry> {
        self.event_hub.subscribe_logs()
    }

    pub fn send_remote_comm_event(&self, type_str: &str, data: &str) -> bool {
        self.event_hub
            .publish_system(crate::manager::SystemEvent::RemoteCommunication {
                type_str: type_str.to_string(),
                data: data.to_string(),
            });
        true
    }

    pub async fn start_program(&self, name: impl Into<String>) -> Result<(), ProgramError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ManagerCommand::StartProgram {
                name: name.into(),
                reply: reply_tx,
            })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?;

        let timeout_dur = AWAIT_ACTION;
        tokio::time::timeout(timeout_dur, reply_rx)
            .await
            .map_err(|_| ProgramError::Timeout {
                name: "manager".to_string(),
                timeout_secs: timeout_dur.as_secs(),
            })?
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?
    }

    pub async fn stop_program(
        &self,
        name: impl Into<String>,
        grace_period: Option<Duration>,
    ) -> Result<(), ProgramError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ManagerCommand::StopProgram {
                name: name.into(),
                grace_period,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?;

        let timeout_dur = grace_period
            .unwrap_or(DEFAULT_STOP_WAIT)
            .checked_add(DEFAULT_HOOK_TIMEOUT)
            .unwrap_or(MAX_TIMEOUT)
            .checked_add(DRAIN_TIMEOUT)
            .unwrap_or(MAX_TIMEOUT)
            .checked_add(STOP_GRACE_EXTRA)
            .unwrap_or(MAX_TIMEOUT);
        tokio::time::timeout(timeout_dur, reply_rx)
            .await
            .map_err(|_| ProgramError::Timeout {
                name: "manager".to_string(),
                timeout_secs: timeout_dur.as_secs(),
            })?
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?
    }

    pub async fn restart_program(
        &self,
        name: impl Into<String>,
        grace_period: Option<Duration>,
    ) -> Result<(), ProgramError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ManagerCommand::RestartProgram {
                name: name.into(),
                grace_period,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?;

        let timeout_dur = grace_period
            .unwrap_or(DEFAULT_STOP_WAIT)
            .checked_add(DEFAULT_HOOK_TIMEOUT * 2)
            .unwrap_or(MAX_TIMEOUT)
            .checked_add(DRAIN_TIMEOUT)
            .unwrap_or(MAX_TIMEOUT)
            .checked_add(RESTART_GRACE_EXTRA)
            .unwrap_or(MAX_TIMEOUT);
        tokio::time::timeout(timeout_dur, reply_rx)
            .await
            .map_err(|_| ProgramError::Timeout {
                name: "manager".to_string(),
                timeout_secs: timeout_dur.as_secs(),
            })?
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?
    }

    pub async fn signal_program(
        &self,
        name: impl Into<String>,
        signal: StopSignal,
    ) -> Result<(), ProgramError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ManagerCommand::SignalProgram {
                name: name.into(),
                signal,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?;

        let timeout_dur = AWAIT_QUERY;
        tokio::time::timeout(timeout_dur, reply_rx)
            .await
            .map_err(|_| ProgramError::Timeout {
                name: "manager".to_string(),
                timeout_secs: timeout_dur.as_secs(),
            })?
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?
    }

    pub async fn start_group(&self, group: impl Into<String>) -> Result<Vec<String>, ProgramError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ManagerCommand::StartGroup {
                group: group.into(),
                reply: reply_tx,
            })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?;

        let timeout_dur = AWAIT_GROUP;
        tokio::time::timeout(timeout_dur, reply_rx)
            .await
            .map_err(|_| ProgramError::Timeout {
                name: "manager".to_string(),
                timeout_secs: timeout_dur.as_secs(),
            })?
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?
    }

    pub async fn stop_group(
        &self,
        group: impl Into<String>,
        grace_period: Option<Duration>,
    ) -> Result<Vec<String>, ProgramError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ManagerCommand::StopGroup {
                group: group.into(),
                grace_period,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?;

        let timeout_dur = AWAIT_GROUP;
        tokio::time::timeout(timeout_dur, reply_rx)
            .await
            .map_err(|_| ProgramError::Timeout {
                name: "manager".to_string(),
                timeout_secs: timeout_dur.as_secs(),
            })?
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?
    }

    pub async fn restart_group(
        &self,
        group: impl Into<String>,
        grace_period: Option<Duration>,
    ) -> Result<Vec<String>, ProgramError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ManagerCommand::RestartGroup {
                group: group.into(),
                grace_period,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?;

        let timeout_dur = AWAIT_BULK;
        tokio::time::timeout(timeout_dur, reply_rx)
            .await
            .map_err(|_| ProgramError::Timeout {
                name: "manager".to_string(),
                timeout_secs: timeout_dur.as_secs(),
            })?
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?
    }

    pub async fn start_all(&self) -> Result<(), ProgramError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ManagerCommand::StartAll { reply: reply_tx })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?;

        let timeout_dur = AWAIT_BULK;
        tokio::time::timeout(timeout_dur, reply_rx)
            .await
            .map_err(|_| ProgramError::Timeout {
                name: "manager".to_string(),
                timeout_secs: timeout_dur.as_secs(),
            })?
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?
    }

    pub async fn stop_all(&self, grace_period: Option<Duration>) -> Result<(), ProgramError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ManagerCommand::StopAll {
                grace_period,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?;

        let timeout_dur = AWAIT_BULK;
        tokio::time::timeout(timeout_dur, reply_rx)
            .await
            .map_err(|_| ProgramError::Timeout {
                name: "manager".to_string(),
                timeout_secs: timeout_dur.as_secs(),
            })?
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?
    }

    pub async fn reload_config(
        &self,
        new_config: SupervisorConfig,
    ) -> Result<ReloadSummary, ProgramError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ManagerCommand::ReloadConfig {
                new_config: Box::new(new_config),
                reply: reply_tx,
            })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?;

        let timeout_dur = AWAIT_GROUP;
        tokio::time::timeout(timeout_dur, reply_rx)
            .await
            .map_err(|_| ProgramError::Timeout {
                name: "manager".to_string(),
                timeout_secs: timeout_dur.as_secs(),
            })?
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?
    }

    pub async fn restart_daemon(&self, new_config: SupervisorConfig) -> Result<(), ProgramError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ManagerCommand::RestartDaemon {
                new_config: Box::new(new_config),
                reply: reply_tx,
            })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?;

        let timeout_dur = AWAIT_BULK;
        tokio::time::timeout(timeout_dur, reply_rx)
            .await
            .map_err(|_| ProgramError::Timeout {
                name: "manager".to_string(),
                timeout_secs: timeout_dur.as_secs(),
            })?
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?
    }

    pub async fn add_process_group(&self, name: &str) -> Result<bool, ProgramError> {
        self.activity_tracker.record_activity();
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ManagerCommand::AddProcessGroup {
                name: name.to_string(),
                reply: reply_tx,
            })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?;

        let timeout_dur = AWAIT_GROUP;
        tokio::time::timeout(timeout_dur, reply_rx)
            .await
            .map_err(|_| ProgramError::Timeout {
                name: "manager".to_string(),
                timeout_secs: timeout_dur.as_secs(),
            })?
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?
    }

    pub async fn remove_process_group(&self, name: &str) -> Result<bool, ProgramError> {
        self.activity_tracker.record_activity();
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ManagerCommand::RemoveProcessGroup {
                name: name.to_string(),
                reply: reply_tx,
            })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?;

        let timeout_dur = AWAIT_BULK;
        tokio::time::timeout(timeout_dur, reply_rx)
            .await
            .map_err(|_| ProgramError::Timeout {
                name: "manager".to_string(),
                timeout_secs: timeout_dur.as_secs(),
            })?
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?
    }

    pub async fn get_status(&self, name: &str) -> Result<ProgramStatus, ProgramError> {
        self.activity_tracker.record_activity();
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ManagerCommand::GetStatus {
                name: name.to_string(),
                reply: reply_tx,
            })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?;

        let timeout_dur = AWAIT_QUERY;
        tokio::time::timeout(timeout_dur, reply_rx)
            .await
            .map_err(|_| ProgramError::Timeout {
                name: "manager".to_string(),
                timeout_secs: timeout_dur.as_secs(),
            })?
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?
    }

    pub async fn get_all_status(&self) -> Result<Vec<ProgramStatus>, ProgramError> {
        self.activity_tracker.record_activity();
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ManagerCommand::GetAllStatus { reply: reply_tx })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?;

        let timeout_dur = AWAIT_QUERY;
        tokio::time::timeout(timeout_dur, reply_rx)
            .await
            .map_err(|_| ProgramError::Timeout {
                name: "manager".to_string(),
                timeout_secs: timeout_dur.as_secs(),
            })?
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })
    }

    pub async fn read_logs(
        &self,
        name: impl Into<String>,
        lines: Option<usize>,
    ) -> Result<Vec<String>, ProgramError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ManagerCommand::ReadLogs {
                name: name.into(),
                lines,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?;

        let timeout_dur = AWAIT_QUERY;
        tokio::time::timeout(timeout_dur, reply_rx)
            .await
            .map_err(|_| ProgramError::Timeout {
                name: "manager".to_string(),
                timeout_secs: timeout_dur.as_secs(),
            })?
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?
    }

    pub async fn subscribe_logs(
        &self,
        name: impl Into<String>,
    ) -> Result<tokio::sync::broadcast::Receiver<String>, ProgramError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ManagerCommand::SubscribeLogs {
                name: name.into(),
                reply: reply_tx,
            })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?;

        let timeout_dur = AWAIT_QUERY;
        tokio::time::timeout(timeout_dur, reply_rx)
            .await
            .map_err(|_| ProgramError::Timeout {
                name: "manager".to_string(),
                timeout_secs: timeout_dur.as_secs(),
            })?
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?
    }

    pub async fn read_log(
        &self,
        name: impl Into<String>,
        channel: crate::logging::LogChannel,
        offset: i64,
        length: i64,
    ) -> Result<(String, i64, bool), ProgramError> {
        let name_str = name.into();
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ManagerCommand::ReadLog {
                name: name_str.clone(),
                channel,
                offset,
                length,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: name_str.clone(),
            })?;

        let timeout_dur = AWAIT_QUERY;
        tokio::time::timeout(timeout_dur, reply_rx)
            .await
            .map_err(|_| ProgramError::Timeout {
                name: name_str.clone(),
                timeout_secs: timeout_dur.as_secs(),
            })?
            .map_err(|_| ProgramError::ChannelClosed { name: name_str })?
    }

    pub async fn tail_log(
        &self,
        name: impl Into<String>,
        channel: crate::logging::LogChannel,
        offset: i64,
        length: i64,
    ) -> Result<(String, i64, bool), ProgramError> {
        let name_str = name.into();
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ManagerCommand::TailLog {
                name: name_str.clone(),
                channel,
                offset,
                length,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: name_str.clone(),
            })?;

        let timeout_dur = AWAIT_QUERY;
        tokio::time::timeout(timeout_dur, reply_rx)
            .await
            .map_err(|_| ProgramError::Timeout {
                name: name_str.clone(),
                timeout_secs: timeout_dur.as_secs(),
            })?
            .map_err(|_| ProgramError::ChannelClosed { name: name_str })?
    }

    pub async fn clear_process_logs(&self, name: impl Into<String>) -> Result<(), ProgramError> {
        let name_str = name.into();
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ManagerCommand::ClearProcessLogs {
                name: name_str.clone(),
                reply: reply_tx,
            })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: name_str.clone(),
            })?;

        let timeout_dur = AWAIT_QUERY;
        tokio::time::timeout(timeout_dur, reply_rx)
            .await
            .map_err(|_| ProgramError::Timeout {
                name: name_str.clone(),
                timeout_secs: timeout_dur.as_secs(),
            })?
            .map_err(|_| ProgramError::ChannelClosed { name: name_str })?
    }

    pub async fn read_main_log(&self, offset: i64, length: i64) -> Result<String, ProgramError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ManagerCommand::ReadMainLog {
                offset,
                length,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?;

        let timeout_dur = AWAIT_QUERY;
        tokio::time::timeout(timeout_dur, reply_rx)
            .await
            .map_err(|_| ProgramError::Timeout {
                name: "manager".to_string(),
                timeout_secs: timeout_dur.as_secs(),
            })?
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?
    }

    pub async fn tail_main_log(
        &self,
        offset: i64,
        length: i64,
    ) -> Result<(String, i64, bool), ProgramError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ManagerCommand::TailMainLog {
                offset,
                length,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?;

        let timeout_dur = AWAIT_QUERY;
        tokio::time::timeout(timeout_dur, reply_rx)
            .await
            .map_err(|_| ProgramError::Timeout {
                name: "manager".to_string(),
                timeout_secs: timeout_dur.as_secs(),
            })?
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?
    }

    pub async fn clear_main_log(&self) -> Result<(), ProgramError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ManagerCommand::ClearMainLog { reply: reply_tx })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?;

        let timeout_dur = AWAIT_QUERY;
        tokio::time::timeout(timeout_dur, reply_rx)
            .await
            .map_err(|_| ProgramError::Timeout {
                name: "manager".to_string(),
                timeout_secs: timeout_dur.as_secs(),
            })?
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?
    }

    pub async fn send_stdin(
        &self,
        name: impl Into<String>,
        data: Vec<u8>,
    ) -> Result<(), ProgramError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ManagerCommand::SendStdin {
                name: name.into(),
                data,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?;

        let timeout_dur = AWAIT_STDIN;
        tokio::time::timeout(timeout_dur, reply_rx)
            .await
            .map_err(|_| ProgramError::Timeout {
                name: "manager".to_string(),
                timeout_secs: timeout_dur.as_secs(),
            })?
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?
    }

    pub async fn get_config(&self, name: impl Into<String>) -> Result<ProgramConfig, ProgramError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ManagerCommand::GetConfig {
                name: name.into(),
                reply: reply_tx,
            })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?;

        let timeout_dur = AWAIT_QUERY;
        tokio::time::timeout(timeout_dur, reply_rx)
            .await
            .map_err(|_| ProgramError::Timeout {
                name: "manager".to_string(),
                timeout_secs: timeout_dur.as_secs(),
            })?
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?
    }

    pub async fn get_all_configs(&self) -> Result<HashMap<String, ProgramConfig>, ProgramError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ManagerCommand::GetAllConfigs { reply: reply_tx })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?;

        let timeout_dur = AWAIT_QUERY;
        tokio::time::timeout(timeout_dur, reply_rx)
            .await
            .map_err(|_| ProgramError::Timeout {
                name: "manager".to_string(),
                timeout_secs: timeout_dur.as_secs(),
            })?
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })
    }

    pub async fn shutdown(&self) -> Result<(), ProgramError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let _ = self
            .command_tx
            .send(ManagerCommand::Shutdown { reply: reply_tx })
            .await;

        let timeout_dur = AWAIT_BULK;
        let _ = tokio::time::timeout(timeout_dur, reply_rx).await;
        self.cancel_token.cancel();
        Ok(())
    }
}

/// Fluent builder for constructing and starting a SupervisorManager.
pub struct SupervisorManagerBuilder {
    config: SupervisorConfig,
    activity_tracker: Option<crate::manager::ActivityTracker>,
    event_hub: Option<crate::manager::EventHub>,
    cancel_token: Option<CancellationToken>,
    main_log_rotator: Option<Arc<InMemoryChannelRotator>>,
    main_file_rotator: Option<Arc<crate::logging::LogRotator>>,
}

impl SupervisorManagerBuilder {
    pub fn new(config: SupervisorConfig) -> Self {
        Self {
            config,
            activity_tracker: None,
            event_hub: None,
            cancel_token: None,
            main_log_rotator: None,
            main_file_rotator: None,
        }
    }

    pub fn with_activity_tracker(mut self, tracker: crate::manager::ActivityTracker) -> Self {
        self.activity_tracker = Some(tracker);
        self
    }

    pub fn with_event_hub(mut self, hub: crate::manager::EventHub) -> Self {
        self.event_hub = Some(hub);
        self
    }

    pub fn with_cancel_token(mut self, token: CancellationToken) -> Self {
        self.cancel_token = Some(token);
        self
    }

    pub fn with_main_log_rotator(mut self, rotator: Option<Arc<InMemoryChannelRotator>>) -> Self {
        self.main_log_rotator = rotator;
        self
    }

    pub fn with_main_file_rotator(
        mut self,
        rotator: Option<Arc<crate::logging::LogRotator>>,
    ) -> Self {
        self.main_file_rotator = rotator;
        self
    }

    pub fn build(self) -> Result<SupervisorManager, ProgramError> {
        let programs_map = self.config.resolve_programs()?;
        let dag = DependencyGraph::build(&programs_map)?;

        let activity_tracker = self.activity_tracker.unwrap_or_else(|| {
            crate::manager::ActivityTracker::with_interval(
                self.config.metrics.idle_timeout_secs.as_secs(),
                self.config.metrics.interval_secs.as_secs(),
                self.config.metrics.enabled,
            )
        });
        let event_hub = self.event_hub.unwrap_or_default();

        let cancel_token = self.cancel_token.unwrap_or_default();
        let (command_tx, command_rx) = mpsc::channel(64);
        let cron_table = crate::manager::cron::CronTable::from_configs(&programs_map);

        let server_identifier = self
            .config
            .server
            .identifier
            .as_deref()
            .unwrap_or("rsupervisord-compat")
            .to_string();

        let handle = ManagerHandle {
            command_tx,
            cancel_token: cancel_token.clone(),
            activity_tracker: activity_tracker.clone(),
            event_hub: event_hub.clone(),
            server_identifier: server_identifier.clone(),
        };

        let mut event_pools = HashMap::new();
        let mut programs = HashMap::new();
        for (name, cfg) in &programs_map {
            let prog = instantiate_program(
                cfg,
                &server_identifier,
                &activity_tracker,
                &event_hub,
                &cancel_token,
                &mut event_pools,
            )?;
            programs.insert(name.clone(), prog);
        }

        // Spawn periodic ticker task for TICK_5, TICK_60, TICK_3600
        let ticker_cancel = cancel_token.clone();
        let ticker_hub = event_hub.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(1));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut last_5 = 0u64;
            let mut last_60 = 0u64;
            let mut last_3600 = 0u64;

            loop {
                tokio::select! {
                    _ = ticker_cancel.cancelled() => break,
                    _ = ticker.tick() => {
                        let now = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
                            Ok(d) => d.as_secs(),
                            Err(_) => 0,
                        };

                        let sec_5 = now / 5;
                        if sec_5 != last_5 {
                            last_5 = sec_5;
                            ticker_hub.publish_system(crate::manager::SystemEvent::Tick {
                                interval: 5,
                                when: now,
                            });
                        }

                        let sec_60 = now / 60;
                        if sec_60 != last_60 {
                            last_60 = sec_60;
                            ticker_hub.publish_system(crate::manager::SystemEvent::Tick {
                                interval: 60,
                                when: now,
                            });
                        }

                        let sec_3600 = now / 3600;
                        if sec_3600 != last_3600 {
                            last_3600 = sec_3600;
                            ticker_hub.publish_system(crate::manager::SystemEvent::Tick {
                                interval: 3600,
                                when: now,
                            });
                        }
                    }
                }
            }
        });

        let watch_handle = crate::manager::WatchService::spawn(
            handle.clone(),
            programs_map.clone(),
            cancel_token.clone(),
        );

        // Track the newest fully-parsed config as the pending/source config; this mirrors
        // Python Supervisor's `process_group_configs` used by addProcessGroup/removeProcessGroup.
        let pending_configs = programs_map.clone();

        let main_log_rotator = self
            .main_log_rotator
            .or_else(|| self.config.logging.build_main_rotator());
        let main_file_rotator = self.main_file_rotator;

        let actor = ManagerActor {
            programs,
            configs: programs_map,
            pending_configs,
            dag,
            cron_table,
            watch_handle,
            command_rx,
            activity_tracker,
            event_hub,
            cancel_token: cancel_token.clone(),
            is_shutting_down: false,
            event_pools,
            server_identifier,
            main_log_rotator: main_log_rotator.clone(),
            main_file_rotator: main_file_rotator.clone(),
            config_logging: self.config.logging.clone(),
        };

        let actor_handle = tokio::spawn(actor.run());
        let cancel_guard = cancel_token.drop_guard();

        Ok(SupervisorManager {
            handle,
            actor_handle: Some(actor_handle),
            main_log_rotator,
            main_file_rotator,
            _cancel_guard: cancel_guard,
        })
    }
}

pub struct SupervisorManager {
    handle: ManagerHandle,
    actor_handle: Option<JoinHandle<()>>,
    main_log_rotator: Option<Arc<InMemoryChannelRotator>>,
    main_file_rotator: Option<Arc<crate::logging::LogRotator>>,
    _cancel_guard: tokio_util::sync::DropGuard,
}

impl SupervisorManager {
    /// Returns a fluent builder for configuring and spawning a SupervisorManager.
    pub fn builder(config: SupervisorConfig) -> SupervisorManagerBuilder {
        SupervisorManagerBuilder::new(config)
    }

    pub fn new(initial_config: &SupervisorConfig) -> Result<Self, ProgramError> {
        Self::builder(initial_config.clone()).build()
    }

    pub fn handle(&self) -> ManagerHandle {
        self.handle.clone()
    }

    pub fn main_log_rotator(&self) -> Option<Arc<InMemoryChannelRotator>> {
        self.main_log_rotator.clone()
    }

    pub fn main_file_rotator(&self) -> Option<Arc<crate::logging::LogRotator>> {
        self.main_file_rotator.clone()
    }

    pub async fn reload_config(
        &self,
        new_config: SupervisorConfig,
    ) -> Result<ReloadSummary, ProgramError> {
        self.handle.reload_config(new_config).await
    }

    pub async fn restart_daemon(&self, new_config: SupervisorConfig) -> Result<(), ProgramError> {
        self.handle.restart_daemon(new_config).await
    }

    pub async fn shutdown(&mut self) -> Result<(), ProgramError> {
        let _ = self.handle.shutdown().await;
        if let Some(mut handle) = self.actor_handle.take() {
            if tokio::time::timeout(crate::consts::MANAGER_SHUTDOWN_TIMEOUT, &mut handle)
                .await
                .is_err()
            {
                tracing::warn!("ManagerActor shutdown timed out after 30s; aborting handle");
                handle.abort();
            }
        }
        Ok(())
    }
}

struct ManagerActor {
    programs: HashMap<String, Box<dyn Program>>,
    configs: HashMap<String, ProgramConfig>,
    pending_configs: HashMap<String, ProgramConfig>,
    dag: DependencyGraph,
    cron_table: crate::manager::cron::CronTable,
    watch_handle: crate::manager::WatchServiceHandle,
    command_rx: mpsc::Receiver<ManagerCommand>,
    activity_tracker: crate::manager::ActivityTracker,
    event_hub: crate::manager::EventHub,
    cancel_token: CancellationToken,
    is_shutting_down: bool,
    event_pools: HashMap<String, EventListenerPool>,
    server_identifier: String,
    main_log_rotator: Option<Arc<InMemoryChannelRotator>>,
    main_file_rotator: Option<Arc<crate::logging::LogRotator>>,
    config_logging: crate::config::schema::LoggingConfig,
}

impl ManagerActor {
    async fn run(mut self) {
        loop {
            let next_cron_deadline = self.cron_table.earliest_deadline();

            tokio::select! {
                biased;

                _ = self.cancel_token.cancelled() => {
                    self.is_shutting_down = true;
                    self.execute_stop_all(None).await;
                    break;
                }

                _ = async {
                    match next_cron_deadline {
                        Some(instant) => tokio::time::sleep_until(instant).await,
                        None => std::future::pending().await,
                    }
                }, if next_cron_deadline.is_some() && !self.is_shutting_down => {
                    self.handle_due_cron_jobs().await;
                }

                Some(cmd) = self.command_rx.recv() => {
                    match cmd {
                        ManagerCommand::StartProgram { name, reply } => {
                            if self.is_shutting_down {
                                let _ = reply.send(Err(ProgramError::ShuttingDown { name: name.clone() }));
                            } else {
                                let res = self.execute_start_program(&name).await;
                                let _ = reply.send(res);
                            }
                        }
                        ManagerCommand::StopProgram { name, grace_period, reply } => {
                            let res = self.execute_stop_program(&name, grace_period).await;
                            let _ = reply.send(res);
                        }
                        ManagerCommand::RestartProgram { name, grace_period, reply } => {
                            if self.is_shutting_down {
                                let _ = reply.send(Err(ProgramError::ShuttingDown { name: name.clone() }));
                            } else {
                                let res = self.execute_restart_program(&name, grace_period).await;
                                let _ = reply.send(res);
                            }
                        }
                        ManagerCommand::SignalProgram { name, signal, reply } => {
                            if self.is_shutting_down {
                                let _ = reply.send(Err(ProgramError::ShuttingDown { name: name.clone() }));
                            } else {
                                let targets = if self.programs.contains_key(&name) {
                                    vec![name.clone()]
                                } else {
                                    self.find_match(&name)
                                };
                                if targets.is_empty() {
                                    let _ = reply.send(Err(ProgramError::NotFound { name }));
                                } else {
                                    let mut last_res = Ok(());
                                    for t in targets {
                                        if let Some(prog) = self.programs.get(&t) {
                                            last_res = prog.signal(signal).await;
                                            if last_res.is_err() {
                                                break;
                                            }
                                        }
                                    }
                                    let _ = reply.send(last_res);
                                }
                            }
                        }
                        ManagerCommand::StartGroup { group, reply } => {
                            if self.is_shutting_down {
                                let _ = reply.send(Err(ProgramError::ShuttingDown { name: format!("group '{}'", group) }));
                            } else {
                                let res = self.execute_start_group(&group).await;
                                let _ = reply.send(res);
                            }
                        }
                        ManagerCommand::StopGroup { group, grace_period, reply } => {
                            let res = self.execute_stop_group(&group, grace_period).await;
                            let _ = reply.send(res);
                        }
                        ManagerCommand::RestartGroup { group, grace_period, reply } => {
                            if self.is_shutting_down {
                                let _ = reply.send(Err(ProgramError::ShuttingDown { name: format!("group '{}'", group) }));
                            } else {
                                let res = self.execute_restart_group(&group, grace_period).await;
                                let _ = reply.send(res);
                            }
                        }
                        ManagerCommand::StartAll { reply } => {
                            if self.is_shutting_down {
                                let _ = reply.send(Err(ProgramError::ShuttingDown { name: "manager".to_string() }));
                            } else {
                                let res = self.execute_start_all().await;
                                let _ = reply.send(res);
                            }
                        }
                        ManagerCommand::StopAll { grace_period, reply } => {
                            self.execute_stop_all(grace_period).await;
                            let _ = reply.send(Ok(()));
                        }
                        ManagerCommand::ReloadConfig { new_config, reply } => {
                            if self.is_shutting_down {
                                let _ = reply.send(Err(ProgramError::ShuttingDown { name: "manager".to_string() }));
                            } else {
                                let res = self.execute_reload_config(*new_config).await;
                                let _ = reply.send(res);
                            }
                        }
                        ManagerCommand::RestartDaemon { new_config, reply } => {
                            if self.is_shutting_down {
                                let _ = reply.send(Err(ProgramError::ShuttingDown { name: "manager".to_string() }));
                            } else {
                                let res = self.execute_restart_daemon(*new_config).await;
                                let _ = reply.send(res);
                            }
                        }
                        ManagerCommand::AddProcessGroup { name, reply } => {
                            if self.is_shutting_down {
                                let _ = reply.send(Err(ProgramError::ShuttingDown { name: name.clone() }));
                            } else {
                                let res = self.execute_add_process_group(&name).await;
                                let _ = reply.send(res);
                            }
                        }
                        ManagerCommand::RemoveProcessGroup { name, reply } => {
                            if self.is_shutting_down {
                                let _ = reply.send(Err(ProgramError::ShuttingDown { name: name.clone() }));
                            } else {
                                let res = self.execute_remove_process_group(&name).await;
                                let _ = reply.send(res);
                            }
                        }
                        ManagerCommand::GetStatus { name, reply } => {
                            let target = if self.programs.contains_key(&name) {
                                Some(name.clone())
                            } else {
                                self.find_match(&name).into_iter().next()
                            };
                            let res = target
                                .and_then(|n| {
                                    self.programs.get(&n).map(|p| {
                                        let mut st = p.status();
                                        st.cron = self.cron_table.get_cron_expr(&n);
                                        st.next_cron_run = self.cron_table.get_next_run(&n);
                                        st
                                    })
                                })
                                .ok_or_else(|| ProgramError::NotFound { name: name.clone() });
                            let _ = reply.send(res);
                        }
                        ManagerCommand::GetAllStatus { reply } => {
                            let statuses = self
                                .programs
                                .iter()
                                .map(|(n, p)| {
                                    let mut st = p.status();
                                    st.cron = self.cron_table.get_cron_expr(n);
                                    st.next_cron_run = self.cron_table.get_next_run(n);
                                    st
                                })
                                .collect();
                            let _ = reply.send(statuses);
                        }
                        ManagerCommand::ReadLogs { name, lines, reply } => {
                            let target = if self.programs.contains_key(&name) {
                                Some(name.clone())
                            } else {
                                self.find_match(&name).into_iter().next()
                            };
                            let res = target
                                .and_then(|n| self.programs.get(&n))
                                .map(|p| p.read_logs(lines))
                                .ok_or_else(|| ProgramError::NotFound { name: name.clone() });
                            let _ = reply.send(res);
                        }
                        ManagerCommand::SubscribeLogs { name, reply } => {
                            let target = if self.programs.contains_key(&name) {
                                Some(name.clone())
                            } else {
                                self.find_match(&name).into_iter().next()
                            };
                            let res = target
                                .and_then(|n| self.programs.get(&n))
                                .map(|p| p.subscribe_logs())
                                .ok_or_else(|| ProgramError::NotFound { name: name.clone() });
                            let _ = reply.send(res);
                        }
                        ManagerCommand::ReadLog { name, channel, offset, length, reply } => {
                            let target = if self.programs.contains_key(&name) {
                                Some(name.clone())
                            } else {
                                self.find_match(&name).into_iter().next()
                            };
                            let res = target
                                .and_then(|n| self.programs.get(&n))
                                .ok_or_else(|| ProgramError::NotFound { name: name.clone() })
                                .and_then(|p| p.read_log(channel, offset, length));
                            let _ = reply.send(res);
                        }
                        ManagerCommand::TailLog { name, channel, offset, length, reply } => {
                            let target = if self.programs.contains_key(&name) {
                                Some(name.clone())
                            } else {
                                self.find_match(&name).into_iter().next()
                            };
                            let res = target
                                .and_then(|n| self.programs.get(&n))
                                .ok_or_else(|| ProgramError::NotFound { name: name.clone() })
                                .and_then(|p| p.tail_log(channel, offset, length));
                            let _ = reply.send(res);
                        }
                        ManagerCommand::ClearProcessLogs { name, reply } => {
                            let target = if self.programs.contains_key(&name) {
                                Some(name.clone())
                            } else {
                                self.find_match(&name).into_iter().next()
                            };
                            let res = target
                                .and_then(|n| self.programs.get(&n))
                                .map(|p| p.clear_logs())
                                .ok_or_else(|| ProgramError::NotFound { name: name.clone() })
                                .and_then(|r| r);
                            let _ = reply.send(res);
                        }
                        ManagerCommand::ReadMainLog { offset, length, reply } => {
                            let res = self.execute_read_main_log(offset, length);
                            let _ = reply.send(res);
                        }
                        ManagerCommand::TailMainLog { offset, length, reply } => {
                            let res = self.execute_tail_main_log(offset, length);
                            let _ = reply.send(res);
                        }
                        ManagerCommand::ClearMainLog { reply } => {
                            let res = self.execute_clear_main_log();
                            let _ = reply.send(res);
                        }
                        ManagerCommand::SendStdin { name, data, reply } => {
                            let target = if self.programs.contains_key(&name) {
                                Some(name.clone())
                            } else {
                                self.find_match(&name).into_iter().next()
                            };
                            let res = if let Some(t) = target
                                && let Some(p) = self.programs.get(&t)
                            {
                                p.send_stdin(data).await
                            } else {
                                Err(ProgramError::NotFound { name })
                            };
                            let _ = reply.send(res);
                        }
                        ManagerCommand::GetConfig { name, reply } => {
                            let target = if self.configs.contains_key(&name) {
                                Some(name.clone())
                            } else {
                                self.find_match(&name).into_iter().next()
                            };
                            let res = target
                                .and_then(|n| self.configs.get(&n).cloned())
                                .ok_or(ProgramError::NotFound { name });
                            let _ = reply.send(res);
                        }
                        ManagerCommand::GetAllConfigs { reply } => {
                            let _ = reply.send(self.configs.clone());
                        }
                        ManagerCommand::Shutdown { reply } => {
                            self.is_shutting_down = true;
                            self.event_hub.publish_system(crate::manager::SystemEvent::DaemonLifecycle {
                                action: "shutting_down".to_string(),
                                timestamp_secs: std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map(|d| d.as_secs())
                                    .unwrap_or(0),
                            });
                            self.execute_stop_all(None).await;
                            let _ = reply.send(Ok(()));
                            break;
                        }
                    }
                }
            }
        }

        // Ensure all managed programs are cleanly shutdown before exiting actor loop
        for prog in self.programs.values_mut() {
            let _ = prog.shutdown().await;
        }
    }

    /// Matches programs according to standard supervisord syntax:
    /// - `name` matches program by direct name (including instance name like `worker:0`), or group by group name
    /// - `group:*` matches all programs in `group`
    /// - `group:program` matches specific program in `group`
    fn find_match(&self, pattern: &str) -> Vec<String> {
        let trimmed = pattern.trim();
        // 1. Exact match on program name first (e.g. direct instance name "worker:0" or "single_prog")
        if self.programs.contains_key(trimmed) {
            return vec![trimmed.to_string()];
        }

        // 2. Colon-separated pattern e.g. "group:*" or "group:name"
        if let Some((group_part, prog_part)) = trimmed.split_once(':') {
            let mut matches = Vec::new();
            for (p_name, cfg) in &self.configs {
                if cfg.group == group_part
                    && (prog_part == "*"
                        || prog_part == p_name
                        || p_name.strip_prefix(&format!("{}:", group_part)) == Some(prog_part))
                {
                    matches.push(p_name.clone());
                }
            }
            if !matches.is_empty() {
                return matches;
            }
        }

        // 3. Group match e.g. "worker" matches all instances with group == "worker"
        let matches: Vec<String> = self
            .configs
            .iter()
            .filter(|(_, cfg)| cfg.group == trimmed)
            .map(|(p_name, _)| p_name.clone())
            .collect();
        matches
    }

    async fn execute_start_group(&mut self, group: &str) -> Result<Vec<String>, ProgramError> {
        let mut targets = self.find_match(&format!("{}:*", group));
        if targets.is_empty() {
            targets = self.find_match(group);
        }
        if targets.is_empty() {
            return Err(ProgramError::NotFound {
                name: format!("group '{}'", group),
            });
        }
        targets.sort_by_key(|n| {
            self.configs
                .get(n)
                .map(|c| c.priority)
                .unwrap_or(DEFAULT_PRIORITY)
        });
        for name in &targets {
            if let Some(prog) = self.programs.get_mut(name) {
                prog.start().await?;
            }
        }
        Ok(targets)
    }

    async fn execute_stop_group(
        &mut self,
        group: &str,
        grace_period: Option<Duration>,
    ) -> Result<Vec<String>, ProgramError> {
        let mut targets = self.find_match(&format!("{}:*", group));
        if targets.is_empty() {
            targets = self.find_match(group);
        }
        if targets.is_empty() {
            return Err(ProgramError::NotFound {
                name: format!("group '{}'", group),
            });
        }
        targets.sort_by_key(|n| {
            self.configs
                .get(n)
                .map(|c| c.priority)
                .unwrap_or(DEFAULT_PRIORITY)
        });
        for name in targets.iter().rev() {
            let default_wait = self
                .configs
                .get(name)
                .map(|c| c.stop_wait_secs)
                .unwrap_or_else(|| DEFAULT_STOP_WAIT);
            let period = grace_period.unwrap_or(default_wait);
            if let Some(prog) = self.programs.get_mut(name) {
                prog.stop(period).await?;
            }
        }
        Ok(targets)
    }

    async fn execute_restart_group(
        &mut self,
        group: &str,
        grace_period: Option<Duration>,
    ) -> Result<Vec<String>, ProgramError> {
        let stopped = self.execute_stop_group(group, grace_period).await?;
        self.execute_start_group(group).await?;
        Ok(stopped)
    }

    async fn execute_start_program(&mut self, name: &str) -> Result<(), ProgramError> {
        let targets = self.find_match(name);
        if targets.is_empty() {
            return Err(ProgramError::NotFound {
                name: name.to_string(),
            });
        }
        for target in targets {
            if let Some(prog) = self.programs.get_mut(&target) {
                prog.start().await?;
            }
        }
        Ok(())
    }

    async fn execute_stop_program(
        &mut self,
        name: &str,
        grace_period: Option<Duration>,
    ) -> Result<(), ProgramError> {
        let targets = self.find_match(name);
        if targets.is_empty() {
            return Err(ProgramError::NotFound {
                name: name.to_string(),
            });
        }
        for target in targets.iter().rev() {
            let default_wait = self
                .configs
                .get(target)
                .map(|c| c.stop_wait_secs)
                .unwrap_or_else(|| DEFAULT_STOP_WAIT);
            let period = grace_period.unwrap_or(default_wait);
            if let Some(prog) = self.programs.get_mut(target) {
                prog.stop(period).await?;
            }
        }
        Ok(())
    }

    async fn execute_restart_program(
        &mut self,
        name: &str,
        grace_period: Option<Duration>,
    ) -> Result<(), ProgramError> {
        let targets = self.find_match(name);
        if targets.is_empty() {
            return Err(ProgramError::NotFound {
                name: name.to_string(),
            });
        }
        for target in targets.iter().rev() {
            let default_wait = self
                .configs
                .get(target)
                .map(|c| c.stop_wait_secs)
                .unwrap_or_else(|| DEFAULT_STOP_WAIT);
            let period = grace_period.unwrap_or(default_wait);
            if let Some(prog) = self.programs.get_mut(target) {
                prog.restart(period).await?;
            }
        }
        Ok(())
    }

    /// Starts all programs in layers according to the DAG topology (concurrently within each layer).
    async fn execute_start_all(&mut self) -> Result<(), ProgramError> {
        for layer in &self.dag.start_layers {
            let mut start_futs = Vec::new();
            for name in layer {
                let should_start = self.configs.get(name).map(|c| c.autostart).unwrap_or(false);
                if should_start
                    && let Some(prog) = self.programs.get(name)
                    && prog.status().state.is_stopped_or_fatal()
                {
                    let prog_name = name.clone();
                    start_futs.push(async move {
                        if let Err(e) = prog.start().await {
                            tracing::error!("Failed to autostart program '{}': {}", prog_name, e);
                        }
                    });
                }
            }
            if !start_futs.is_empty() {
                futures_util::future::join_all(start_futs).await;
            }
        }
        Ok(())
    }

    /// Gracefully stops all programs in reverse topological order (concurrently within each layer).
    async fn execute_stop_all(&mut self, grace_period: Option<Duration>) {
        for layer in &self.dag.stop_layers {
            let mut stop_futs = Vec::new();
            for name in layer {
                let wait_secs = self
                    .configs
                    .get(name)
                    .map(|c| c.stop_wait_secs)
                    .unwrap_or_else(|| DEFAULT_STOP_WAIT);
                let dur = grace_period.unwrap_or(wait_secs);
                if let Some(prog) = self.programs.get(name) {
                    let state = prog.status().state;
                    if state != ProgramState::Stopped {
                        let prog_name = name.clone();
                        stop_futs.push(async move {
                            if let Err(e) = prog.stop(dur).await {
                                tracing::warn!("Failed to stop program '{}': {}", prog_name, e);
                            }
                        });
                    }
                }
            }
            if !stop_futs.is_empty() {
                futures_util::future::join_all(stop_futs).await;
            }
        }
    }

    /// Performs hot reload: incremental update ensuring unchanged programs remain online with zero downtime.
    async fn execute_reload_config(
        &mut self,
        new_config: SupervisorConfig,
    ) -> Result<ReloadSummary, ProgramError> {
        let new_programs_map = new_config.resolve_programs()?;
        let new_dag = DependencyGraph::build(&new_programs_map)?;

        // The newly parsed config becomes the pending/source config for
        // addProcessGroup/removeProcessGroup, mirroring process_group_configs.
        self.pending_configs = new_programs_map.clone();

        let diff = ConfigDiff::compute(&self.configs, &new_programs_map);

        let summary = ReloadSummary {
            added: diff.added.iter().map(|c| c.name.clone()).collect(),
            removed: diff.removed.clone(),
            modified: diff.modified.iter().map(|c| c.name.clone()).collect(),
            unchanged: diff.unchanged.clone(),
        };

        // 1. Removed programs: gracefully stop, clean up and unregister
        for name in &diff.removed {
            if let Some(mut prog) = self.programs.remove(name) {
                let _ = prog.stop(DEFAULT_STOP_WAIT).await;
                let _ = prog.shutdown().await;
            }
            self.configs.remove(name);
        }

        // 2. Modified programs: gracefully stop old instance, replace with new instance, restart if autostart
        for new_cfg in diff.modified {
            let name = &new_cfg.name;
            if let Some(prog) = self.programs.get_mut(name) {
                let _ = prog.stop(new_cfg.stop_wait_secs).await;
                let _ = prog.shutdown().await;
            }

            let new_prog = instantiate_program(
                &new_cfg,
                &self.server_identifier,
                &self.activity_tracker,
                &self.event_hub,
                &self.cancel_token,
                &mut self.event_pools,
            )?;
            if new_cfg.autostart {
                let _ = new_prog.start().await;
            }

            self.programs.insert(name.clone(), new_prog);
            self.configs.insert(name.clone(), new_cfg);
        }

        // 3. Added programs: instantiate, register, and start if autostart
        for new_cfg in diff.added {
            let name = &new_cfg.name;
            let new_prog = instantiate_program(
                &new_cfg,
                &self.server_identifier,
                &self.activity_tracker,
                &self.event_hub,
                &self.cancel_token,
                &mut self.event_pools,
            )?;
            if new_cfg.autostart {
                let _ = new_prog.start().await;
            }

            self.programs.insert(name.clone(), new_prog);
            self.configs.insert(name.clone(), new_cfg);
        }

        // 4. Update the active DAG
        self.dag = new_dag;

        // 5. Rebuild cron table with updated configurations
        self.cron_table = crate::manager::cron::CronTable::from_configs(&self.configs);

        // 6. Update watch service with new program configurations
        self.watch_handle.update_configs(self.configs.clone()).await;

        // 7. Update active daemon logging configuration and rotator
        self.sync_logging_config(new_config.logging.clone());

        // 8. Broadcast ConfigReloaded event to subscribers
        self.event_hub
            .publish_system(crate::manager::SystemEvent::ConfigReloaded {
                added: summary.added.clone(),
                removed: summary.removed.clone(),
                modified: summary.modified.clone(),
                unchanged: summary.unchanged.clone(),
            });

        Ok(summary)
    }

    /// Performs Python-compatible daemon reload: stops all programs, drops instances,
    /// and restarts all programs from the newly parsed configuration.
    async fn execute_restart_daemon(
        &mut self,
        new_config: SupervisorConfig,
    ) -> Result<(), ProgramError> {
        let new_programs_map = new_config.resolve_programs()?;
        let _new_dag = DependencyGraph::build(&new_programs_map)?;

        // 1. Stop all running programs gracefully
        self.execute_stop_all(None).await;

        // 2. Shutdown existing program instances
        for (_, mut prog) in self.programs.drain() {
            let _ = prog.shutdown().await;
        }

        self.configs.clear();
        self.event_pools.clear();

        // 3. Store pending configs and instantiate programs from the new configuration
        self.pending_configs = new_programs_map.clone();

        for (name, cfg) in &new_programs_map {
            let prog = instantiate_program(
                cfg,
                &self.server_identifier,
                &self.activity_tracker,
                &self.event_hub,
                &self.cancel_token,
                &mut self.event_pools,
            )?;
            self.programs.insert(name.clone(), prog);
            self.configs.insert(name.clone(), cfg.clone());
        }

        self.sync_logging_config(new_config.logging.clone());
        self.refresh_derived_state().await;

        // 4. Autostart programs configured with autostart = true
        let _ = self.execute_start_all().await;

        self.event_hub
            .publish_system(crate::manager::SystemEvent::DaemonLifecycle {
                action: "restarted".to_string(),
                timestamp_secs: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            });

        Ok(())
    }

    /// Activates a process group from the pending/source config at runtime.
    /// Mirrors Python's `supervisord.add_process_group`: returns Ok(false) when the
    /// group is already active (ALREADY_ADDED), Err(NotFound) when the group does not
    /// exist in the pending config (BAD_NAME).
    async fn execute_add_process_group(&mut self, group: &str) -> Result<bool, ProgramError> {
        if self.configs.values().any(|cfg| cfg.group == group) {
            return Ok(false);
        }

        let candidates: Vec<ProgramConfig> = self
            .pending_configs
            .values()
            .filter(|cfg| cfg.group == group)
            .cloned()
            .collect();
        if candidates.is_empty() {
            return Err(ProgramError::NotFound {
                name: group.to_string(),
            });
        }

        for cfg in &candidates {
            let prog = instantiate_program(
                cfg,
                &self.server_identifier,
                &self.activity_tracker,
                &self.event_hub,
                &self.cancel_token,
                &mut self.event_pools,
            )?;
            if cfg.autostart {
                let _ = prog.start().await;
            }
            self.programs.insert(cfg.name.clone(), prog);
            self.configs.insert(cfg.name.clone(), cfg.clone());
        }

        self.refresh_derived_state().await;
        self.event_hub
            .publish_system(crate::manager::SystemEvent::ProcessGroupAdded {
                group: group.to_string(),
            });
        Ok(true)
    }

    /// Deactivates a process group from the active set at runtime.
    /// Mirrors Python's `supervisord.remove_process_group`: returns Ok(false) while any
    /// process in the group is still running (STILL_RUNNING), Err(NotFound) when the
    /// group is not active (BAD_NAME). The group remains in the pending config.
    async fn execute_remove_process_group(&mut self, group: &str) -> Result<bool, ProgramError> {
        let active: Vec<String> = self
            .configs
            .values()
            .filter(|cfg| cfg.group == group)
            .map(|cfg| cfg.name.clone())
            .collect();
        if active.is_empty() {
            return Err(ProgramError::NotFound {
                name: group.to_string(),
            });
        }

        for name in &active {
            if let Some(prog) = self.programs.get(name) {
                let st = prog.status();
                if st.state.is_active() || matches!(st.state, ProgramState::Backoff) {
                    return Ok(false);
                }
            }
        }

        for name in &active {
            if let Some(mut prog) = self.programs.remove(name) {
                let _ = prog.stop(DEFAULT_STOP_WAIT).await;
                let _ = prog.shutdown().await;
            }
            self.configs.remove(name);
        }

        self.refresh_derived_state().await;
        self.event_hub
            .publish_system(crate::manager::SystemEvent::ProcessGroupRemoved {
                group: group.to_string(),
            });
        Ok(true)
    }

    /// Rebuilds the derived scheduling structures after the active set changes.
    async fn refresh_derived_state(&mut self) {
        if let Ok(dag) = DependencyGraph::build(&self.configs) {
            self.dag = dag;
        }
        self.cron_table = crate::manager::cron::CronTable::from_configs(&self.configs);
        self.watch_handle.update_configs(self.configs.clone()).await;
    }

    /// Handles cron actions that became due at current wall-clock time.
    async fn handle_due_cron_jobs(&mut self) {
        let now = chrono::Utc::now();
        let actions = self.cron_table.pop_due_actions(now);

        for action in actions {
            match action {
                crate::manager::cron::CronAction::Start {
                    name,
                    group,
                    expression,
                } => {
                    if let Some(prog) = self.programs.get(&name) {
                        let st = prog.status();
                        if !st.state.is_active() {
                            tracing::info!(
                                program = %name,
                                cron = %expression,
                                "Cron schedule triggered program start"
                            );
                            self.event_hub.publish_system(
                                crate::manager::SystemEvent::CronTriggered {
                                    name: name.clone(),
                                    group,
                                    action: "start".to_string(),
                                    expression,
                                },
                            );
                            let _ = self.execute_start_program(&name).await;
                        } else {
                            tracing::info!(
                                program = %name,
                                state = ?st.state,
                                "Cron trigger skipped: program is already active"
                            );
                        }
                    }
                }
                crate::manager::cron::CronAction::Stop {
                    name,
                    group,
                    expression,
                } => {
                    if let Some(prog) = self.programs.get(&name) {
                        let st = prog.status();
                        if st.state.is_active() {
                            tracing::info!(
                                program = %name,
                                cron = %expression,
                                "Cron schedule triggered program stop"
                            );
                            self.event_hub.publish_system(
                                crate::manager::SystemEvent::CronTriggered {
                                    name: name.clone(),
                                    group,
                                    action: "stop".to_string(),
                                    expression,
                                },
                            );
                            let _ = self.execute_stop_program(&name, None).await;
                        }
                    }
                }
            }
        }
    }

    fn sync_logging_config(&mut self, new_logging: crate::config::schema::LoggingConfig) {
        let active_in_mem = self.config_logging.enabled.is_in_memory_only();
        let target_in_mem = new_logging.enabled.is_in_memory_only();

        if active_in_mem != target_in_mem {
            tracing::warn!(
                "Daemon logging mode switch (active: {:?}, target: {:?}) requires restarting the daemon to rebind tracing subscriber sinks; retaining active mode",
                self.config_logging.enabled,
                new_logging.enabled
            );
            let mut preserved = new_logging;
            preserved.enabled = self.config_logging.enabled;
            self.config_logging = preserved;
            return;
        }

        self.config_logging = new_logging;
    }

    fn execute_read_main_log(&self, offset: i64, length: i64) -> Result<String, ProgramError> {
        if let Some(ref rotator) = self.main_log_rotator {
            let (data, _, _) = rotator.read_bytes(offset, length);
            Ok(data)
        } else {
            let path = self.config_logging.resolved_file_path();
            if path.exists() {
                crate::logging::LogFileReader::read_bytes(&path, offset, length).map_err(|e| {
                    ProgramError::ReadLogFailed {
                        name: "main".to_string(),
                        error: e.to_string(),
                    }
                })
            } else {
                Ok(String::new())
            }
        }
    }

    fn execute_tail_main_log(
        &self,
        offset: i64,
        length: i64,
    ) -> Result<(String, i64, bool), ProgramError> {
        if let Some(ref rotator) = self.main_log_rotator {
            Ok(rotator.tail_bytes(offset, length))
        } else {
            let path = self.config_logging.resolved_file_path();
            if path.exists() {
                Ok(crate::logging::LogFileReader::tail_bytes(
                    &path, offset, length,
                ))
            } else {
                Ok((String::new(), offset, false))
            }
        }
    }

    fn execute_clear_main_log(&self) -> Result<(), ProgramError> {
        if let Some(ref rotator) = self.main_log_rotator {
            rotator.clear();
        }
        if let Some(ref rotator) = self.main_file_rotator {
            let _ = crate::logging::LogBackend::clear(rotator.as_ref());
        }
        let path = self.config_logging.resolved_file_path();
        if path.exists() {
            let _ = std::fs::write(&path, "");
        }
        Ok(())
    }
}

fn instantiate_program(
    cfg: &ProgramConfig,
    server_identifier: &str,
    activity_tracker: &crate::manager::ActivityTracker,
    event_hub: &crate::manager::EventHub,
    cancel_token: &CancellationToken,
    event_pools: &mut HashMap<String, EventListenerPool>,
) -> Result<Box<dyn Program>, ProgramError> {
    if let Some(ref el_cfg) = cfg.event_listener {
        let pool = event_pools
            .entry(el_cfg.pool_name.clone())
            .or_insert_with(|| {
                let pool = EventListenerPool::new(
                    &el_cfg.pool_name,
                    server_identifier,
                    el_cfg.events.clone(),
                    el_cfg.buffer_size,
                    cancel_token.clone(),
                );
                pool.spawn_event_hub_listener(event_hub.clone());
                pool
            });
        let prog = EventListenerProgram::new(cfg.clone(), pool.clone(), cancel_token.clone())?;
        Ok(Box::new(prog) as Box<dyn Program>)
    } else {
        let prog =
            ProcessProgram::with_options(cfg.clone(), activity_tracker.clone(), event_hub.clone())?;
        Ok(Box::new(prog) as Box<dyn Program>)
    }
}
