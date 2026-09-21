// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use crate::logging::ring_buffer::RingBuffer;
use crate::logging::rotator::LogRotator;
use crate::manager::{EventHub, LogEntry};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::task::JoinHandle;

/// Builder for constructing asynchronous log pump tasks.
pub struct LogPumpBuilder<R> {
    reader: R,
    ring_buffer: Arc<RingBuffer>,
    stream_name: &'static str,
    rotator: Option<LogRotator>,
    ring_prefix: Option<String>,
    event_hub: Option<EventHub>,
    program_name: Option<String>,
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
            rotator: None,
            ring_prefix: None,
            event_hub: None,
            program_name: None,
        }
    }

    /// Configures the optional rotating file writer.
    pub fn with_rotator(mut self, rotator: Option<LogRotator>) -> Self {
        self.rotator = rotator;
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

    /// Spawns the background pumping task onto the Tokio runtime.
    pub fn spawn(self) -> JoinHandle<()> {
        let LogPumpBuilder {
            reader,
            ring_buffer,
            stream_name,
            rotator,
            ring_prefix,
            event_hub,
            program_name,
        } = self;

        tokio::spawn(async move {
            // Guarantee file flush on EOF, task cancellation, or unexpected loop termination
            let flush_rotator = rotator.clone();
            let prog_name_diag = program_name.clone();
            scopeguard::defer! {
                if let Some(ref rot) = flush_rotator
                    && let Err(e) = rot.flush()
                {
                    tracing::warn!(
                        program = ?prog_name_diag,
                        stream = stream_name,
                        error = %e,
                        "Failed to flush log rotator on shutdown"
                    );
                }
            }

            let mut lines = BufReader::new(reader).lines();

            while let Ok(Some(line)) = lines.next_line().await {
                // Push to in-memory RingBuffer (for tail -f and web streaming)
                if let Some(ref prefix) = ring_prefix {
                    ring_buffer.push(format!("{}: {}", prefix, line));
                } else {
                    ring_buffer.push(&line);
                }

                // Broadcast to the central EventHub LogBus if configured
                if let Some(ref hub) = event_hub
                    && let Some(ref prog) = program_name
                {
                    hub.publish_log(LogEntry::new(prog, stream_name, &line));
                }

                // Write raw line to file rotator if configured
                if let Some(ref rot) = rotator
                    && let Err(e) = rot.write_line(&line)
                {
                    tracing::warn!(
                        program = ?program_name,
                        stream = stream_name,
                        error = %e,
                        "Failed to write log line to rotator"
                    );
                }
            }
        })
    }
}

/// Spawns an asynchronous background task to pump lines from an async reader (stdout/stderr)
/// into a RingBuffer, an optional LogRotator file writer, and the central EventHub.
pub fn spawn_log_pump<R>(
    reader: R,
    ring_buffer: Arc<RingBuffer>,
    rotator: Option<LogRotator>,
    ring_prefix: Option<String>,
    event_hub: Option<EventHub>,
    program_name: Option<String>,
    stream_name: &'static str,
) -> JoinHandle<()>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    LogPumpBuilder::new(reader, ring_buffer, stream_name)
        .with_rotator(rotator)
        .with_ring_prefix(ring_prefix)
        .with_event_hub(event_hub)
        .with_program_name(program_name)
        .spawn()
}
