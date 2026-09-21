// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use crate::error::ProgramError;
use crate::logging::RingBuffer;
use crate::platform::PlatformProcessGuard;
use crate::program::config::{AutoRestartPolicy, ProgramConfig};
use crate::program::state::{ProgramState, ProgramStatus};
use crate::program::traits::Program;
use async_trait::async_trait;
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Commands accepted by the ProcessProgram Actor task.
#[derive(Debug)]
pub enum ProgramCommand {
    Start {
        reply: oneshot::Sender<Result<(), ProgramError>>,
    },
    Stop {
        grace_period: Duration,
        reply: oneshot::Sender<Result<(), ProgramError>>,
    },
    Restart {
        grace_period: Duration,
        reply: oneshot::Sender<Result<(), ProgramError>>,
    },
    Shutdown {
        reply: oneshot::Sender<Result<(), ProgramError>>,
    },
}

/// Concrete implementation of the Program trait managing an OS process.
pub struct ProcessProgram {
    config: ProgramConfig,
    command_tx: mpsc::Sender<ProgramCommand>,
    status_snapshot: Arc<RwLock<ProgramStatus>>,
    ring_buffer: Arc<RingBuffer>,
    started_at: Arc<RwLock<Option<Instant>>>,
    event_hub: crate::manager::EventHub,
    cancel_token: CancellationToken,
    _cancel_guard: tokio_util::sync::DropGuard,
    actor_handle: Option<JoinHandle<()>>,
}

impl ProcessProgram {
    pub fn new(config: ProgramConfig) -> Result<Self, ProgramError> {
        Self::with_options(
            config,
            crate::manager::ActivityTracker::default(),
            crate::manager::EventHub::default(),
        )
    }

    pub fn with_activity_tracker(
        config: ProgramConfig,
        activity_tracker: crate::manager::ActivityTracker,
    ) -> Result<Self, ProgramError> {
        Self::with_options(
            config,
            activity_tracker,
            crate::manager::EventHub::default(),
        )
    }

    pub fn with_options(
        config: ProgramConfig,
        activity_tracker: crate::manager::ActivityTracker,
        event_hub: crate::manager::EventHub,
    ) -> Result<Self, ProgramError> {
        config.validate()?;

        let status_snapshot = Arc::new(RwLock::new(ProgramStatus::new_stopped_with_group(
            &config.name,
            &config.group,
        )));
        let ring_buffer = Arc::new(RingBuffer::default());
        let started_at = Arc::new(RwLock::new(None));
        let cancel_token = CancellationToken::new();
        let (command_tx, command_rx) = mpsc::channel(32);

        let actor = ProgramActor::new(
            config.clone(),
            command_rx,
            status_snapshot.clone(),
            ring_buffer.clone(),
            started_at.clone(),
            activity_tracker,
            event_hub.clone(),
            cancel_token.clone(),
        );

        let actor_handle = tokio::spawn(actor.run());
        let cancel_guard = cancel_token.clone().drop_guard();

        Ok(Self {
            config,
            command_tx,
            status_snapshot,
            ring_buffer,
            started_at,
            event_hub,
            cancel_token,
            _cancel_guard: cancel_guard,
            actor_handle: Some(actor_handle),
        })
    }

    /// Waits until the program reaches the target state or times out using event-driven notification.
    pub async fn wait_for_state(
        &self,
        target: ProgramState,
        timeout_dur: Duration,
    ) -> Result<(), ProgramError> {
        if self.status().state == target {
            return Ok(());
        }

        let mut rx = self.event_hub.subscribe_system();
        // Check once again in case state transitioned before subscription
        if self.status().state == target {
            return Ok(());
        }

        let name = &self.config.name;
        let timeout_future = tokio::time::timeout(timeout_dur, async {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if let crate::manager::SystemEvent::StateChanged {
                            name: ref evt_name,
                            new_state,
                            ..
                        } = event
                            && evt_name == name
                            && new_state == target
                        {
                            return Ok(());
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        if self.status().state == target {
                            return Ok(());
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        return Err(ProgramError::ChannelClosed { name: name.clone() });
                    }
                }
            }
        });

        match timeout_future.await {
            Ok(res) => res,
            Err(_) => Err(ProgramError::Timeout {
                name: self.config.name.clone(),
                timeout_secs: timeout_dur.as_secs(),
            }),
        }
    }
}

