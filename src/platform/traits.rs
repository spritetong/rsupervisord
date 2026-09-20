// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use crate::error::ProgramError;
use crate::program::config::StopSignal;
use async_trait::async_trait;
use std::io;
use std::path::{Path, PathBuf};

/// Unified stream trait combining AsyncRead and AsyncWrite.
pub trait AsyncStream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + ?Sized> AsyncStream for T {}

/// Trait representing an OS-level IPC listener (Unix Domain Socket or Windows Named Pipe/UDS).
#[async_trait]
pub trait PlatformIpcListener: Send + Sync {
    /// Accepts an incoming connection stream.
    async fn accept(&mut self) -> io::Result<Box<dyn AsyncStream>>;
}

/// Process resource utilization metrics.
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct ProcessMetrics {
    pub memory_rss_bytes: u64,
    pub cpu_percent: f32,
}

/// Trait representing an OS-level process guard capable of managing,
/// signaling, and cleanly terminating a process tree.
pub trait PlatformProcessGuard: Send + Sync {
    /// Sends a graceful stop signal to the process and its descendants.
    fn send_stop_signal(&self, signal: StopSignal) -> Result<(), ProgramError>;

    /// Forcefully terminates the process and all its descendants.
    fn force_kill(&self) -> Result<(), ProgramError>;

    /// Returns the primary process ID.
    fn pid(&self) -> u32;

    /// Queries real-time resource utilization metrics for the process (and its tree).
    fn query_metrics(&self) -> Result<ProcessMetrics, ProgramError> {
        Ok(ProcessMetrics::default())
    }
}

/// Trait providing platform-specific abstractions for process configuration,
/// child tracking, IPC communication, and platform defaults.
#[async_trait]
pub trait PlatformBackend: Send + Sync {
    /// Configures the command builder before spawning (e.g. process groups, user privileges, umask).
    fn configure_command(
        &self,
        cmd: &mut tokio::process::Command,
        user: Option<&str>,
        umask: Option<u32>,
    ) -> Result<(), ProgramError>;

    /// Attaches to a freshly spawned child process and returns an OS-level guard.
    fn attach_child(
        &self,
        child: &tokio::process::Child,
        pid: u32,
    ) -> Result<Box<dyn PlatformProcessGuard>, ProgramError>;

    /// Returns the platform default path for Unix Domain Sockets (UDS).
    fn default_uds_path(&self) -> PathBuf;

    /// Checks if the current process runs with elevated (administrator / root) privileges.
    fn is_elevated(&self) -> bool;

    /// Validates caller privileges before accepting requests.
    fn validate_caller_privileges(&self, allow_unelevated: bool) -> Result<(), ProgramError>;

    /// Returns the platform-appropriate default stop signal (e.g. SIGTERM on Unix, CTRL_BREAK on Windows).
    fn default_stop_signal(&self) -> StopSignal;

    /// Constructs a platform-specific shell execution command (e.g. `sh -c` on Unix, `cmd /C` on Windows).
    fn build_shell_command(&self, command: &str) -> tokio::process::Command;

    /// Connects to a local IPC socket (Unix Domain Socket / AF_UNIX) at the given path.
    async fn connect_ipc(&self, path: &Path) -> io::Result<Box<dyn AsyncStream>>;

    /// Connects to a Windows named pipe at the given path (unsupported on Unix).
    async fn connect_named_pipe(&self, path: &Path) -> io::Result<Box<dyn AsyncStream>>;

    /// Binds an OS-level IPC listener at the given path.
    fn bind_ipc_listener(&self, path: &Path) -> io::Result<Box<dyn PlatformIpcListener>>;
}
