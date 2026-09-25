// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::error::ProgramError;
use crate::logging::backend::LogBackend;
use crate::logging::types::LogChunk;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

/// Composite log backend fanning out log writes across multiple inner backends.
///
/// Implements full Go parity for comma-separated multi-destination configurations
/// (e.g. `stdout_logfile = test.log, /dev/stdout`), as well as Python `stdout_syslog`
/// fan-out.
#[derive(Clone, Default)]
pub struct CompositeLogBackend {
    backends: Vec<Arc<dyn LogBackend>>,
}

impl CompositeLogBackend {
    /// Creates a new composite backend from a list of log backends.
    pub fn new(backends: Vec<Arc<dyn LogBackend>>) -> Self {
        Self { backends }
    }

    /// Adds a backend to the composite collection.
    pub fn push(&mut self, backend: Arc<dyn LogBackend>) {
        self.backends.push(backend);
    }

    /// Returns a slice of the child backends.
    pub fn backends(&self) -> &[Arc<dyn LogBackend>] {
        &self.backends
    }

    /// Returns the primary (first) backend, if any.
    pub fn primary(&self) -> Option<&Arc<dyn LogBackend>> {
        self.backends.first()
    }
}

#[async_trait]
impl LogBackend for CompositeLogBackend {
    async fn write_chunk(&self, chunk: &LogChunk) -> Result<(), ProgramError> {
        let mut first_err = None;
        for backend in &self.backends {
            if let Err(e) = backend.write_chunk(chunk).await {
                first_err.get_or_insert(e);
            }
        }
        if let Some(err) = first_err {
            Err(err)
        } else {
            Ok(())
        }
    }

    async fn flush(&self) -> Result<(), ProgramError> {
        for backend in &self.backends {
            let _ = backend.flush().await;
        }
        Ok(())
    }

    async fn close(&self) -> Result<(), ProgramError> {
        for backend in &self.backends {
            let _ = backend.close().await;
        }
        Ok(())
    }

    fn clear(&self) -> Result<(), ProgramError> {
        let mut first_err = None;
        for backend in &self.backends {
            if let Err(e) = backend.clear() {
                first_err.get_or_insert(e);
            }
        }
        if let Some(err) = first_err {
            Err(err)
        } else {
            Ok(())
        }
    }
}

/// Discards all output completely (used for NONE, off, /dev/null).
#[derive(Debug, Clone, Copy, Default)]
pub struct NullLogBackend;

#[async_trait]
impl LogBackend for NullLogBackend {
    async fn write_chunk(&self, _chunk: &LogChunk) -> Result<(), ProgramError> {
        Ok(())
    }

    async fn flush(&self) -> Result<(), ProgramError> {
        Ok(())
    }
}

/// Standard I/O stream sink directing child output directly to the parent daemon process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdIoStream {
    Stdout,
    Stderr,
}

/// Directs child log output to the daemon process's stdout or stderr (/dev/stdout, /dev/stderr).
pub struct StdIoLogBackend {
    stream: StdIoStream,
}

impl StdIoLogBackend {
    pub fn stdout() -> Self {
        Self {
            stream: StdIoStream::Stdout,
        }
    }

    pub fn stderr() -> Self {
        Self {
            stream: StdIoStream::Stderr,
        }
    }
}

#[async_trait]
impl LogBackend for StdIoLogBackend {
    async fn write_chunk(&self, chunk: &LogChunk) -> Result<(), ProgramError> {
        match self.stream {
            StdIoStream::Stdout => {
                let mut out = tokio::io::stdout();
                out.write_all(&chunk.data).await.map_err(|e| {
                    ProgramError::PlatformError(format!("Failed to write to daemon stdout: {}", e))
                })?;
            }
            StdIoStream::Stderr => {
                let mut err = tokio::io::stderr();
                err.write_all(&chunk.data).await.map_err(|e| {
                    ProgramError::PlatformError(format!("Failed to write to daemon stderr: {}", e))
                })?;
            }
        }
        Ok(())
    }

    async fn flush(&self) -> Result<(), ProgramError> {
        match self.stream {
            StdIoStream::Stdout => {
                let mut out = tokio::io::stdout();
                let _ = out.flush().await;
            }
            StdIoStream::Stderr => {
                let mut err = tokio::io::stderr();
                let _ = err.flush().await;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logging::types::LogChannel;
    use parking_lot::Mutex;

    struct MockBackend {
        chunks: Mutex<Vec<Vec<u8>>>,
    }

    impl MockBackend {
        fn new() -> Self {
            Self {
                chunks: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl LogBackend for MockBackend {
        async fn write_chunk(&self, chunk: &LogChunk) -> Result<(), ProgramError> {
            self.chunks.lock().push(chunk.data.to_vec());
            Ok(())
        }
        async fn flush(&self) -> Result<(), ProgramError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_composite_fan_out() {
        let b1 = Arc::new(MockBackend::new());
        let b2 = Arc::new(MockBackend::new());
        let composite = CompositeLogBackend::new(vec![b1.clone(), b2.clone()]);

        let chunk = LogChunk::new(LogChannel::Stdout, "test", None, "multi-dest-line\n");
        composite.write_chunk(&chunk).await.unwrap();

        assert_eq!(b1.chunks.lock().len(), 1);
        assert_eq!(b2.chunks.lock().len(), 1);
        assert_eq!(b1.chunks.lock()[0], b"multi-dest-line\n");
        assert_eq!(b2.chunks.lock()[0], b"multi-dest-line\n");
    }
}