#[async_trait]
impl Program for ProcessProgram {
    fn name(&self) -> &str {
        &self.config.name
    }

    fn priority(&self) -> u8 {
        self.config.priority
    }

    fn dependencies(&self) -> &[String] {
        &self.config.depends_on
    }

    fn status(&self) -> ProgramStatus {
        let mut status = self.status_snapshot.read().clone();
        if (status.state == ProgramState::Running || status.state == ProgramState::Starting)
            && let Some(started) = *self.started_at.read()
        {
            status.uptime_secs = Some(started.elapsed().as_secs());
        }
        status
    }

    async fn start(&self) -> Result<(), ProgramError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ProgramCommand::Start { reply: reply_tx })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: self.config.name.clone(),
            })?;

        let timeout_dur = Duration::from_secs(10);
        tokio::time::timeout(timeout_dur, reply_rx)
            .await
            .map_err(|_| ProgramError::Timeout {
                name: self.config.name.clone(),
                timeout_secs: timeout_dur.as_secs(),
            })?
            .map_err(|_| ProgramError::ChannelClosed {
                name: self.config.name.clone(),
            })?
    }

    async fn stop(&self, grace_period: Duration) -> Result<(), ProgramError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ProgramCommand::Stop {
                grace_period,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: self.config.name.clone(),
            })?;

        let timeout_dur = grace_period
            .checked_add(Duration::from_secs(5))
            .unwrap_or(Duration::from_secs(86400));
        tokio::time::timeout(timeout_dur, reply_rx)
            .await
            .map_err(|_| ProgramError::Timeout {
                name: self.config.name.clone(),
                timeout_secs: timeout_dur.as_secs(),
            })?
            .map_err(|_| ProgramError::ChannelClosed {
                name: self.config.name.clone(),
            })?
    }

    async fn restart(&self, grace_period: Duration) -> Result<(), ProgramError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ProgramCommand::Restart {
                grace_period,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: self.config.name.clone(),
            })?;

        let timeout_dur = grace_period
            .checked_add(Duration::from_secs(10))
            .unwrap_or(Duration::from_secs(86400));
        tokio::time::timeout(timeout_dur, reply_rx)
            .await
            .map_err(|_| ProgramError::Timeout {
                name: self.config.name.clone(),
                timeout_secs: timeout_dur.as_secs(),
            })?
            .map_err(|_| ProgramError::ChannelClosed {
                name: self.config.name.clone(),
            })?
    }

    async fn shutdown(&mut self) -> Result<(), ProgramError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if self
            .command_tx
            .send(ProgramCommand::Shutdown { reply: reply_tx })
            .await
            .is_ok()
        {
            let timeout_dur = Duration::from_secs(self.config.stop_wait_secs + 2);
            let _ = tokio::time::timeout(timeout_dur, reply_rx).await;
        }
        self.cancel_token.cancel();
        if let Some(handle) = self.actor_handle.take() {
            let _ = handle.await;
        }
        Ok(())
    }

    fn read_logs(&self, max_lines: Option<usize>) -> Vec<String> {
        self.ring_buffer.get_lines(max_lines)
    }

    fn subscribe_logs(&self) -> tokio::sync::broadcast::Receiver<String> {
        self.ring_buffer.subscribe()
    }
}

struct RunningChild {
    child: tokio::process::Child,
    pid: u32,
    marked_running: bool,
    platform_guard: Box<dyn PlatformProcessGuard>,
    stdout_pump: Option<JoinHandle<()>>,
    stderr_pump: Option<JoinHandle<()>>,
    cancel_token: CancellationToken,
    _cancel_guard: tokio_util::sync::DropGuard,
    health_task: Option<JoinHandle<()>>,
}

impl RunningChild {
    async fn drain_pumps(&mut self) {
        self.cancel_token.cancel();
        let health = self.health_task.take();
        let out = self.stdout_pump.take();
        let err = self.stderr_pump.take();

        let health_abort = health.as_ref().map(|h| h.abort_handle());
        let out_abort = out.as_ref().map(|h| h.abort_handle());
        let err_abort = err.as_ref().map(|h| h.abort_handle());

        let hard_deadline = tokio::time::timeout(Duration::from_secs(2), async {
            if let Some(h) = health {
                let _ = h.await;
            }
            if let Some(h) = out {
                let _ = h.await;
            }
            if let Some(h) = err {
                let _ = h.await;
            }
        });

        if hard_deadline.await.is_err() {
            tracing::warn!(
                "Log pump drain timed out (inherited pipe by grandchild?); aborting pumps"
            );
            if let Some(ref a) = health_abort {
                a.abort();
            }
            if let Some(ref a) = out_abort {
                a.abort();
            }
            if let Some(ref a) = err_abort {
                a.abort();
            }
        }
    }
}

