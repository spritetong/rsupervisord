// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

pub mod backend;
pub mod composite;
pub mod destination;
pub mod in_memory_rotator;
pub mod pump;
pub mod reader;
pub mod ring_buffer;
pub mod rotator;
pub mod syslog;
pub mod transport;
pub mod types;

pub use backend::LogBackend;
pub use composite::{CompositeLogBackend, NullLogBackend, StdIoLogBackend, StdIoStream};
pub use destination::{
    BackendBuildOptions, LogDestination, SyslogFacility, SyslogProto, SyslogSeverity, SyslogTarget,
};
pub use in_memory_rotator::{InMemoryChannelRotator, InMemoryLogRotator};
pub use pump::LogPumpBuilder;
pub use reader::InstantLogReader;
pub use ring_buffer::RingBuffer;
pub use rotator::LogRotator;
pub use syslog::SyslogLogBackend;
pub use transport::{LogTransport, ProcessStdioHandles, TransportStreams};
pub use types::{LogChannel, LogChunk};
