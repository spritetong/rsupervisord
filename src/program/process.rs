// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::consts::*;
use crate::error::ProgramError;
use crate::logging::RingBuffer;
use crate::platform::PlatformProcessGuard;
use crate::program::config::{AutoRestartPolicy, ProgramConfig, StopSignal};
use crate::program::state::{ProgramState, ProgramStatus};
use crate::program::traits::Program;
use async_trait::async_trait;
use parking_lot::RwLock;
use std::path::PathBuf;
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
    Signal {
        signal: StopSignal,
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
    in_memory_rotator: Arc<crate::logging::InMemoryLogRotator>,
    started_at: Arc<RwLock<Option<Instant>>>,
    stdin_tx: Arc<RwLock<Option<mpsc::Sender<Vec<u8>>>>>,
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
        let in_memory_buffer_size = config.logs.effective_buffer_size();
        let seg_size = (in_memory_buffer_size / 2).max(1024);
        let in_memory_rotator = Arc::new(crate::logging::InMemoryLogRotator::new(seg_size, 1));

        if config.logs.is_in_memory_only() {
            in_memory_rotator.seed_from_files(
                config.logs.stdout.as_deref(),
                config.logs.stderr.as_deref(),
                seg_size,
            );
        }

        let started_at = Arc::new(RwLock::new(None));
        let stdin_tx = Arc::new(RwLock::new(None));
        let cancel_token = CancellationToken::new();
        let (command_tx, command_rx) = mpsc::channel(32);

        let actor = ProgramActor::new(
            config.clone(),
            command_rx,
            status_snapshot.clone(),
            ring_buffer.clone(),
            started_at.clone(),
            stdin_tx.clone(),
            activity_tracker,
            event_hub.clone(),
            cancel_token.clone(),
            in_memory_rotator.clone(),
        );

        let actor_handle = tokio::spawn(actor.run());
        let cancel_guard = cancel_token.clone().drop_guard();

        Ok(Self {
            config,
            command_tx,
            status_snapshot,
            ring_buffer,
            in_memory_rotator,
            started_at,
            stdin_tx,
            event_hub,
            cancel_token,
            _cancel_guard: cancel_guard,
            actor_handle: Some(actor_handle),
        })
    }

    /// Returns a reference to the in-memory log rotator for this process.
    pub fn in_memory_rotator(&self) -> &Arc<crate::logging::InMemoryLogRotator> {
        &self.in_memory_rotator
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

    /// Resolves the configured log file path for a channel, checking redirect_stderr constraints.
    fn resolve_channel_path(
        &self,
        channel: crate::logging::LogChannel,
    ) -> Result<Option<&std::path::Path>, ProgramError> {
        match channel {
            crate::logging::LogChannel::Stderr => {
                if self.config.logs.redirect_stderr {
                    Err(ProgramError::ReadLogFailed {
                        name: self.config.name.clone(),
                        error: "no log file".to_string(),
                    })
                } else {
                    Ok(self.config.logs.stderr.as_deref())
                }
            }
            crate::logging::LogChannel::Stdout => Ok(self.config.logs.stdout.as_deref()),
        }
    }
}

#[async_trait]
impl Program for ProcessProgram {
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

        let timeout_dur = AWAIT_QUERY;
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
            .checked_add(PROCESS_STOP_GRACE_EXTRA)
            .unwrap_or(MAX_TIMEOUT);
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
            .checked_add(PROCESS_RESTART_GRACE_EXTRA)
            .unwrap_or(MAX_TIMEOUT);
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
            let timeout_dur = self.config.stop_wait_secs + std::time::Duration::from_secs(2);
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

    async fn signal(&self, signal: StopSignal) -> Result<(), ProgramError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ProgramCommand::Signal {
                signal,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ProgramError::ChannelClosed {
                name: self.config.name.clone(),
            })?;

        let timeout_dur = AWAIT_QUERY;
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