struct ProgramActor {
    config: ProgramConfig,
    command_rx: mpsc::Receiver<ProgramCommand>,
    health_rx: mpsc::Receiver<crate::manager::HealthEvent>,
    health_tx: mpsc::Sender<crate::manager::HealthEvent>,
    status_snapshot: Arc<RwLock<ProgramStatus>>,
    ring_buffer: Arc<RingBuffer>,
    started_at: Arc<RwLock<Option<Instant>>>,
    activity_tracker: crate::manager::ActivityTracker,
    event_hub: crate::manager::EventHub,
    cancel_token: CancellationToken,
    current_child: Option<RunningChild>,
    retry_count: u32,
    manual_stop: bool,
    is_shutting_down: bool,
    backoff_deadline: Option<tokio::time::Instant>,
    stdout_rotator: Option<crate::logging::LogRotator>,
    stderr_rotator: Option<crate::logging::LogRotator>,
}

impl ProgramActor {
    #[allow(clippy::too_many_arguments)]
    fn new(
        config: ProgramConfig,
        command_rx: mpsc::Receiver<ProgramCommand>,
        status_snapshot: Arc<RwLock<ProgramStatus>>,
        ring_buffer: Arc<RingBuffer>,
        started_at: Arc<RwLock<Option<Instant>>>,
        activity_tracker: crate::manager::ActivityTracker,
        event_hub: crate::manager::EventHub,
        cancel_token: CancellationToken,
    ) -> Self {
        let (health_tx, health_rx) = mpsc::channel(16);

        let max_bytes = match &config.logs.max_bytes {
            Some(s) => crate::logging::parse_byte_size(s).unwrap_or(20 * 1024 * 1024),
            None => 20 * 1024 * 1024,
        };
        let backups = config.logs.backups.unwrap_or(3);
        let stdout_disabled = config.logs.is_stdout_disabled();
        let stderr_disabled = config.logs.is_stderr_disabled();

        let stdout_rotator = if !stdout_disabled {
            if let Some(ref path) = config.logs.stdout {
                match crate::logging::LogRotator::new(path, max_bytes, backups) {
                    Ok(rot) => Some(rot),
                    Err(e) => {
                        tracing::error!(
                            "Failed to initialize stdout LogRotator for '{}' at {:?}: {}",
                            config.name,
                            path,
                            e
                        );
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        let stderr_rotator = if !stderr_disabled {
            if config.logs.redirect_stderr {
                stdout_rotator.clone()
            } else if let Some(ref path) = config.logs.stderr {
                match crate::logging::LogRotator::new(path, max_bytes, backups) {
                    Ok(rot) => Some(rot),
                    Err(e) => {
                        tracing::error!(
                            "Failed to initialize stderr LogRotator for '{}' at {:?}: {}",
                            config.name,
                            path,
                            e
                        );
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        Self {
            config,
            command_rx,
            health_rx,
            health_tx,
            status_snapshot,
            ring_buffer,
            started_at,
            activity_tracker,
            event_hub,
            cancel_token,
            current_child: None,
            retry_count: 0,
            manual_stop: false,
            is_shutting_down: false,
            backoff_deadline: None,
            stdout_rotator,
            stderr_rotator,
        }
    }

    async fn run(mut self) {
        let has_health_check = self.config.health_check.is_some();
        let mut start_deadline: Option<tokio::time::Instant> = None;
        let mut metrics_interval =
            tokio::time::interval(Duration::from_secs(self.activity_tracker.interval_secs()));
        metrics_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            let has_child = self.current_child.is_some();
            let is_waiting_start = start_deadline.is_some();
            let is_metrics_active = self.activity_tracker.is_metrics_active();

            tokio::select! {
                biased;

                _ = self.cancel_token.cancelled() => {
                    self.is_shutting_down = true;
                    self.manual_stop = true;
                    self.backoff_deadline = None;
                    self.stop_current_child(Duration::from_secs(self.config.stop_wait_secs)).await;
                    break;
                }

                Some(cmd) = self.command_rx.recv() => {
                    self.handle_command(cmd).await;
                    if self.current_child.is_some() {
                        let marked_running = self
                            .current_child
                            .as_ref()
                            .map(|c| c.marked_running)
                            .unwrap_or(false);
                        if !marked_running && self.config.start_secs > 0 {
                            start_deadline = Some(
                                tokio::time::Instant::now()
                                    + Duration::from_secs(self.config.start_secs),
                            );
                        } else {
                            start_deadline = None;
                        }
                    } else {
                        start_deadline = None;
                    }
                }

                Some(health_event) = self.health_rx.recv(), if has_child && has_health_check => {
                    match health_event {
                        crate::manager::HealthEvent::Healthy => {
                            let changed = {
                                let mut snapshot = self.status_snapshot.write();
                                let old = snapshot.health;
                                snapshot.health = crate::program::state::HealthStatus::Healthy;
                                snapshot.is_healthy = true;
                                old != crate::program::state::HealthStatus::Healthy
                            };
                            if changed {
                                self.event_hub.publish_system(crate::manager::SystemEvent::HealthChanged {
                                    name: self.config.name.clone(),
                                    healthy: true,
                                    status: crate::program::state::HealthStatus::Healthy,
                                    reason: None,
                                });
                            }
                        }
                        crate::manager::HealthEvent::Unhealthy => {
                            let (should_restart, changed) = {
                                let mut snapshot = self.status_snapshot.write();
                                let old = snapshot.health;
                                snapshot.health = crate::program::state::HealthStatus::Unhealthy;
                                snapshot.is_healthy = false;
                                (self.config.autorestart != AutoRestartPolicy::Never, old != crate::program::state::HealthStatus::Unhealthy)
                            };

                            if changed {
                                self.event_hub.publish_system(crate::manager::SystemEvent::HealthChanged {
                                    name: self.config.name.clone(),
                                    healthy: false,
                                    status: crate::program::state::HealthStatus::Unhealthy,
                                    reason: Some("Health check probe failed consecutively".to_string()),
                                });
                            }

                            if should_restart {
                                tracing::warn!(
                                    program = %self.config.name,
                                    "Program health check failed consecutively; restarting child process"
                                );
                                self.stop_current_child(Duration::from_secs(self.config.stop_wait_secs)).await;
                                if let Err(e) = self.spawn_child().await {
                                    tracing::error!(program = %self.config.name, error = %e, "Failed to restart child after health check failure");
                                    start_deadline = None;
                                    self.update_status(
                                        ProgramState::Fatal,
                                        None,
                                        None,
                                        format!("Health check restart failed: {}", e),
                                    );
                                } else if self.config.start_secs > 0 {
                                    start_deadline = Some(tokio::time::Instant::now() + Duration::from_secs(self.config.start_secs));
                                } else {
                                    start_deadline = None;
                                }
                            }
                        }
                    }
                }

                _ = async {
                    match self.backoff_deadline {
                        Some(dl) => tokio::time::sleep_until(dl).await,
                        None => std::future::pending().await,
                    }
                }, if self.backoff_deadline.is_some() && !self.is_shutting_down && !self.manual_stop => {
                    self.backoff_deadline = None;
                    if self.is_shutting_down || self.manual_stop {
                        // Suppress restart during shutdown or manual stop
                    } else if let Err(e) = self.spawn_child().await {
                        tracing::error!(program = %self.config.name, error = %e, "Failed to spawn child after backoff");
                        self.update_status(
                            ProgramState::Fatal,
                            None,
                            None,
                            format!("Spawn failed after backoff: {}", e),
                        );
                    } else if self.config.start_secs > 0 {
                        start_deadline = Some(tokio::time::Instant::now() + Duration::from_secs(self.config.start_secs));
                    }
                }

                _ = metrics_interval.tick(), if has_child && is_metrics_active => {
                    if let Some(ref child) = self.current_child
                        && let Ok(m) = child.platform_guard.query_metrics()
                    {
                        let mut snapshot = self.status_snapshot.write();
                        snapshot.metrics = Some(m);
                    }
                }

                exit_res = async {
                    match self.current_child.as_mut() {
                        Some(c) => c.platform_guard.wait_exit(&mut c.child).await,
                        None => std::future::pending().await,
                    }
                }, if has_child => {
                    start_deadline = None;
                    let restarted = self.handle_child_exit(exit_res).await;
                    if restarted && self.config.start_secs > 0 {
                        start_deadline = Some(tokio::time::Instant::now() + Duration::from_secs(self.config.start_secs));
                    }
                }

                _ = async {
                    match start_deadline {
                        Some(dl) => tokio::time::sleep_until(dl).await,
                        None => std::future::pending().await,
                    }
                }, if is_waiting_start => {
                    start_deadline = None;
                    if let Some(child) = self.current_child.as_mut() {
                        if let Ok(Some(_)) = child.child.try_wait() {
                            // Child already terminated; exit_res will handle exit on next iteration
                        } else {
                            child.marked_running = true;
                            let pid = child.pid;
                            self.retry_count = 0;
                            self.update_status(ProgramState::Running, Some(pid), None, "Running".to_string());
                        }
                    }
                }
            }
        }

        self.update_status(ProgramState::Stopped, None, None, "Stopped".to_string());
    }

    async fn handle_command(&mut self, cmd: ProgramCommand) {
        match cmd {
            ProgramCommand::Start { reply } => {
                if self.is_shutting_down {
                    let _ = reply.send(Err(ProgramError::ShuttingDown {
                        name: self.config.name.clone(),
                    }));
                    return;
                }
                self.manual_stop = false;
                self.retry_count = 0;
                self.backoff_deadline = None;
                let res = self.spawn_child().await;
                let _ = reply.send(res);
            }
            ProgramCommand::Stop {
                grace_period,
                reply,
            } => {
                self.manual_stop = true;
                self.backoff_deadline = None;
                self.stop_current_child(grace_period).await;
                let _ = reply.send(Ok(()));
            }
            ProgramCommand::Restart {
                grace_period,
                reply,
            } => {
                if self.is_shutting_down {
                    let _ = reply.send(Err(ProgramError::ShuttingDown {
                        name: self.config.name.clone(),
                    }));
                    return;
                }
                self.manual_stop = true;
                self.retry_count = 0;
                self.backoff_deadline = None;
                self.stop_current_child(grace_period).await;
                self.manual_stop = false;
                let res = self.spawn_child().await;
                let _ = reply.send(res);
            }
            ProgramCommand::Shutdown { reply } => {
                self.is_shutting_down = true;
                self.manual_stop = true;
                self.backoff_deadline = None;
                self.stop_current_child(Duration::from_secs(self.config.stop_wait_secs))
                    .await;
                let _ = reply.send(Ok(()));
                self.cancel_token.cancel();
            }
        }
    }

    async fn spawn_child(&mut self) -> Result<(), ProgramError> {
        if self.is_shutting_down {
            return Err(ProgramError::ShuttingDown {
                name: self.config.name.clone(),
            });
        }

        if let Some(ref mut child) = self.current_child {
            match child.child.try_wait() {
                Ok(Some(_status)) => {
                    tracing::warn!(
                        program = %self.config.name,
                        pid = child.pid,
                        "Previous child process already terminated; cleaning up before new spawn"
                    );
                    child.drain_pumps().await;
                    self.current_child = None;
                }
                _ => {
                    return Err(ProgramError::AlreadyRunning {
                        name: self.config.name.clone(),
                        pid: child.pid,
                    });
                }
            }
        }

        self.manual_stop = false;

        if let Some(ref hook) = self.config.pre_start {
            self.event_hub
                .publish_system(crate::manager::SystemEvent::ProcessPreStart {
                    name: self.config.name.clone(),
                    group: self.config.group.clone(),
                    command: hook.clone(),
                });

            let timeout_dur = Duration::from_secs(self.config.hook_timeout_secs);
            if let Err(err) = self.run_hook(hook, timeout_dur).await {
                self.event_hub
                    .publish_system(crate::manager::SystemEvent::ProcessPreStartFailed {
                        name: self.config.name.clone(),
                        group: self.config.group.clone(),
                        error: err.clone(),
                    });

                if !self.config.pre_start_ignore_failure {
                    self.update_status(
                        ProgramState::Fatal,
                        None,
                        None,
                        format!("pre_start hook failed: {}", err),
                    );
                    return Err(ProgramError::PreStartHookFailed {
                        name: self.config.name.clone(),
                        reason: err,
                    });
                } else {
                    tracing::warn!(
                        program = %self.config.name,
                        error = %err,
                        "pre_start hook failed, but pre_start_ignore_failure is true; continuing start"
                    );
                }
            }
        }

        let mut cmd = tokio::process::Command::new(&self.config.command);
        cmd.args(&self.config.args);

        if let Some(ref dir) = self.config.directory {
            cmd.current_dir(dir);
        }

        for (k, v) in &self.config.environment {
            cmd.env(k, v);
        }

        cmd.stdin(std::process::Stdio::null());

        let stdout_disabled = self.config.logs.is_stdout_disabled();
        let stderr_disabled = self.config.logs.is_stderr_disabled();

        // Configure async pipes or null stdio for stdout and stderr capture
        if stdout_disabled {
            cmd.stdout(std::process::Stdio::null());
        } else {
            cmd.stdout(std::process::Stdio::piped());
        }

        if stderr_disabled {
            cmd.stderr(std::process::Stdio::null());
        } else {
            cmd.stderr(std::process::Stdio::piped());
        }

        // Platform-agnostic pre-spawn configuration via PlatformBackend
        let platform = crate::platform::native_platform();
        platform.configure_command(&mut cmd, self.config.user.as_deref(), self.config.umask)?;

        let child = cmd.spawn().map_err(|e| ProgramError::StartFailed {
            name: self.config.name.clone(),
            source: e,
        })?;

        // Guard the newly spawned child process so it is safely terminated if any subsequent setup step fails
        let mut child_guard = scopeguard::guard(child, |mut c| {
            let _ = c.start_kill();
        });

        let pid = child_guard.id().ok_or_else(|| {
            ProgramError::PlatformError("Process spawned without PID".to_string())
        })?;

        let stdout = child_guard.stdout.take();
        let stderr = child_guard.stderr.take();

        // Attach platform-specific process guard
        let platform_guard = platform.attach_child(&child_guard, pid)?;

        let stdout_pump = if !stdout_disabled {
            stdout.map(|pipe| {
                crate::logging::spawn_log_pump(
                    pipe,
                    self.ring_buffer.clone(),
                    self.stdout_rotator.clone(),
                    None,
                    Some(self.event_hub.clone()),
                    Some(self.config.name.clone()),
                    "stdout",
                )
            })
        } else {
            None
        };

        let stderr_prefix = if self.config.logs.redirect_stderr {
            None
        } else {
            Some("STDERR".to_string())
        };

        let stderr_pump = if !stderr_disabled {
            stderr.map(|pipe| {
                crate::logging::spawn_log_pump(
                    pipe,
                    self.ring_buffer.clone(),
                    self.stderr_rotator.clone(),
                    stderr_prefix,
                    Some(self.event_hub.clone()),
                    Some(self.config.name.clone()),
                    "stderr",
                )
            })
        } else {
            None
        };

        *self.started_at.write() = Some(Instant::now());
        let child_cancel = CancellationToken::new();

        let health_task = if let Some(ref hcfg) = self.config.health_check {
            let runner = crate::manager::HealthProbeRunner::new(
                &self.config.name,
                hcfg.clone(),
                self.health_tx.clone(),
                child_cancel.clone(),
            );
            Some(tokio::spawn(runner.run()))
        } else {
            None
        };

        let initial_health = if self.config.health_check.is_some() {
            crate::program::state::HealthStatus::Starting
        } else {
            crate::program::state::HealthStatus::None
        };

        let marked_running = self.config.start_secs == 0;
        let initial_state = if marked_running {
            ProgramState::Running
        } else {
            ProgramState::Starting
        };

        self.update_status(
            initial_state,
            Some(pid),
            None,
            format!("{:?}", initial_state),
        );

        {
            let mut snapshot = self.status_snapshot.write();
            snapshot.health = initial_health;
            snapshot.is_healthy = initial_health == crate::program::state::HealthStatus::Healthy
                || (initial_health == crate::program::state::HealthStatus::None && marked_running);
            snapshot.uptime_secs = Some(0);
        }

        let child = scopeguard::ScopeGuard::into_inner(child_guard);
        let cancel_guard = child_cancel.clone().drop_guard();

        self.current_child = Some(RunningChild {
            child,
            pid,
            marked_running,
            platform_guard,
            stdout_pump,
            stderr_pump,
            cancel_token: child_cancel,
            _cancel_guard: cancel_guard,
            health_task,
        });

        Ok(())
    }

    async fn stop_current_child(&mut self, grace_period: Duration) {
        *self.started_at.write() = None;
        if let Some(mut child_info) = self.current_child.take() {
            self.update_status(
                ProgramState::Stopping,
                Some(child_info.pid),
                None,
                "Stopping".to_string(),
            );

            // Execute pre_stop hook if configured (with graceful failure degradation)
            if let Some(ref hook) = self.config.pre_stop {
                self.event_hub
                    .publish_system(crate::manager::SystemEvent::ProcessPreStop {
                        name: self.config.name.clone(),
                        group: self.config.group.clone(),
                        command: hook.clone(),
                    });

                let timeout_dur = Duration::from_secs(self.config.hook_timeout_secs);
                if let Err(err) = self.run_hook(hook, timeout_dur).await {
                    self.event_hub.publish_system(
                        crate::manager::SystemEvent::ProcessPreStopFailed {
                            name: self.config.name.clone(),
                            group: self.config.group.clone(),
                            error: err.clone(),
                        },
                    );
                    tracing::warn!(
                        program = %self.config.name,
                        error = %err,
                        "pre_stop hook failed; degrading to proceed with process termination"
                    );
                }
            }

            // Signal the process tree using the platform guard
            let _ = child_info
                .platform_guard
                .send_stop_signal(self.config.stop_signal);

            // Wait for graceful exit; escalate to force_kill if grace period expires
            let wait_res = tokio::time::timeout(
                grace_period,
                child_info.platform_guard.wait_exit(&mut child_info.child),
            )
            .await;
            let final_code = match wait_res {
                Ok(Ok(status)) => status.code(),
                _ => {
                    let _ = child_info.platform_guard.force_kill();
                    let _ = child_info.child.kill().await;
                    let post_kill_wait = tokio::time::timeout(
                        Duration::from_secs(2),
                        child_info.platform_guard.wait_exit(&mut child_info.child),
                    )
                    .await;
                    match post_kill_wait {
                        Ok(Ok(status)) => status.code(),
                        _ => None,
                    }
                }
            };

            // Drain stdout & stderr log pipes before marking stopped
            child_info.drain_pumps().await;

            if let Some(code) = final_code {
                self.update_status(
                    ProgramState::Stopped,
                    None,
                    Some(code),
                    format!("Stopped with exit code {:?}", code),
                );
            } else {
                self.update_status(
                    ProgramState::Stopped,
                    None,
                    None,
                    "Force killed".to_string(),
                );
            }
        } else {
            self.update_status(ProgramState::Stopped, None, None, "Stopped".to_string());
        }
    }

    /// Executes an external lifecycle hook script with bounded timeout.
    async fn run_hook(&self, hook_cmd: &str, timeout_dur: Duration) -> Result<(), String> {
        let mut cmd = crate::platform::native_platform().build_shell_command(hook_cmd);
        if let Some(ref dir) = self.config.directory {
            cmd.current_dir(dir);
        }
        for (k, v) in &self.config.environment {
            cmd.env(k, v);
        }
        let platform = crate::platform::native_platform();
        if let Err(e) =
            platform.configure_command(&mut cmd, self.config.user.as_deref(), self.config.umask)
        {
            return Err(format!("Failed to configure hook command: {}", e));
        }
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());

        match tokio::time::timeout(timeout_dur, cmd.status()).await {
            Ok(Ok(status)) if status.success() => Ok(()),
            Ok(Ok(status)) => Err(format!(
                "Hook exited with failure status {}",
                status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".to_string())
            )),
            Ok(Err(e)) => Err(format!("Hook spawn failed: {}", e)),
            Err(_) => Err(format!("Hook timed out after {}s", timeout_dur.as_secs())),
        }
    }

    /// Handles child process exit. Returns true if an automatic restart was scheduled.
    async fn handle_child_exit(
        &mut self,
        exit_res: std::io::Result<std::process::ExitStatus>,
    ) -> bool {
        *self.started_at.write() = None;
        let mut child_info = self.current_child.take();
        let pid = child_info.as_ref().map(|c| c.pid);
        let marked_running = child_info
            .as_ref()
            .map(|c| c.marked_running)
            .unwrap_or(false);

        let exit_code = match exit_res {
            Ok(status) => status.code(),
            Err(e) => {
                tracing::error!(program = %self.config.name, error = %e, "Error waiting for child exit");
                None
            }
        };

        if let Some(ref mut c) = child_info {
            c.drain_pumps().await;
        }

        if self.is_shutting_down || self.manual_stop {
            self.backoff_deadline = None;
            self.update_status(
                ProgramState::Stopped,
                None,
                exit_code,
                format!("Stopped with code {:?}", exit_code),
            );
            return false;
        }

        let is_expected = if let Some(code) = exit_code {
            self.config.exit_codes.contains(&code)
        } else {
            false
        };

        if !marked_running {
            // Process crashed during the start_secs window: treated as startup failure
            self.retry_count += 1;
            if self.retry_count <= self.config.start_retries {
                self.update_status(
                    ProgramState::Backoff,
                    pid,
                    exit_code,
                    format!(
                        "Crashed during startup, retry {}/{}",
                        self.retry_count, self.config.start_retries
                    ),
                );
                let backoff_secs = 2u64.pow(self.retry_count.min(5));
                self.backoff_deadline =
                    Some(tokio::time::Instant::now() + Duration::from_secs(backoff_secs));
                false
            } else {
                self.update_status(
                    ProgramState::Fatal,
                    None,
                    exit_code,
                    format!(
                        "Exited during startup with code {:?}, max retries exceeded",
                        exit_code
                    ),
                );
                false
            }
        } else {
            // Process exited after being marked running
            match self.config.autorestart {
                AutoRestartPolicy::Always => {
                    self.update_status(
                        ProgramState::Starting,
                        None,
                        exit_code,
                        "Autorestarting (policy: always)".to_string(),
                    );
                    if let Err(e) = self.spawn_child().await {
                        tracing::error!(
                            program = %self.config.name,
                            error = %e,
                            "Failed to autorestart child process"
                        );
                        self.update_status(
                            ProgramState::Fatal,
                            None,
                            exit_code,
                            format!("Autorestart spawn failed: {}", e),
                        );
                        false
                    } else {
                        true
                    }
                }
                AutoRestartPolicy::Unexpected => {
                    if !is_expected {
                        self.update_status(
                            ProgramState::Starting,
                            None,
                            exit_code,
                            format!("Unexpected exit with code {:?}, autorestarting", exit_code),
                        );
                        if let Err(e) = self.spawn_child().await {
                            tracing::error!(
                                program = %self.config.name,
                                error = %e,
                                "Failed to autorestart child process"
                            );
                            self.update_status(
                                ProgramState::Fatal,
                                None,
                                exit_code,
                                format!("Autorestart spawn failed: {}", e),
                            );
                            false
                        } else {
                            true
                        }
                    } else {
                        self.update_status(
                            ProgramState::Exited,
                            None,
                            exit_code,
                            format!("Exited normally with code {:?}", exit_code),
                        );
                        false
                    }
                }
                AutoRestartPolicy::Never => {
                    self.update_status(
                        ProgramState::Exited,
                        None,
                        exit_code,
                        format!("Exited with code {:?}", exit_code),
                    );
                    false
                }
            }
        }
    }

    fn update_status(
        &self,
        state: ProgramState,
        pid: Option<u32>,
        exit_code: Option<i32>,
        description: String,
    ) {
        let (old_state, should_emit) = {
            let mut snapshot = self.status_snapshot.write();
            let old_state = snapshot.state;
            snapshot.state = state;
            snapshot.pid = pid;
            snapshot.exit_code = exit_code;
            snapshot.description = description.clone();
            if state != ProgramState::Running && state != ProgramState::Starting {
                snapshot.health = crate::program::state::HealthStatus::None;
                snapshot.is_healthy = false;
                snapshot.metrics = None;
                snapshot.uptime_secs = None;
            }
            (old_state, old_state != state || exit_code.is_some())
        };

        if should_emit {
            self.event_hub
                .publish_system(crate::manager::SystemEvent::StateChanged {
                    name: self.config.name.clone(),
                    old_state,
                    new_state: state,
                    pid,
                    exit_code,
                    description,
                });
        }
    }
}
