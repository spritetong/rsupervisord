// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::error::ProgramError;
use crate::eventlistener::pool::EventListenerPool;
use crate::eventlistener::protocol::{EventEnvelope, ListenerState, evaluate_result};
use crate::logging::RingBuffer;
use crate::program::config::{ProgramConfig, StopSignal};
use crate::program::state::{ProgramState, ProgramStatus};
use crate::program::traits::Program;
use async_trait::async_trait;
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

enum ListenerCommand {
    Start {
        reply: tokio::sync::oneshot::Sender<Result<(), ProgramError>>,
    },
    Stop {
        grace_period: Duration,
        reply: tokio::sync::oneshot::Sender<Result<(), ProgramError>>,
    },
    Restart {
        grace_period: Duration,
        reply: tokio::sync::oneshot::Sender<Result<(), ProgramError>>,
    },
    Signal {
        signal: StopSignal,
        reply: tokio::sync::oneshot::Sender<Result<(), ProgramError>>,
    },
}

pub struct EventListenerProgram {
    config: ProgramConfig,
    status_snapshot: Arc<RwLock<ProgramStatus>>,
    started_at: Arc<RwLock<Option<Instant>>>,
    ring_buffer: Arc<RingBuffer>,
    command_tx: mpsc::Sender<ListenerCommand>,
    cancel_token: CancellationToken,
}

impl EventListenerProgram {
    pub fn new(
        config: ProgramConfig,
        pool: EventListenerPool,
        cancel_token: CancellationToken,
    ) -> Result<Self, ProgramError> {
        let initial_status = ProgramStatus {
            name: config.name.clone(),
            group: config.group.clone(),
            state: ProgramState::Stopped,
            description: "Stopped".to_string(),
            pid: None,
            exit_code: None,
            is_healthy: false,
            health: crate::program::state::HealthStatus::None,
            uptime_secs: None,
            metrics: None,
            cron: None,
            next_cron_run: None,
        };

        let status_snapshot = Arc::new(RwLock::new(initial_status));
        let started_at = Arc::new(RwLock::new(None));
        let ring_buffer = Arc::new(RingBuffer::new(512));
        let (command_tx, command_rx) = mpsc::channel(16);

        let actor = EventListenerActor {
            config: config.clone(),
            pool,
            status_snapshot: status_snapshot.clone(),
            started_at: started_at.clone(),
            ring_buffer: ring_buffer.clone(),
            command_rx,
            cancel_token: cancel_token.clone(),
        };

        tokio::spawn(actor.run());

        Ok(Self {
            config,
            status_snapshot,
            started_at,
            ring_buffer,
            command_tx,
            cancel_token,
        })
    }
}

#[async_trait]
impl Program for EventListenerProgram {
    fn name(&self) -> &str {
        &self.config.name
    }

    fn priority(&self) -> u32 {
        self.config.priority
    }

    fn dependencies(&self) -> &[String] {
        &self.config.depends_on
    }

    fn status(&self) -> ProgramStatus {
        let mut status = self.status_snapshot.read().clone();
        if status.state == ProgramState::Running
            && let Some(started) = *self.started_at.read()
        {
            let elapsed = started.elapsed().as_secs();
            status.uptime_secs = Some(elapsed);
            if let Some(pid) = status.pid {
                status.description = format!(
                    "pid {}, uptime {}:{:02}:{:02}",
                    pid,
                    elapsed / 3600,
                    (elapsed % 3600) / 60,
                    elapsed % 60
                );
            }
        }
        status
    }