    async fn send_stdin(&self, data: Vec<u8>) -> Result<(), ProgramError> {
        let tx = {
            let guard = self.stdin_tx.read();
            guard.clone()
        };
        let Some(tx) = tx else {
            return Err(ProgramError::NotRunning {
                name: self.config.name.clone(),
            });
        };

        let timeout_dur = AWAIT_QUERY;
        match tokio::time::timeout(timeout_dur, tx.send(data)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_closed)) => Err(ProgramError::NotRunning {
                name: self.config.name.clone(),
            }),
            Err(_) => Err(ProgramError::StdinWriteTimeout {
                name: self.config.name.clone(),
                timeout_secs: timeout_dur.as_secs(),
            }),
        }
    }

    fn read_log(
        &self,
        channel: crate::logging::LogChannel,
        offset: i64,
        length: i64,
    ) -> Result<(String, i64, bool), ProgramError> {
        if self.config.logs.is_in_memory_only() {
            if channel == crate::logging::LogChannel::Stderr && self.config.logs.redirect_stderr {
                return Err(ProgramError::ReadLogFailed {
                    name: self.config.name.clone(),
                    error: "no log file".to_string(),
                });
            }
            if offset < 0 && length != 0 {
                return Err(ProgramError::ReadLogFailed {
                    name: self.config.name.clone(),
                    error: "length must be 0 when offset is negative".to_string(),
                });
            }
            if offset >= 0 && length < 0 {
                return Err(ProgramError::ReadLogFailed {
                    name: self.config.name.clone(),
                    error: "length cannot be negative".to_string(),
                });
            }
            Ok(crate::logging::InstantLogReader::read_bytes(
                &*self.in_memory_rotator,
                channel,
                offset,
                length,
            ))
        } else {
            let configured_path = self.resolve_channel_path(channel)?;
            if let Some(path) = configured_path.filter(|p| p.exists()) {
                crate::logging::LogFileReader::read_bytes(path, offset, length)
                    .map(|s| (s, 0, false))
                    .map_err(|e| ProgramError::ReadLogFailed {
                        name: self.config.name.clone(),
                        error: e.to_string(),
                    })
            } else if channel == crate::logging::LogChannel::Stdout {
                let lines = self.ring_buffer.get_lines(None);
                if lines.is_empty() && configured_path.is_some() {
                    return Err(ProgramError::ReadLogFailed {
                        name: self.config.name.clone(),
                        error: "no log file".to_string(),
                    });
                }
                let mut full_text = lines.join("\n");
                if !full_text.is_empty() {
                    full_text.push('\n');
                }
                crate::logging::LogFileReader::read_bytes_from_slice(
                    full_text.as_bytes(),
                    offset,
                    length,
                )
                .map(|s| (s, 0, false))
                .map_err(|e| ProgramError::ReadLogFailed {
                    name: self.config.name.clone(),
                    error: e.to_string(),
                })
            } else {
                Err(ProgramError::ReadLogFailed {
                    name: self.config.name.clone(),
                    error: "no log file".to_string(),
                })
            }
        }
    }

    fn tail_log(
        &self,
        channel: crate::logging::LogChannel,
        offset: i64,
        length: i64,
    ) -> Result<(String, i64, bool), ProgramError> {
        if channel == crate::logging::LogChannel::Stderr && self.config.logs.redirect_stderr {
            return Ok((String::new(), offset, false));
        }

        if self.config.logs.is_in_memory_only() {
            Ok(crate::logging::InstantLogReader::tail_bytes(
                &*self.in_memory_rotator,
                channel,
                offset,
                length,
            ))
        } else {
            let configured_path = self.resolve_channel_path(channel)?;
            if let Some(path) = configured_path.filter(|p| p.exists()) {
                Ok(crate::logging::LogFileReader::tail_bytes(
                    path, offset, length,
                ))
            } else if channel == crate::logging::LogChannel::Stdout {
                let (data, new_off, overflow) = crate::logging::InstantLogReader::tail_bytes(
                    &*self.in_memory_rotator,
                    channel,
                    offset,
                    length,
                );
                if data.is_empty() {
                    let lines = self.ring_buffer.get_lines(None);
                    let mut full_text = lines.join("\n");
                    if !full_text.is_empty() {
                        full_text.push('\n');
                    }
                    Ok(crate::logging::LogFileReader::tail_bytes_from_slice(
                        full_text.as_bytes(),
                        offset,
                        length,
                    ))
                } else {
                    Ok((data, new_off, overflow))
                }
            } else {
                Ok((String::new(), offset, false))
            }
        }
    }

    fn clear_logs(&self) -> Result<(), ProgramError> {
        self.ring_buffer.clear();
        self.in_memory_rotator.stdout().clear();
        self.in_memory_rotator.stderr().clear();
        if let Some(ref path) = self.config.logs.stdout
            && path.exists()
        {
            let _ = std::fs::write(path, "");
        }
        if let Some(ref path) = self.config.logs.stderr
            && path.exists()
        {
            let _ = std::fs::write(path, "");
        }
        Ok(())
    }
}

