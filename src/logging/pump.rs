// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use crate::logging::ring_buffer::RingBuffer;
use crate::logging::rotator::LogRotator;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::task::JoinHandle;

/// Spawns an asynchronous background task to pump lines from an async reader (stdout/stderr)
/// into a RingBuffer and an optional LogRotator file writer.
pub fn spawn_log_pump<R>(
    reader: R,
    ring_buffer: Arc<RingBuffer>,
    rotator: Option<LogRotator>,
    ring_prefix: Option<String>,
) -> JoinHandle<()>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();

        while let Ok(Some(line)) = lines.next_line().await {
            // Push to in-memory RingBuffer (for tail -f and web streaming)
            if let Some(ref prefix) = ring_prefix {
                ring_buffer.push(format!("{}: {}", prefix, line));
            } else {
                ring_buffer.push(&line);
            }

            // Write raw line to file rotator if configured
            if let Some(ref rot) = rotator {
                let _ = rot.write_line(&line);
            }
        }

        // Final flush on EOF
        if let Some(ref rot) = rotator {
            let _ = rot.flush();
        }
    })
}