    async fn start(&self) -> Result<(), ProgramError> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(ListenerCommand::Start { reply: reply_tx })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: self.config.name.clone(),
            })?;
        reply_rx.await.map_err(|_| ProgramError::ChannelClosed {
            name: self.config.name.clone(),
        })?
    }

    async fn stop(&self, grace_period: Duration) -> Result<(), ProgramError> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(ListenerCommand::Stop {
                grace_period,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: self.config.name.clone(),
            })?;
        reply_rx.await.map_err(|_| ProgramError::ChannelClosed {
            name: self.config.name.clone(),
        })?
    }

    async fn restart(&self, grace_period: Duration) -> Result<(), ProgramError> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(ListenerCommand::Restart {
                grace_period,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: self.config.name.clone(),
            })?;
        reply_rx.await.map_err(|_| ProgramError::ChannelClosed {
            name: self.config.name.clone(),
        })?
    }

    async fn shutdown(&mut self) -> Result<(), ProgramError> {
        self.cancel_token.cancel();
        let _ = self.stop(Duration::from_secs(2)).await;
        Ok(())
    }

    fn read_logs(&self, max_lines: Option<usize>) -> Vec<String> {
        self.ring_buffer.get_lines(max_lines)
    }

    fn subscribe_logs(&self) -> tokio::sync::broadcast::Receiver<String> {
        self.ring_buffer.subscribe()
    }

    async fn signal(&self, signal: StopSignal) -> Result<(), ProgramError> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(ListenerCommand::Signal {
                signal,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: self.config.name.clone(),
            })?;
        reply_rx.await.map_err(|_| ProgramError::ChannelClosed {
            name: self.config.name.clone(),
        })?
    }

    async fn send_stdin(&self, _data: Vec<u8>) -> Result<(), ProgramError> {
        Err(ProgramError::PlatformError(
            "Cannot send arbitrary stdin to event listener process".to_string(),
        ))
    }
}

struct EventListenerChild {
    child: tokio::process::Child,
    platform_guard: Box<dyn crate::platform::PlatformProcessGuard>,
    cancel_token: CancellationToken,
}

struct EventListenerActor {
    config: ProgramConfig,
    pool: EventListenerPool,
    status_snapshot: Arc<RwLock<ProgramStatus>>,
    started_at: Arc<RwLock<Option<Instant>>>,
    ring_buffer: Arc<RingBuffer>,
    command_rx: mpsc::Receiver<ListenerCommand>,
    cancel_token: CancellationToken,
}

impl EventListenerActor {
    async fn run(mut self) {
        let mut child_process: Option<EventListenerChild> = None;

        // Autostart if configured
        if self.config.autostart
            && let Ok(child) = self.spawn_child().await
        {
            child_process = Some(child);
        }

        loop {
            tokio::select! {
                _ = self.cancel_token.cancelled() => {
                    if let Some(mut c) = child_process.take() {
                        self.stop_child(&mut c, Duration::from_secs(self.config.stop_wait_secs)).await;
                    }
                    break;
                }
                cmd = self.command_rx.recv() => {
                    match cmd {
                        Some(ListenerCommand::Start { reply }) => {
                            if child_process.is_some() {
                                let _ = reply.send(Ok(()));
                            } else {
                                match self.spawn_child().await {
                                    Ok(c) => {
                                        child_process = Some(c);
                                        let _ = reply.send(Ok(()));
                                    }
                                    Err(e) => {
                                        let _ = reply.send(Err(e));
                                    }
                                }
                            }
                        }
                        Some(ListenerCommand::Stop { grace_period, reply }) => {
                            if let Some(mut c) = child_process.take() {
                                self.stop_child(&mut c, grace_period).await;
                                self.set_state(ProgramState::Stopped, None, "Stopped by user");
                            }
                            let _ = reply.send(Ok(()));
                        }
                        Some(ListenerCommand::Restart { grace_period, reply }) => {
                            if let Some(mut c) = child_process.take() {
                                self.stop_child(&mut c, grace_period).await;
                            }
                            match self.spawn_child().await {
                                Ok(c) => {
                                    child_process = Some(c);
                                    let _ = reply.send(Ok(()));
                                }
                                Err(e) => {
                                    let _ = reply.send(Err(e));
                                }
                            }
                        }
                        Some(ListenerCommand::Signal { signal, reply }) => {
                            let res = if let Some(ref c) = child_process {
                                c.platform_guard.send_stop_signal(signal)
                            } else {
                                Err(ProgramError::NotRunning {
                                    name: self.config.name.clone(),
                                })
                            };
                            let _ = reply.send(res);
                        }
                        None => break,
                    }
                }
                status = async {
                    match child_process.as_mut() {
                        Some(c) => c.child.wait().await,
                        None => std::future::pending().await,
                    }
                } => {
                    // Child exited
                    let exit_code = status.ok().and_then(|s| s.code());
                    if let Some(c) = child_process.take() {
                        c.cancel_token.cancel();
                    }

                    self.set_state(ProgramState::Exited, exit_code, "Process exited");

                    // Check autorestart
                    if self.config.autorestart.should_restart(exit_code.unwrap_or(-1), &self.config.exit_codes) {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        if let Ok(c) = self.spawn_child().await {
                            child_process = Some(c);
                        }
                    }
                }
            }
        }
    }