const MAX_STDIN_BUFFER_BYTES: usize = 64 * 1024;

/// Builds a local syslog backend from program log settings and composites it onto `base`.
/// Errors are logged (fail-loud) and `base` is returned unchanged so file sinks keep working.
fn attach_syslog_backend(
    base: Option<Arc<dyn crate::logging::LogBackend>>,
    logs: &crate::program::config::ProgramLogsConfig,
    priority: Option<&str>,
    program_name: &str,
) -> Option<Arc<dyn crate::logging::LogBackend>> {
    let facility = logs
        .syslog_facility
        .as_deref()
        .and_then(|f| f.parse::<crate::logging::SyslogFacility>().ok())
        .unwrap_or_default();
    let severity = priority
        .and_then(|p| p.parse::<crate::logging::SyslogSeverity>().ok())
        .unwrap_or_default();
    let tag = logs
        .syslog_tag
        .clone()
        .unwrap_or_else(|| program_name.to_string());
    match crate::logging::SyslogLogBackend::new(
        crate::logging::SyslogTarget::Local,
        facility,
        severity,
        tag,
    ) {
        Ok(syslog_b) => {
            let syslog_arc: Arc<dyn crate::logging::LogBackend> = Arc::new(syslog_b);
            Some(match base {
                Some(b) => Arc::new(crate::logging::CompositeLogBackend::new(vec![
                    b, syslog_arc,
                ])),
                None => syslog_arc,
            })
        }
        Err(e) => {
            tracing::error!(
                "Failed to initialize syslog backend for '{}': {}",
                program_name,
                e
            );
            base
        }
    }
}

fn spawn_stdin_writer(
    program_name: String,
    mut child_stdin: tokio::process::ChildStdin,
    mut rx: mpsc::Receiver<Vec<u8>>,
    cancel_token: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        use bytes::{Buf, BytesMut};
        use tokio::io::AsyncWriteExt;

        let mut buffer = BytesMut::with_capacity(8192);

        loop {
            tokio::select! {
                biased;

                _ = cancel_token.cancelled() => {
                    break;
                }

                // Drain branch: active only if internal buffer has data to write to the OS pipe
                write_res = child_stdin.write(&buffer[..]), if !buffer.is_empty() => {
                    match write_res {
                        Ok(0) => {
                            tracing::debug!(program = %program_name, "Child stdin closed by reader (broken pipe)");
                            break;
                        }
                        Ok(n) => {
                            buffer.advance(n);
                        }
                        Err(e) => {
                            tracing::debug!(program = %program_name, error = %e, "Child stdin write failed");
                            break;
                        }
                    }
                }

                // Read branch: active only if internal buffer has not reached the backpressure limit
                msg = rx.recv(), if buffer.len() < MAX_STDIN_BUFFER_BYTES => {
                    match msg {
                        Some(chunk) => {
                            // Append into buffer without awaiting or blocking
                            buffer.extend_from_slice(&chunk);
                        }
                        None => {
                            break;
                        }
                    }
                }
            }
        }

        // Best-effort drain of remaining buffer before dropping pipe (bounded by 500ms)
        if !buffer.is_empty() && !cancel_token.is_cancelled() {
            let _ = tokio::time::timeout(SHORT_RETRY_DELAY, async {
                while !buffer.is_empty() {
                    match child_stdin.write(&buffer[..]).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buffer.advance(n),
                    }
                }
                let _ = child_stdin.flush().await;
            })
            .await;
        }

        // Dropping child_stdin explicitly closes the pipe handle, sending EOF to child
        drop(child_stdin);
        tracing::debug!(program = %program_name, "Stdin writer task finished");
    })
}

