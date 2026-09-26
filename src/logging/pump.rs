// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::logging::backend::LogBackend;
use crate::logging::ring_buffer::RingBuffer;
use crate::logging::types::{LogChannel, LogChunk};
use crate::manager::{EventHub, LogEntry};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};
use tokio::task::JoinHandle;

/// Builder for constructing asynchronous log pump tasks.
pub struct LogPumpBuilder<R> {
    reader: R,
    ring_buffer: Arc<RingBuffer>,
    stream_name: &'static str,
    backend: Option<Arc<dyn LogBackend>>,
    ring_prefix: Option<String>,
    event_hub: Option<EventHub>,
    program_name: Option<String>,
    group_name: Option<String>,
    pid: Option<u32>,
    events_enabled: bool,
}

impl<R> LogPumpBuilder<R>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    /// Creates a new builder for pumping log lines from `reader` into `ring_buffer`.
    pub fn new(reader: R, ring_buffer: Arc<RingBuffer>, stream_name: &'static str) -> Self {
        Self {
            reader,
            ring_buffer,
            stream_name,
            backend: None,
            ring_prefix: None,
            event_hub: None,
            program_name: None,
            group_name: None,
            pid: None,
            events_enabled: false,
        }
    }

    /// Configures the extensible log backend (File, Syslog, Stdio, Composite).
    pub fn with_backend(mut self, backend: Option<Arc<dyn LogBackend>>) -> Self {
        self.backend = backend;
        self
    }

    /// Configures the prefix for ring buffer entries (e.g. "worker-1").
    pub fn with_ring_prefix(mut self, prefix: Option<String>) -> Self {
        self.ring_prefix = prefix;
        self
    }

    /// Configures the central event hub for streaming log distribution.
    pub fn with_event_hub(mut self, hub: Option<EventHub>) -> Self {
        self.event_hub = hub;
        self
    }

    /// Configures the originating program identifier.
    pub fn with_program_name(mut self, name: Option<String>) -> Self {
        self.program_name = name;
        self
    }

    /// Configures the originating group identifier.
    pub fn with_group_name(mut self, group: Option<String>) -> Self {
        self.group_name = group;
        self
    }

    /// Configures the child process PID.
    pub fn with_pid(mut self, pid: Option<u32>) -> Self {
        self.pid = pid;
        self
    }

    /// Configures whether process log events are enabled for event listeners.
    pub fn with_events_enabled(mut self, enabled: bool) -> Self {
        self.events_enabled = enabled;
        self
    }

    /// Spawns the background pumping task onto the Tokio runtime.
    pub fn spawn(self) -> JoinHandle<()> {
        let LogPumpBuilder {
            reader,
            ring_buffer,
            stream_name,
            backend,
            ring_prefix,
            event_hub,
            program_name,
            group_name,
            pid,
            events_enabled,
        } = self;

        let channel = if stream_name.eq_ignore_ascii_case("stderr") {
            LogChannel::Stderr
        } else {
            LogChannel::Stdout
        };

        tokio::spawn(async move {
            // Guarantee file flush on EOF, task cancellation, or unexpected loop termination
            let flush_backend = backend.clone();
            let prog_name_diag = program_name.clone();
            scopeguard::defer! {
                if let Some(ref b) = flush_backend {
                    let b_clone = b.clone();
                    let diag = prog_name_diag.clone();
                    tokio::spawn(async move {
                        if let Err(e) = b_clone.flush().await {
                            tracing::warn!(
                                program = ?diag,
                                stream = stream_name,
                                error = %e,
                                "Failed to flush log backend on shutdown"
                            );
                        }
                    });
                }
            }

            let mut reader = BufReader::new(reader);
            let mut buf = Vec::new();

            loop {
                buf.clear();
                match (&mut reader)
                    .take(crate::consts::MAX_PUMP_CHUNK_SIZE as u64)
                    .read_until(b'\n', &mut buf)
                    .await
                {
                    Ok(0) => break, // EOF
                    Ok(_) => {}
                    Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(ref e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::BrokenPipe
                                | std::io::ErrorKind::UnexpectedEof
                                | std::io::ErrorKind::ConnectionReset
                                | std::io::ErrorKind::ConnectionAborted
                                | std::io::ErrorKind::NotConnected
                        ) =>
                    {
                        break;
                    }
                    Err(e) => {
                        tracing::warn!(
                            program = ?program_name,
                            stream = stream_name,
                            error = %e,
                            "Log read error; terminating pump"
                        );
                        break;
                    }
                }

                // Strip trailing '\n' and optional '\r'
                if buf.ends_with(b"\n") {
                    buf.pop();
                    if buf.ends_with(b"\r") {
                        buf.pop();
                    }
                }

                let line = String::from_utf8_lossy(&buf);
                // Push to in-memory RingBuffer (for tail -f and web streaming)
                if let Some(ref prefix) = ring_prefix {
                    ring_buffer.push(format!("{}: {}", prefix, line));
                } else {
                    ring_buffer.push(&*line);
                }

                // Broadcast to the central EventHub LogBus if configured
                if let Some(ref hub) = event_hub
                    && let Some(ref prog) = program_name
                {
                    hub.publish_log(LogEntry::with_details(
                        prog,
                        group_name.clone(),
                        pid,
                        stream_name,
                        &*line,
                        events_enabled,
                    ));
                }

                // Write chunk to generic log backend if configured
                if let Some(ref b) = backend {
                    let chunk = LogChunk::new(
                        channel,
                        program_name.clone().unwrap_or_default(),
                        pid,
                        format!("{}\n", line),
                    );
                    if let Err(e) = b.write_chunk(&chunk).await {
                        tracing::warn!(
                            program = ?program_name,
                            stream = stream_name,
                            error = %e,
                            "Failed to write log chunk to backend"
                        );
                    }
                }
            }

            if let Some(ref b) = backend
                && let Err(e) = b.flush().await
            {
                tracing::warn!(
                    program = ?program_name,
                    stream = stream_name,
                    error = %e,
                    "Failed to flush log backend on pump completion"
                );
            }
        })
    }
}
