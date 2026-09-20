// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use crate::config::diff::ConfigDiff;
use crate::config::schema::SupervisorConfig;
use crate::error::ProgramError;
use crate::manager::dag::DependencyGraph;
use crate::program::config::ProgramConfig;
use crate::program::process::ProcessProgram;
use crate::program::state::ProgramStatus;
use crate::program::traits::Program;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
}

impl ManagerHandle {
    pub fn activity_tracker(&self) -> &crate::manager::ActivityTracker {
        &self.activity_tracker
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

        let timeout_dur = Duration::from_secs(30);
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

        let timeout_dur = Duration::from_secs(30);
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

        let timeout_dur = Duration::from_secs(45);
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

        let timeout_dur = Duration::from_secs(120);
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

        let timeout_dur = Duration::from_secs(120);
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

        let timeout_dur = Duration::from_secs(60);
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

        let timeout_dur = Duration::from_secs(10);
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

        let timeout_dur = Duration::from_secs(10);
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

        let timeout_dur = Duration::from_secs(10);
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

        let timeout_dur = Duration::from_secs(10);
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

    pub async fn shutdown(&self) -> Result<(), ProgramError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ManagerCommand::Shutdown { reply: reply_tx })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: "manager".to_string(),
            })?;

        let timeout_dur = Duration::from_secs(120);
        let _ = tokio::time::timeout(timeout_dur, reply_rx).await;
        self.cancel_token.cancel();
        Ok(())
    }
}

pub struct SupervisorManager {
    handle: ManagerHandle,
    actor_handle: Option<JoinHandle<()>>,
    _cancel_guard: tokio_util::sync::DropGuard,
}

impl SupervisorManager {
    pub fn new(initial_config: &SupervisorConfig) -> Result<Self, ProgramError> {
        let programs_map = initial_config.resolve_programs()?;
        let dag = DependencyGraph::build(&programs_map)?;

        let activity_tracker = crate::manager::ActivityTracker::with_interval(
            initial_config.metrics.idle_timeout_secs,
            initial_config.metrics.interval_secs,
            initial_config.metrics.enabled,
        );
        let event_hub = crate::manager::EventHub::default();

        let mut programs = HashMap::new();
        for (name, cfg) in &programs_map {
            let prog = ProcessProgram::with_options(
                cfg.clone(),
                activity_tracker.clone(),
                event_hub.clone(),
            )?;
            programs.insert(name.clone(), Box::new(prog) as Box<dyn Program>);
        }

        let cancel_token = CancellationToken::new();
        let (command_tx, command_rx) = mpsc::channel(64);

        let actor = ManagerActor {
            programs,
            configs: programs_map,
            dag,
            command_rx,
            activity_tracker: activity_tracker.clone(),
            event_hub: event_hub.clone(),
            cancel_token: cancel_token.clone(),
        };

        let actor_handle = tokio::spawn(actor.run());

        let handle = ManagerHandle {
            command_tx,
            cancel_token: cancel_token.clone(),
            activity_tracker,
            event_hub,
        };

        let cancel_guard = cancel_token.clone().drop_guard();

        Ok(Self {
            handle,
            actor_handle: Some(actor_handle),
            _cancel_guard: cancel_guard,
        })
    }

    pub fn handle(&self) -> ManagerHandle {
        self.handle.clone()
    }

    pub async fn shutdown(&mut self) -> Result<(), ProgramError> {
        let _ = self.handle.shutdown().await;
        if let Some(handle) = self.actor_handle.take() {
            let _ = handle.await;
        }
        Ok(())
    }
}

struct ManagerActor {
    programs: HashMap<String, Box<dyn Program>>,
    configs: HashMap<String, ProgramConfig>,
    dag: DependencyGraph,
    command_rx: mpsc::Receiver<ManagerCommand>,
    activity_tracker: crate::manager::ActivityTracker,
    event_hub: crate::manager::EventHub,
    cancel_token: CancellationToken,
}

