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
    cancel_token: CancellationToken,
    actor_handle: Option<JoinHandle<()>>,
}

impl ProcessProgram {
    pub fn new(config: ProgramConfig) -> Result<Self, ProgramError> {
        Self::with_activity_tracker(config, crate::manager::ActivityTracker::default())
    }

    pub fn with_activity_tracker(
        config: ProgramConfig,
        activity_tracker: crate::manager::ActivityTracker,
    ) -> Result<Self, ProgramError> {
        config.validate()?;

        let status_snapshot = Arc::new(RwLock::new(ProgramStatus::new_stopped(&config.name)));
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
            cancel_token.clone(),
        );

        let actor_handle = tokio::spawn(actor.run());

        Ok(Self {
            config,
            command_tx,
            status_snapshot,
            ring_buffer,
            started_at,
            cancel_token,
            actor_handle: Some(actor_handle),
        })
    }

    /// Waits until the program reaches the target state or times out.
    pub async fn wait_for_state(
        &self,
        target: ProgramState,
        timeout_dur: Duration,
    ) -> Result<(), ProgramError> {
        let start = Instant::now();
        while start.elapsed() < timeout_dur {
            if self.status().state == target {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Err(ProgramError::Timeout {
            name: self.config.name.clone(),
            timeout_secs: timeout_dur.as_secs(),
        })
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

    async fn start(&mut self) -> Result<(), ProgramError> {
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

    async fn stop(&mut self, grace_period: Duration) -> Result<(), ProgramError> {
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

        let timeout_dur = grace_period + Duration::from_secs(5);
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

    async fn restart(&mut self, grace_period: Duration) -> Result<(), ProgramError> {
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

        let timeout_dur = grace_period + Duration::from_secs(10);
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

impl Drop for ProcessProgram {
    fn drop(&mut self) {
        self.cancel_token.cancel();
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
    health_task: Option<JoinHandle<()>>,
}

impl RunningChild {
    async fn drain_pumps(&mut self) {
        self.cancel_token.cancel();
        if let Some(handle) = self.health_task.take() {
            let _ = handle.await;
        }
        if let Some(handle) = self.stdout_pump.take() {
            let _ = handle.await;
        }
        if let Some(handle) = self.stderr_pump.take() {
            let _ = handle.await;
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
    cancel_token: CancellationToken,
    current_child: Option<RunningChild>,
    retry_count: u32,
    manual_stop: bool,
}

impl ProgramActor {
    fn new(
        config: ProgramConfig,
        command_rx: mpsc::Receiver<ProgramCommand>,
        status_snapshot: Arc<RwLock<ProgramStatus>>,
        ring_buffer: Arc<RingBuffer>,
        started_at: Arc<RwLock<Option<Instant>>>,
        activity_tracker: crate::manager::ActivityTracker,
        cancel_token: CancellationToken,
    ) -> Self {
        let (health_tx, health_rx) = mpsc::channel(16);
        Self {
            config,
            command_rx,
            health_rx,
            health_tx,
            status_snapshot,
            ring_buffer,
            started_at,
            activity_tracker,
            cancel_token,
            current_child: None,
            retry_count: 0,
            manual_stop: false,
        }
    }

    async fn run(mut self) {
        let has_health_check = self.config.health_check.is_some();
        let mut start_deadline: Option<tokio::time::Instant> = None;
        let mut metrics_interval = tokio::time::interval(Duration::from_secs(2));
        metrics_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            let has_child = self.current_child.is_some();
            let is_waiting_start = start_deadline.is_some();
            let is_metrics_active = self.activity_tracker.is_metrics_active();

            tokio::select! {
                biased;

                _ = self.cancel_token.cancelled() => {
                    self.stop_current_child(Duration::from_secs(self.config.stop_wait_secs)).await;
                    break;
                }

                Some(health_event) = self.health_rx.recv(), if has_child && has_health_check => {
                    match health_event {
                        crate::manager::HealthEvent::Healthy => {
                            let mut snapshot = self.status_snapshot.write();
                            snapshot.health = crate::program::state::HealthStatus::Healthy;
                            snapshot.is_healthy = true;
                        }
                        crate::manager::HealthEvent::Unhealthy => {
                            let should_restart = {
                                let mut snapshot = self.status_snapshot.write();
                                snapshot.health = crate::program::state::HealthStatus::Unhealthy;
                                snapshot.is_healthy = false;
                                self.config.autorestart != AutoRestartPolicy::Never
                            };

                            if should_restart {
                                tracing::warn!(
                                    program = %self.config.name,
                                    "Program health check failed consecutively; restarting child process"
                                );
                                self.stop_current_child(Duration::from_secs(self.config.stop_wait_secs)).await;
                                let _ = self.spawn_child().await;
                            }
                        }
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

                Some(cmd) = self.command_rx.recv() => {
                    let was_running = self.current_child.is_some();
                    self.handle_command(cmd).await;
                    if !was_running && self.current_child.is_some() {
                        if self.config.start_secs > 0 {
                            start_deadline = Some(tokio::time::Instant::now() + Duration::from_secs(self.config.start_secs));
                        } else {
                            start_deadline = None;
                        }
                    } else if self.current_child.is_none() {
                        start_deadline = None;
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
                        child.marked_running = true;
                        let pid = child.pid;
                        self.retry_count = 0;
                        self.update_status(ProgramState::Running, Some(pid), None, "Running".to_string());
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
            }
        }

        self.update_status(ProgramState::Stopped, None, None, "Stopped".to_string());
    }

    async fn handle_command(&mut self, cmd: ProgramCommand) {
        match cmd {
            ProgramCommand::Start { reply } => {
                let res = self.spawn_child().await;
                let _ = reply.send(res);
            }
            ProgramCommand::Stop {
                grace_period,
                reply,
            } => {
                self.manual_stop = true;
                self.stop_current_child(grace_period).await;
                let _ = reply.send(Ok(()));
            }
            ProgramCommand::Restart {
                grace_period,
                reply,
            } => {
                self.manual_stop = true;
                self.stop_current_child(grace_period).await;
                self.manual_stop = false;
                let res = self.spawn_child().await;
                let _ = reply.send(res);
            }
            ProgramCommand::Shutdown { reply } => {
                self.manual_stop = true;
                self.stop_current_child(Duration::from_secs(self.config.stop_wait_secs))
                    .await;
                let _ = reply.send(Ok(()));
                self.cancel_token.cancel();
            }
        }
    }

    async fn spawn_child(&mut self) -> Result<(), ProgramError> {
        if let Some(ref child) = self.current_child {
            return Err(ProgramError::AlreadyRunning {
                name: self.config.name.clone(),
                pid: child.pid,
            });
        }

        self.manual_stop = false;

        let mut cmd = tokio::process::Command::new(&self.config.command);
        cmd.args(&self.config.args);

        if let Some(ref dir) = self.config.directory {
            cmd.current_dir(dir);
        }

        for (k, v) in &self.config.environment {
            cmd.env(k, v);
        }

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

        let mut child = cmd.spawn().map_err(|e| ProgramError::StartFailed {
            name: self.config.name.clone(),
            source: e,
        })?;

        let pid = child.id().ok_or_else(|| {
            ProgramError::PlatformError("Process spawned without PID".to_string())
        })?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        // Attach platform-specific process guard
        let platform_guard = platform.attach_child(&child, pid)?;

        let max_bytes = match &self.config.logs.max_bytes {
            Some(s) => crate::logging::parse_byte_size(s).unwrap_or(20 * 1024 * 1024),
            None => 20 * 1024 * 1024,
        };
        let backups = self.config.logs.backups.unwrap_or(3);

        let stdout_rotator = if !stdout_disabled {
            if let Some(ref path) = self.config.logs.stdout {
                crate::logging::LogRotator::new(path, max_bytes, backups).ok()
            } else {
                None
            }
        } else {
            None
        };

        let stderr_rotator = if !stderr_disabled {
            if self.config.logs.redirect_stderr {
                stdout_rotator.clone()
            } else if let Some(ref path) = self.config.logs.stderr {
                crate::logging::LogRotator::new(path, max_bytes, backups).ok()
            } else {
                None
            }
        } else {
            None
        };

        let stdout_pump = if !stdout_disabled {
            stdout.map(|pipe| {
                crate::logging::spawn_log_pump(pipe, self.ring_buffer.clone(), stdout_rotator, None)
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
                    stderr_rotator,
                    stderr_prefix,
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

        self.current_child = Some(RunningChild {
            child,
            pid,
            marked_running,
            platform_guard,
            stdout_pump,
            stderr_pump,
            cancel_token: child_cancel,
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
                    let _ = child_info
                        .platform_guard
                        .wait_exit(&mut child_info.child)
                        .await;
                    None
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

        if self.manual_stop {
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
            if self.retry_count <= self.config.start_retries
                && self.config.autorestart != AutoRestartPolicy::Never
            {
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
                tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
                self.spawn_child().await.is_ok()
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
                    self.spawn_child().await.is_ok()
                }
                AutoRestartPolicy::Unexpected => {
                    if !is_expected {
                        self.update_status(
                            ProgramState::Starting,
                            None,
                            exit_code,
                            format!("Unexpected exit with code {:?}, autorestarting", exit_code),
                        );
                        self.spawn_child().await.is_ok()
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
        let mut snapshot = self.status_snapshot.write();
        snapshot.state = state;
        snapshot.pid = pid;
        snapshot.exit_code = exit_code;
        snapshot.description = description;
        if state != ProgramState::Running && state != ProgramState::Starting {
            snapshot.health = crate::program::state::HealthStatus::None;
            snapshot.is_healthy = false;
            snapshot.metrics = None;
            snapshot.uptime_secs = None;
        }
    }
}
