// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::error::ProgramError;
use crate::logging::types::LogChunk;
use async_trait::async_trait;

/// Extensible backend destination interface for log records.
#[async_trait]
pub trait LogBackend: Send + Sync + 'static {
    /// Asynchronously writes a log chunk to the backend destination.
    async fn write_chunk(&self, chunk: &LogChunk) -> Result<(), ProgramError>;

    /// Flushes any pending buffered log records to the underlying storage/network.
    async fn flush(&self) -> Result<(), ProgramError>;

    /// Gracefully closes the backend, ensuring all data is persisted before shutdown.
    async fn close(&self) -> Result<(), ProgramError> {
        self.flush().await
    }
}