    async fn spawn_child(&self) -> Result<EventListenerChild, ProgramError> {
        let mut cmd = tokio::process::Command::new(&self.config.command);
        cmd.args(&self.config.args);

        if let Some(ref dir) = self.config.directory {
            cmd.current_dir(dir);
        }

        for (k, v) in &self.config.environment {
            cmd.env(k, v);
        }

        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let platform = crate::platform::native_platform();
        platform.configure_command(&mut cmd, self.config.user.as_deref(), self.config.umask)?;

        let mut child = cmd.spawn().map_err(|e| ProgramError::StartFailed {
            name: self.config.name.clone(),
            source: e,
        })?;

        let pid = child.id().ok_or_else(|| {
            ProgramError::PlatformError("Process spawned without PID".to_string())
        })?;

        let platform_guard = platform.attach_child(&child, pid)?;

        let stdin = child.stdin.take().ok_or_else(|| {
            ProgramError::PlatformError("Failed to capture child stdin".to_string())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            ProgramError::PlatformError("Failed to capture child stdout".to_string())
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            ProgramError::PlatformError("Failed to capture child stderr".to_string())
        })?;

        *self.started_at.write() = Some(Instant::now());
        self.set_state(
            ProgramState::Running,
            Some(pid as i32),
            &format!("pid {}, uptime 0:00:00", pid),
        );

        let child_cancel = CancellationToken::new();

        // 1. Spawn stderr reader task into ring buffer
        let ring = self.ring_buffer.clone();
        let cancel_err = child_cancel.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            loop {
                tokio::select! {
                    _ = cancel_err.cancelled() => break,
                    line_res = lines.next_line() => {
                        match line_res {
                            Ok(Some(line)) => ring.push(line),
                            _ => break,
                        }
                    }
                }
            }
        });

        // 2. Spawn wire protocol handshake task
        let pool = self.pool.clone();
        let name = self.config.name.clone();
        let cancel_wire = child_cancel.clone();
        let server_id = pool.server_identifier().to_string();

        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut writer = stdin;
            let mut state = ListenerState::Acknowledged;
            let mut current_envelope: Option<EventEnvelope> = None;
            let mut unexpected_buf = String::new();

            loop {
                tokio::select! {
                    _ = cancel_wire.cancelled() => break,
                    _ = pool.inner().cancel_token.cancelled() => break,
                    action = async {
                        match state {
                            ListenerState::Acknowledged => {
                                let mut line = String::new();
                                match reader.read_line(&mut line).await {
                                    Ok(0) => None, // EOF
                                    Ok(_) => {
                                        let trimmed = line.trim();
                                        if trimmed == "READY" {
                                            state = ListenerState::Ready;
                                            pool.inner().dispatch_notify.notify_waiters();
                                        } else {
                                            tracing::warn!("{}: has entered the UNKNOWN state", name);
                                            state = ListenerState::Unknown;
                                        }
                                        Some(())
                                    }
                                    Err(_) => None,
                                }
                            }
                            ListenerState::Ready => {
                                tokio::select! {
                                    biased;
                                    line_res = reader.read_line(&mut unexpected_buf) => {
                                        unexpected_buf.clear();
                                        match line_res {
                                            Ok(0) => None, // EOF
                                            Ok(_) => {
                                                tracing::warn!("{}: has entered the UNKNOWN state", name);
                                                state = ListenerState::Unknown;
                                                Some(())
                                            }
                                            Err(_) => None,
                                        }
                                    }
                                    next_env = async {
                                        loop {
                                            if let Some(env) = pool.inner().pop_next_event() {
                                                return env;
                                            }
                                            pool.inner().dispatch_notify.notified().await;
                                        }
                                    } => {
                                        let wire = next_env.format_wire(&server_id);
                                        if writer.write_all(wire.as_bytes()).await.is_err()
                                            || writer.flush().await.is_err()
                                        {
                                            tracing::warn!("{}: failed to write event envelope", name);
                                            pool.inner().requeue_rejected(next_env);
                                            return None;
                                        }
                                        current_envelope = Some(next_env);
                                        state = ListenerState::Busy;
                                        Some(())
                                    }
                                }
                            }
                            ListenerState::Busy => {
                                let mut line = String::new();
                                match reader.read_line(&mut line).await {
                                    Ok(0) => None, // EOF
                                    Ok(_) => {
                                        let trimmed = line.trim_end_matches(['\r', '\n']);
                                        if let Some(len_str) = trimmed.strip_prefix("RESULT ") {
                                            if let Ok(expected_len) = len_str.trim().parse::<usize>() {
                                                let mut body_buf = vec![0u8; expected_len];
                                                if reader.read_exact(&mut body_buf).await.is_ok() {
                                                    let body_str = String::from_utf8_lossy(&body_buf);
                                                    if evaluate_result(&body_str) == crate::eventlistener::protocol::ProcessResult::Ok {
                                                        current_envelope = None;
                                                    } else if let Some(env) = current_envelope.take() {
                                                        pool.inner().requeue_rejected(env);
                                                    }
                                                    state = ListenerState::Acknowledged;
                                                    Some(())
                                                } else {
                                                    tracing::warn!("{}: has entered the UNKNOWN state", name);
                                                    state = ListenerState::Unknown;
                                                    if let Some(env) = current_envelope.take() {
                                                        pool.inner().requeue_rejected(env);
                                                    }
                                                    Some(())
                                                }
                                            } else {
                                                tracing::warn!("{}: has entered the UNKNOWN state", name);
                                                state = ListenerState::Unknown;
                                                if let Some(env) = current_envelope.take() {
                                                    pool.inner().requeue_rejected(env);
                                                }
                                                Some(())
                                            }
                                        } else {
                                            tracing::warn!("{}: has entered the UNKNOWN state", name);
                                            state = ListenerState::Unknown;
                                            if let Some(env) = current_envelope.take() {
                                                pool.inner().requeue_rejected(env);
                                            }
                                            Some(())
                                        }
                                    }
                                    Err(_) => None,
                                }
                            }
                            ListenerState::Unknown => {
                                // Wait indefinitely until child process terminates or task cancelled
                                std::future::pending::<Option<()>>().await
                            }
                        }
                    } => {
                        if action.is_none() {
                            // EOF or I/O error on child stream
                            if let Some(env) = current_envelope.take() {
                                pool.inner().requeue_rejected(env);
                            }
                            break;
                        }
                    }
                }
            }
        });

        Ok(EventListenerChild {
            child,
            platform_guard,
            cancel_token: child_cancel,
        })
    }

    async fn stop_child(&self, child_info: &mut EventListenerChild, grace_period: Duration) {
        child_info.cancel_token.cancel();
        let _ = child_info
            .platform_guard
            .send_stop_signal(self.config.stop_signal);

        let wait_res = tokio::time::timeout(
            grace_period,
            child_info.platform_guard.wait_exit(&mut child_info.child),
        )
        .await;

        if wait_res.is_err() {
            let _ = child_info.platform_guard.force_kill();
            let _ = child_info.child.kill().await;
            let _ = tokio::time::timeout(
                Duration::from_secs(2),
                child_info.platform_guard.wait_exit(&mut child_info.child),
            )
            .await;
        }
    }

    fn set_state(&self, state: ProgramState, pid_or_code: Option<i32>, desc: &str) {
        let mut snap = self.status_snapshot.write();
        snap.state = state;
        snap.description = desc.to_string();
        if state == ProgramState::Running {
            snap.pid = pid_or_code.map(|p| p as u32);
            snap.exit_code = None;
        } else {
            snap.pid = None;
            snap.exit_code = pid_or_code;
        }
    }
}