struct RunningChild {
    child: tokio::process::Child,
    pid: u32,
    marked_running: bool,
    platform_guard: Box<dyn PlatformProcessGuard>,
    stdout_pump: Option<JoinHandle<()>>,
    stderr_pump: Option<JoinHandle<()>>,
    stdin_writer: Option<JoinHandle<()>>,
    cancel_token: CancellationToken,
    _cancel_guard: tokio_util::sync::DropGuard,
    health_task: Option<JoinHandle<()>>,
}

impl RunningChild {
    async fn drain_pumps(&mut self) {
        self.cancel_token.cancel();
        let stdin_w = self.stdin_writer.take();
        let health = self.health_task.take();
        let out = self.stdout_pump.take();
        let err = self.stderr_pump.take();

        let stdin_abort = stdin_w.as_ref().map(|h| h.abort_handle());
        let health_abort = health.as_ref().map(|h| h.abort_handle());
        let out_abort = out.as_ref().map(|h| h.abort_handle());
        let err_abort = err.as_ref().map(|h| h.abort_handle());

        let hard_deadline = tokio::time::timeout(DRAIN_TIMEOUT, async {
            if let Some(w) = stdin_w {
                let _ = w.await;
            }
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
                "Log/stdin pump drain timed out (inherited pipe by grandchild?); aborting pumps"
            );
            if let Some(ref a) = stdin_abort {
                a.abort();
            }
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

impl Drop for RunningChild {
    fn drop(&mut self) {
        self.cancel_token.cancel();
        if let Some(w) = self.stdin_writer.take() {
            w.abort();
        }
        if let Some(h) = self.health_task.take() {
            h.abort();
        }
        if let Some(o) = self.stdout_pump.take() {
            o.abort();
        }
        if let Some(e) = self.stderr_pump.take() {
            e.abort();
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
    stdin_tx: Arc<RwLock<Option<mpsc::Sender<Vec<u8>>>>>,
    activity_tracker: crate::manager::ActivityTracker,
    event_hub: crate::manager::EventHub,
    cancel_token: CancellationToken,
    current_child: Option<RunningChild>,
    retry_count: u32,
    manual_stop: bool,
    is_shutting_down: bool,
    backoff_deadline: Option<tokio::time::Instant>,
    stdout_backend: Option<Arc<dyn crate::logging::LogBackend>>,
    stderr_backend: Option<Arc<dyn crate::logging::LogBackend>>,
}

impl ProgramActor {
    #[allow(clippy::too_many_arguments)]
    fn new(
        config: ProgramConfig,
        command_rx: mpsc::Receiver<ProgramCommand>,
        status_snapshot: Arc<RwLock<ProgramStatus>>,
        ring_buffer: Arc<RingBuffer>,
        started_at: Arc<RwLock<Option<Instant>>>,
        stdin_tx: Arc<RwLock<Option<mpsc::Sender<Vec<u8>>>>>,
        activity_tracker: crate::manager::ActivityTracker,
        event_hub: crate::manager::EventHub,
        cancel_token: CancellationToken,
        in_memory_rotator: Arc<crate::logging::InMemoryLogRotator>,
    ) -> Self {
        let (health_tx, health_rx) = mpsc::channel(16);

        let stdout_max_bytes = config.logs.effective_stdout_max_bytes();
        let stdout_backups = config.logs.effective_stdout_backups();
        let stderr_max_bytes = config.logs.effective_stderr_max_bytes();
        let stderr_backups = config.logs.effective_stderr_backups();
        let stdout_disabled = config.logs.is_stdout_disabled();
        let stderr_disabled = config.logs.is_stderr_disabled();

        let (stdout_backend, stderr_backend) = if config.logs.is_in_memory_only() {
            let mut out_b: Option<Arc<dyn crate::logging::LogBackend>> = if !stdout_disabled {
                Some(in_memory_rotator.stdout().clone() as Arc<dyn crate::logging::LogBackend>)
            } else {
                None
            };
            if config.logs.stdout_syslog && !stdout_disabled {
                out_b = attach_syslog_backend(
                    out_b,
                    &config.logs,
                    config.logs.syslog_stdout_priority.as_deref(),
                    &config.name,
                );
            }

            let err_b = if !stderr_disabled {
                if config.logs.redirect_stderr {
                    let mut b = out_b.clone();
                    if config.logs.stderr_syslog && !config.logs.stdout_syslog {
                        b = attach_syslog_backend(
                            b,
                            &config.logs,
                            config.logs.syslog_stderr_priority.as_deref(),
                            &config.name,
                        );
                    }
                    b
                } else {
                    let mut b: Option<Arc<dyn crate::logging::LogBackend>> =
                        Some(in_memory_rotator.stderr().clone()
                            as Arc<dyn crate::logging::LogBackend>);
                    if config.logs.stderr_syslog {
                        b = attach_syslog_backend(
                            b,
                            &config.logs,
                            config.logs.syslog_stderr_priority.as_deref(),
                            &config.name,
                        );
                    }
                    b
                }
            } else {
                None
            };

            (out_b, err_b)
        } else {
            let stdout_dest = match config.logs.stdout.as_ref() {
                Some(p) => {
                    let s = p.to_string_lossy();
                    match crate::logging::LogDestination::parse(&s) {
                        Ok(d) => d,
                        Err(e) => {
                            tracing::error!(
                                "Program '{}' invalid stdout log destination '{}': {}; falling back to AUTO",
                                config.name,
                                s,
                                e
                            );
                            crate::logging::LogDestination::Auto
                        }
                    }
                }
                None => crate::logging::LogDestination::Auto,
            };

            let stdout_opts = crate::logging::BackendBuildOptions {
                program_name: &config.name,
                channel: crate::logging::LogChannel::Stdout,
                max_bytes: stdout_max_bytes,
                backups: stdout_backups,
                timestamp_suffix: config.logs.stdout_timestamp_suffix,
                syslog_facility: config.logs.syslog_facility.as_deref(),
                syslog_tag: config.logs.syslog_tag.as_deref(),
                syslog_priority: config.logs.syslog_stdout_priority.as_deref(),
            };

            let mut stdout_backend = if !stdout_disabled {
                match stdout_dest.build_backend(&stdout_opts) {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::error!(
                            "Failed to initialize stdout log backend for '{}': {}",
                            config.name,
                            e
                        );
                        None
                    }
                }
            } else {
                None
            };

            // Attach flag-based syslog only when the destination does not already include it.
            if config.logs.stdout_syslog && !stdout_disabled && !stdout_dest.contains_syslog() {
                stdout_backend = attach_syslog_backend(
                    stdout_backend,
                    &config.logs,
                    config.logs.syslog_stdout_priority.as_deref(),
                    &config.name,
                );
            }

            let stderr_backend = if !stderr_disabled {
                if config.logs.redirect_stderr {
                    let mut b = stdout_backend.clone();
                    // Honor stderr_syslog on the shared (redirected) backend only when
                    // the shared backend does not already include a syslog sink.
                    if config.logs.stderr_syslog
                        && !config.logs.stdout_syslog
                        && !stdout_dest.contains_syslog()
                    {
                        b = attach_syslog_backend(
                            b,
                            &config.logs,
                            config.logs.syslog_stderr_priority.as_deref(),
                            &config.name,
                        );
                    }
                    b
                } else {
                    let stderr_dest = match config.logs.stderr.as_ref() {
                        Some(p) => {
                            let s = p.to_string_lossy();
                            match crate::logging::LogDestination::parse(&s) {
                                Ok(d) => d,
                                Err(e) => {
                                    tracing::error!(
                                        "Program '{}' invalid stderr log destination '{}': {}; falling back to AUTO",
                                        config.name,
                                        s,
                                        e
                                    );
                                    crate::logging::LogDestination::Auto
                                }
                            }
                        }
                        None => crate::logging::LogDestination::Auto,
                    };

                    let stderr_opts = crate::logging::BackendBuildOptions {
                        program_name: &config.name,
                        channel: crate::logging::LogChannel::Stderr,
                        max_bytes: stderr_max_bytes,
                        backups: stderr_backups,
                        timestamp_suffix: config.logs.stderr_timestamp_suffix,
                        syslog_facility: config.logs.syslog_facility.as_deref(),
                        syslog_tag: config.logs.syslog_tag.as_deref(),
                        syslog_priority: config.logs.syslog_stderr_priority.as_deref(),
                    };

                    let mut b = match stderr_dest.build_backend(&stderr_opts) {
                        Ok(b) => b,
                        Err(e) => {
                            tracing::error!(
                                "Failed to initialize stderr log backend for '{}': {}",
                                config.name,
                                e
                            );
                            None
                        }
                    };

                    if config.logs.stderr_syslog && !stderr_dest.contains_syslog() {
                        b = attach_syslog_backend(
                            b,
                            &config.logs,
                            config.logs.syslog_stderr_priority.as_deref(),
                            &config.name,
                        );
                    }
                    b
                }
            } else {
                None
            };

            (stdout_backend, stderr_backend)
        };

        Self {
            config,
            command_rx,
            health_rx,
            health_tx,
            status_snapshot,
            ring_buffer,
            started_at,
            stdin_tx,
            activity_tracker,
            event_hub,
            cancel_token,
            current_child: None,
            retry_count: 0,
            manual_stop: false,
            is_shutting_down: false,
            backoff_deadline: None,
            stdout_backend,
            stderr_backend,
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
                                self.stop_current_child(self.config.stop_wait_secs).await;
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
                        if !marked_running && !self.config.start_secs.is_zero() {
                            start_deadline = Some(
                                tokio::time::Instant::now() + self.config.start_secs,
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
                    self.stop_current_child(self.config.stop_wait_secs).await;
                                if let Err(e) = self.spawn_child().await {
                                    tracing::error!(program = %self.config.name, error = %e, "Failed to restart child after health check failure");
                                    start_deadline = None;
                                    self.update_status(
                                        ProgramState::Fatal,
                                        None,
                                        None,
                                        format!("Health check restart failed: {}", e),
                                    );
                                } else if !self.config.start_secs.is_zero() {
                                    start_deadline = Some(tokio::time::Instant::now() + self.config.start_secs);
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
                    } else if !self.config.start_secs.is_zero() {
                        start_deadline = Some(tokio::time::Instant::now() + self.config.start_secs);
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
                    if restarted && !self.config.start_secs.is_zero() {
                        start_deadline = Some(tokio::time::Instant::now() + self.config.start_secs);
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
                self.stop_current_child(self.config.stop_wait_secs).await;
                let _ = reply.send(Ok(()));
                self.cancel_token.cancel();
            }
            ProgramCommand::Signal { signal, reply } => {
                if let Some(ref child) = self.current_child {
                    let res = child.platform_guard.send_stop_signal(signal);
                    let _ = reply.send(res);
                } else {
                    let _ = reply.send(Err(ProgramError::NotRunning {
                        name: self.config.name.clone(),
                    }));
                }
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

            let timeout_dur = self.config.hook_timeout_secs;
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

        let platform = crate::platform::native_platform();
        let cmd_path = platform
            .resolve_executable(&self.config.command, self.config.directory.as_deref())
            .unwrap_or_else(|| PathBuf::from(&self.config.command));
        let mut cmd = platform.build_command(&cmd_path, &self.config.args);

        if let Some(ref dir) = self.config.directory {
            cmd.current_dir(dir);
        }

        // OI-2: load `.env` files first; program `environment` wins on conflict.
        for (k, v) in crate::program::envfile::load_env_files(&self.config.env_files) {
            cmd.env(k, v);
        }
        for (k, v) in &self.config.environment {
            cmd.env(k, v);
        }

        cmd.stdin(std::process::Stdio::piped());

        let stdout_disabled = self.config.logs.is_stdout_disabled();
        let stderr_disabled = self.config.logs.is_stderr_disabled();

        let transport_config = crate::platform::ProcessTransportConfig {
            program_name: self.config.name.clone(),
            capture_stdout: !stdout_disabled,
            capture_stderr: !stderr_disabled,
            redirect_stderr: self.config.logs.redirect_stderr,
        };

        let mut transport = platform
            .create_process_log_transport(&transport_config)
            .await?;
        let mut stdio_handles = transport.take_child_stdio()?;

        // Configure async transport handles or null stdio for stdout and stderr capture
        if let Some(stdout) = stdio_handles.stdout.take() {
            cmd.stdout(stdout);
        } else {
            cmd.stdout(std::process::Stdio::null());
        }

        if let Some(stderr) = stdio_handles.stderr.take() {
            cmd.stderr(stderr);
        } else {
            cmd.stderr(std::process::Stdio::null());
        }

        // Platform-agnostic pre-spawn configuration via PlatformBackend
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

        let stdin = child_guard.stdin.take();
        let streams = transport.into_streams()?;

        // Attach platform-specific process guard (OI-9: group vs single-pid)
        let platform_guard = platform.attach_child(
            &child_guard,
            pid,
            self.config.stop_as_group,
            self.config.kill_as_group,
        )?;

        let stdout_pump = if !stdout_disabled {
            streams.stdout.map(|pipe| {
                crate::logging::LogPumpBuilder::new(pipe, self.ring_buffer.clone(), "stdout")
                    .with_backend(self.stdout_backend.clone())
                    .with_event_hub(Some(self.event_hub.clone()))
                    .with_program_name(Some(self.config.name.clone()))
                    .with_group_name(Some(self.config.group.clone()))
                    .with_pid(Some(pid))
                    .with_events_enabled(self.config.logs.stdout_events_enabled)
                    .spawn()
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
            streams.stderr.map(|pipe| {
                crate::logging::LogPumpBuilder::new(pipe, self.ring_buffer.clone(), "stderr")
                    .with_backend(self.stderr_backend.clone())
                    .with_ring_prefix(stderr_prefix)
                    .with_event_hub(Some(self.event_hub.clone()))
                    .with_program_name(Some(self.config.name.clone()))
                    .with_group_name(Some(self.config.group.clone()))
                    .with_pid(Some(pid))
                    .with_events_enabled(self.config.logs.stderr_events_enabled)
                    .spawn()
            })
        } else {
            None
        };

        *self.started_at.write() = Some(Instant::now());
        let child_cancel = CancellationToken::new();

        let stdin_writer = if let Some(cin) = stdin {
            let (tx, rx) = mpsc::channel(16);
            *self.stdin_tx.write() = Some(tx);
            Some(spawn_stdin_writer(
                self.config.name.clone(),
                cin,
                rx,
                child_cancel.clone(),
            ))
        } else {
            *self.stdin_tx.write() = None;
            None
        };

        let health_task = if let Some(ref hcfg) = self.config.health_check {
            let runner = crate::manager::HealthProbeRunner::new(
                &self.config.name,
                hcfg.clone(),
                self.config.directory.clone(),
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

        let marked_running = self.config.start_secs.is_zero();
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
            stdin_writer,
            cancel_token: child_cancel,
            _cancel_guard: cancel_guard,
            health_task,
        });

        Ok(())
    }

    async fn stop_current_child(&mut self, grace_period: Duration) {
        *self.started_at.write() = None;
        *self.stdin_tx.write() = None;
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

                let timeout_dur = self.config.hook_timeout_secs;
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
                    // OI-3: honor per-program killwaitsecs (default = DRAIN_TIMEOUT).
                    let post_kill_wait = tokio::time::timeout(
                        self.config.kill_wait_secs,
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
        *self.stdin_tx.write() = None;
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
                let backoff_delay = if !self.config.restart_pause_secs.is_zero() {
                    self.config.restart_pause_secs
                } else {
                    Duration::from_secs(2u64.pow(self.retry_count.min(BACKOFF_MAX_EXPONENT)))
                };
                self.backoff_deadline = Some(tokio::time::Instant::now() + backoff_delay);
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
            let should_restart = self
                .config
                .autorestart
                .should_restart(exit_code.unwrap_or(0), &self.config.exit_codes);

            if should_restart {
                let desc = match self.config.autorestart {
                    AutoRestartPolicy::Always => "Autorestarting (policy: always)".to_string(),
                    _ => format!("Unexpected exit with code {:?}, autorestarting", exit_code),
                };
                self.update_status(ProgramState::Starting, None, exit_code, desc);
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
                let desc = if is_expected {
                    format!("Exited normally with code {:?}", exit_code)
                } else {
                    format!("Exited with code {:?}", exit_code)
                };
                self.update_status(ProgramState::Exited, None, exit_code, desc);
                false
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
                    group: self.config.group.clone(),
                    old_state,
                    new_state: state,
                    pid,
                    exit_code,
                    tries: self.retry_count,
                    description,
                });
        }
    }
}