impl ManagerActor {
    async fn run(mut self) {
        loop {
            tokio::select! {
                biased;

                _ = self.cancel_token.cancelled() => {
                    self.execute_stop_all(None).await;
                    break;
                }

                Some(cmd) = self.command_rx.recv() => {
                    match cmd {
                        ManagerCommand::StartProgram { name, reply } => {
                            let res = self.execute_start_program(&name).await;
                            let _ = reply.send(res);
                        }
                        ManagerCommand::StopProgram { name, grace_period, reply } => {
                            let res = self.execute_stop_program(&name, grace_period).await;
                            let _ = reply.send(res);
                        }
                        ManagerCommand::RestartProgram { name, grace_period, reply } => {
                            let res = self.execute_restart_program(&name, grace_period).await;
                            let _ = reply.send(res);
                        }
                        ManagerCommand::StartAll { reply } => {
                            let res = self.execute_start_all().await;
                            let _ = reply.send(res);
                        }
                        ManagerCommand::StopAll { grace_period, reply } => {
                            self.execute_stop_all(grace_period).await;
                            let _ = reply.send(Ok(()));
                        }
                        ManagerCommand::ReloadConfig { new_config, reply } => {
                            let res = self.execute_reload_config(*new_config).await;
                            let _ = reply.send(res);
                        }
                        ManagerCommand::GetStatus { name, reply } => {
                            let res = self.programs.get(&name).map(|p| p.status()).ok_or_else(|| {
                                ProgramError::NotRunning { name: name.clone() }
                            });
                            let _ = reply.send(res);
                        }
                        ManagerCommand::GetAllStatus { reply } => {
                            let statuses = self.programs.values().map(|p| p.status()).collect();
                            let _ = reply.send(statuses);
                        }
                        ManagerCommand::ReadLogs { name, lines, reply } => {
                            let res = self.programs.get(&name).map(|p| p.read_logs(lines)).ok_or_else(|| {
                                ProgramError::NotFound { name: name.clone() }
                            });
                            let _ = reply.send(res);
                        }
                        ManagerCommand::SubscribeLogs { name, reply } => {
                            let res = self.programs.get(&name).map(|p| p.subscribe_logs()).ok_or_else(|| {
                                ProgramError::NotFound { name: name.clone() }
                            });
                            let _ = reply.send(res);
                        }
                        ManagerCommand::Shutdown { reply } => {
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

    async fn execute_start_program(&mut self, name: &str) -> Result<(), ProgramError> {
        let prog = self
            .programs
            .get_mut(name)
            .ok_or_else(|| ProgramError::NotFound {
                name: name.to_string(),
            })?;

        prog.start().await
    }

    async fn execute_stop_program(
        &mut self,
        name: &str,
        grace_period: Option<Duration>,
    ) -> Result<(), ProgramError> {
        let default_wait = self
            .configs
            .get(name)
            .map(|c| c.stop_wait_secs)
            .unwrap_or(10);
        let period = grace_period.unwrap_or_else(|| Duration::from_secs(default_wait));

        let prog = self
            .programs
            .get_mut(name)
            .ok_or_else(|| ProgramError::NotFound {
                name: name.to_string(),
            })?;

        prog.stop(period).await
    }

    async fn execute_restart_program(
        &mut self,
        name: &str,
        grace_period: Option<Duration>,
    ) -> Result<(), ProgramError> {
        let default_wait = self
            .configs
            .get(name)
            .map(|c| c.stop_wait_secs)
            .unwrap_or(10);
        let period = grace_period.unwrap_or_else(|| Duration::from_secs(default_wait));

        let prog = self
            .programs
            .get_mut(name)
            .ok_or_else(|| ProgramError::NotFound {
                name: name.to_string(),
            })?;

        prog.restart(period).await
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
                    .unwrap_or(5);
                let dur = grace_period.unwrap_or_else(|| Duration::from_secs(wait_secs));
                if let Some(prog) = self.programs.get(name)
                    && prog.status().state.is_active()
                {
                    let prog_name = name.clone();
                    stop_futs.push(async move {
                        if let Err(e) = prog.stop(dur).await {
                            tracing::warn!("Failed to stop program '{}': {}", prog_name, e);
                        }
                    });
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
                let _ = prog.stop(Duration::from_secs(5)).await;
                let _ = prog.shutdown().await;
            }
            self.configs.remove(name);
        }

        // 2. Modified programs: gracefully stop old instance, replace with new instance, restart if autostart
        for new_cfg in diff.modified {
            let name = &new_cfg.name;
            if let Some(prog) = self.programs.get_mut(name) {
                let _ = prog.stop(Duration::from_secs(new_cfg.stop_wait_secs)).await;
                let _ = prog.shutdown().await;
            }

            let new_prog = ProcessProgram::with_options(
                new_cfg.clone(),
                self.activity_tracker.clone(),
                self.event_hub.clone(),
            )?;
            if new_cfg.autostart {
                let _ = new_prog.start().await;
            }

            self.programs
                .insert(name.clone(), Box::new(new_prog) as Box<dyn Program>);
            self.configs.insert(name.clone(), new_cfg);
        }

        // 3. Added programs: instantiate, register, and start if autostart
        for new_cfg in diff.added {
            let name = &new_cfg.name;
            let new_prog = ProcessProgram::with_options(
                new_cfg.clone(),
                self.activity_tracker.clone(),
                self.event_hub.clone(),
            )?;
            if new_cfg.autostart {
                let _ = new_prog.start().await;
            }

            self.programs
                .insert(name.clone(), Box::new(new_prog) as Box<dyn Program>);
            self.configs.insert(name.clone(), new_cfg);
        }

        // 4. Update the active DAG
        self.dag = new_dag;

        // 5. Broadcast ConfigReloaded event to subscribers
        self.event_hub
            .publish_system(crate::manager::SystemEvent::ConfigReloaded {
                added: summary.added.clone(),
                removed: summary.removed.clone(),
                modified: summary.modified.clone(),
                unchanged: summary.unchanged.clone(),
            });

        Ok(summary)
    }
}
