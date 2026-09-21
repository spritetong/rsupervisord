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
#[async_trait]
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

    /// Asynchronously waits for child process exit using the platform's optimal mechanism
    /// (e.g. event-driven kernel wait, or encapsulated fallback polling if OS primitives are unavailable).
    async fn wait_exit(
        &mut self,
        child: &mut tokio::process::Child,
    ) -> io::Result<std::process::ExitStatus>;
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

    /// Returns the platform default local IPC path (named pipe or UDS) for a given command name.
    fn default_local_ipc_path(&self, cmd_name: &str, config_dir: Option<&Path>) -> PathBuf;

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

    /// Returns the platform default daemon log path.
    fn default_daemon_log_path(&self, cmd_name: &str, config_dir: Option<&Path>) -> PathBuf;

    /// Returns the platform default program log path.
    fn default_program_log_path(
        &self,
        cmd_name: &str,
        program_name: &str,
        config_dir: Option<&Path>,
    ) -> PathBuf;

    /// Returns the platform default system configuration directory if applicable (e.g. /etc/<cmd_name> on Unix).
    fn default_system_config_dir(&self, cmd_name: &str) -> Option<PathBuf>;

    /// Returns the host node name (hostname).
    fn hostname(&self) -> String {
        "localhost".to_string()
    }

    /// Resolves an executable binary path against the working directory and system PATH.
    fn resolve_executable(&self, command: &str, working_dir: Option<&Path>) -> Option<PathBuf>;

    /// Returns the platform system service manager.
    fn service(&self) -> &dyn PlatformService;
}

/// Trait representing platform-specific system service management and execution.
pub trait PlatformService: Send + Sync {
    /// Installs the binary as an auto-start system service.
    fn install(&self, cmd_name: &str, config_path: Option<&Path>) -> anyhow::Result<()>;

    /// Uninstalls the system service.
    fn uninstall(&self, cmd_name: &str) -> anyhow::Result<()>;

    /// Starts the installed system service.
    fn start(&self, cmd_name: &str) -> anyhow::Result<()>;

    /// Stops the running system service.
    fn stop(&self, cmd_name: &str) -> anyhow::Result<()>;

    /// Restarts the system service.
    fn restart(&self, cmd_name: &str) -> anyhow::Result<()>;

    /// Runs the process as a system service (e.g. Windows SCM dispatcher).
    fn run_service(
        &self,
        daemon_args: crate::daemon::DaemonArgs,
        config_path: PathBuf,
        cmd_name: String,
    ) -> anyhow::Result<()>;
}
