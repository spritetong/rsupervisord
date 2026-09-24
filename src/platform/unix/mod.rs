// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

pub mod service;
pub mod transport;
pub use service::UnixService;

use crate::error::ProgramError;
use crate::platform::traits::{
    AsyncStream, PlatformBackend, PlatformIpcListener, PlatformProcessGuard, PlatformService,
    ProcessTransportConfig, abs_path,
};
use crate::program::config::StopSignal;
use async_trait::async_trait;
use nix::sys::signal::{self, Signal};
use nix::unistd::{Gid, Pid, Uid};
use std::io;
use std::path::{Path, PathBuf};
use tokio::process::Command as TokioCommand;

/// Guard managing a Unix process group.
pub struct UnixProcessGuard {
    pub pid: u32,
    pub pgid: i32,
    stop_as_group: bool,
    kill_as_group: bool,
    last_cpu_sample: std::sync::Mutex<Option<(std::time::Instant, u64)>>,
}

impl UnixProcessGuard {
    pub fn new(pid: u32, stop_as_group: bool, kill_as_group: bool) -> Self {
        Self {
            pid,
            pgid: pid as i32,
            stop_as_group,
            kill_as_group,
            last_cpu_sample: std::sync::Mutex::new(None),
        }
    }

    /// Sends a POSIX signal to the entire process group.
    fn send_signal_to_group(&self, sig: Signal) -> Result<(), ProgramError> {
        let group_pid = Pid::from_raw(-self.pgid);
        match signal::kill(group_pid, sig) {
            Ok(_) => Ok(()),
            Err(nix::errno::Errno::ESRCH) => {
                // The process group might have already exited; fall back to the individual PID
                let single_pid = Pid::from_raw(self.pid as i32);
                let _ = signal::kill(single_pid, sig);
                Ok(())
            }
            Err(e) => Err(ProgramError::PlatformError(format!(
                "Failed to send signal {:?} to pgid {}: {}",
                sig, self.pgid, e
            ))),
        }
    }

    /// Sends a POSIX signal to the primary PID only (Python default when
    /// `stopasgroup`/`killasgroup` are false).
    fn send_signal_to_pid(&self, sig: Signal) -> Result<(), ProgramError> {
        let single_pid = Pid::from_raw(self.pid as i32);
        match signal::kill(single_pid, sig) {
            Ok(_) | Err(nix::errno::Errno::ESRCH) => Ok(()),
            Err(e) => Err(ProgramError::PlatformError(format!(
                "Failed to send signal {:?} to pid {}: {}",
                sig, self.pid, e
            ))),
        }
    }

    /// Fallback polling abstraction for restricted or legacy Unix environments where
    /// async signals or pidfds are unavailable.
    pub async fn poll_exit_fallback(
        &self,
        child: &mut tokio::process::Child,
        poll_interval: std::time::Duration,
    ) -> io::Result<std::process::ExitStatus> {
        loop {
            if let Some(status) = child.try_wait()? {
                return Ok(status);
            }
            tokio::time::sleep(poll_interval).await;
        }
    }
}

#[async_trait]
impl PlatformProcessGuard for UnixProcessGuard {
    async fn wait_exit(
        &mut self,
        child: &mut tokio::process::Child,
    ) -> io::Result<std::process::ExitStatus> {
        // Optimal Unix OS mechanism:
        // Tokio's process driver uses pidfd (Linux >= 5.3) or the SIGCHLD signal handler
        // to wake up asynchronously without continuous user-space polling.
        child.wait().await
    }

    fn send_stop_signal(&self, signal: StopSignal) -> Result<(), ProgramError> {
        let nix_sig = to_nix_signal(signal);
        if self.stop_as_group {
            self.send_signal_to_group(nix_sig)
        } else {
            self.send_signal_to_pid(nix_sig)
        }
    }

    fn force_kill(&self) -> Result<(), ProgramError> {
        if self.kill_as_group {
            self.send_signal_to_group(Signal::SIGKILL)
        } else {
            self.send_signal_to_pid(Signal::SIGKILL)
        }
    }

    fn pid(&self) -> u32 {
        self.pid
    }

    fn query_metrics(&self) -> Result<crate::platform::traits::ProcessMetrics, ProgramError> {
        let mut memory_rss_bytes = 0u64;
        let mut cpu_percent = 0.0f32;

        // 1. Read /proc/{pid}/status for VmRSS
        let status_path = format!("/proc/{}/status", self.pid);
        if let Ok(content) = std::fs::read_to_string(&status_path) {
            for line in content.lines() {
                if let Some(rest) = line.strip_prefix("VmRSS:") {
                    let parts: Vec<&str> = rest.split_whitespace().collect();
                    if let Some(kb_str) = parts.first()
                        && let Ok(kb) = kb_str.parse::<u64>()
                    {
                        memory_rss_bytes = kb * 1024;
                    }
                    break;
                }
            }
        }

        // 2. Read /proc/{pid}/stat for CPU times (fields 14 utime and 15 stime)
        let stat_path = format!("/proc/{}/stat", self.pid);
        if let Ok(content) = std::fs::read_to_string(&stat_path)
            && let Some(last_paren_idx) = content.rfind(')')
        {
            let rest = &content[last_paren_idx + 1..];
            let fields: Vec<&str> = rest.split_whitespace().collect();
            if fields.len() > 12 {
                let utime = fields[11].parse::<u64>().unwrap_or(0);
                let stime = fields[12].parse::<u64>().unwrap_or(0);
                let total_ticks = utime + stime;
                let now = std::time::Instant::now();

                if let Ok(mut lock) = self.last_cpu_sample.lock() {
                    if let Some((prev_instant, prev_ticks)) = *lock {
                        let delta_ticks = total_ticks.saturating_sub(prev_ticks);
                        let elapsed_secs = now.duration_since(prev_instant).as_secs_f64();
                        if elapsed_secs > 0.0 {
                            let clk_tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
                            let ticks_per_sec = if clk_tck > 0 { clk_tck as f64 } else { 100.0 };
                            let cpus = std::thread::available_parallelism()
                                .map(|n| n.get())
                                .unwrap_or(1) as f64;
                            let pct =
                                (delta_ticks as f64 / ticks_per_sec / elapsed_secs) * 100.0 / cpus;
                            cpu_percent = (pct as f32).max(0.0);
                        }
                    }
                    *lock = Some((now, total_ticks));
                }
            }
        }

        Ok(crate::platform::traits::ProcessMetrics {
            memory_rss_bytes,
            cpu_percent,
        })
    }
}

/// Native Unix platform backend implementation.
pub struct UnixPlatformBackend;

#[async_trait]
impl PlatformBackend for UnixPlatformBackend {
    async fn create_process_log_transport(
        &self,
        config: &ProcessTransportConfig,
    ) -> Result<Box<dyn crate::logging::LogTransport>, ProgramError> {
        let transport = transport::UnixProcessLogTransport::create(config).await?;
        Ok(Box::new(transport))
    }

    fn configure_command(
        &self,
        cmd: &mut TokioCommand,
        user: Option<&str>,
        umask: Option<u32>,
    ) -> Result<(), ProgramError> {
        let parsed_ids = if let Some(user_spec) = user {
            Some(parse_user_spec(user_spec)?)
        } else {
            None
        };

        unsafe {
            cmd.pre_exec(move || {
                // 1. Establish independent process group
                nix::unistd::setpgid(Pid::from_raw(0), Pid::from_raw(0))
                    .map_err(std::io::Error::other)?;

                // 2. Apply umask if configured
                if let Some(mask) = umask {
                    nix::sys::stat::umask(nix::sys::stat::Mode::from_bits_truncate(mask));
                }

                // 3. Drop privileges if user specified
                if let Some((uid, gid)) = parsed_ids {
                    if let Some(g) = gid {
                        let _ = nix::unistd::setgroups(&[g]);
                        nix::unistd::setgid(g).map_err(std::io::Error::other)?;
                    }
                    if let Some(u) = uid {
                        nix::unistd::setuid(u).map_err(std::io::Error::other)?;
                    }
                }

                Ok(())
            });
        }

        Ok(())
    }

    fn attach_child(
        &self,
        _child: &tokio::process::Child,
        pid: u32,
        stop_as_group: bool,
        kill_as_group: bool,
    ) -> Result<Box<dyn PlatformProcessGuard>, ProgramError> {
        Ok(Box::new(UnixProcessGuard::new(
            pid,
            stop_as_group,
            kill_as_group,
        )))
    }

    fn default_uds_path(&self) -> PathBuf {
        let cmd_name = crate::config::paths::get_cmd_name();
        self.default_local_ipc_path(&cmd_name, None)
    }

    fn default_local_ipc_path(&self, cmd_name: &str, _config_dir: Option<&Path>) -> PathBuf {
        PathBuf::from(format!("/var/run/{}.sock", cmd_name))
    }

    fn is_elevated(&self) -> bool {
        nix::unistd::getuid().is_root()
    }

    fn default_stop_signal(&self) -> StopSignal {
        StopSignal::Term
    }

    fn build_shell_command(&self, command: &str) -> TokioCommand {
        let mut cmd = TokioCommand::new("sh");
        cmd.args(["-c", command]);
        cmd
    }

    async fn connect_ipc(&self, path: &Path) -> io::Result<Box<dyn AsyncStream>> {
        let stream = tokio::net::UnixStream::connect(path).await?;
        Ok(Box::new(stream))
    }

    async fn connect_named_pipe(&self, _path: &Path) -> io::Result<Box<dyn AsyncStream>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Named pipes are not supported on Unix systems",
        ))
    }

    fn bind_ipc_listener(
        &self,
        path: &Path,
        allow_unelevated: bool,
        mode: u32,
    ) -> io::Result<Box<dyn PlatformIpcListener>> {
        let listener = UnixIpcListener::bind(path, allow_unelevated, mode)?;
        Ok(Box::new(listener))
    }

    fn default_daemon_log_path(&self, cmd_name: &str, _config_dir: Option<&Path>) -> PathBuf {
        PathBuf::from(format!("/var/log/{}/{}.log", cmd_name, cmd_name))
    }

    fn default_program_log_path(
        &self,
        cmd_name: &str,
        program_name: &str,
        _config_dir: Option<&Path>,
    ) -> PathBuf {
        let sanitized = program_name.replace(':', "_");
        PathBuf::from(format!("/var/log/{}/{}.log", cmd_name, sanitized))
    }

    fn default_system_config_dir(&self, cmd_name: &str) -> Option<PathBuf> {
        Some(PathBuf::from(format!("/etc/{}", cmd_name)))
    }

    fn hostname(&self) -> String {
        if let Ok(h) = nix::unistd::gethostname()
            && let Ok(s) = h.into_string()
        {
            return s;
        }
        std::env::var("HOSTNAME").unwrap_or_else(|_| "localhost".to_string())
    }

    fn resolve_executable(&self, command: &str, working_dir: Option<&Path>) -> Option<PathBuf> {
        let base_path = Path::new(command);

        if base_path.is_absolute() {
            if base_path.is_file() {
                return Some(abs_path(base_path));
            }
            return None;
        }

        if command.contains('/') {
            let wd = working_dir.unwrap_or_else(|| Path::new("."));
            let candidate = wd.join(base_path);
            if candidate.is_file() {
                return Some(abs_path(&candidate));
            }
            return None;
        }

        if let Some(wd) = working_dir {
            let candidate = wd.join(base_path);
            if candidate.is_file() {
                return Some(abs_path(&candidate));
            }
        }

        if let Some(paths) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&paths) {
                let candidate = dir.join(base_path);
                if candidate.is_file() {
                    return Some(abs_path(&candidate));
                }
            }
        }

        None
    }

    fn split_command_line(&self, cmd: &str) -> Result<Vec<String>, String> {
        shell_words::split(cmd).map_err(|e| e.to_string())
    }

    fn build_command(&self, program: &Path, args: &[String]) -> TokioCommand {
        let mut cmd = TokioCommand::new(program);
        cmd.args(args);
        cmd
    }

    fn service(&self) -> &dyn PlatformService {
        &UnixService
    }
}

fn cleanup_unix_ipc(p: PathBuf) {
    let _ = std::fs::remove_file(&p);
}

/// Unix domain socket IPC listener.
pub struct UnixIpcListener {
    listener: tokio::net::UnixListener,
    _cleanup: scopeguard::ScopeGuard<PathBuf, fn(PathBuf)>,
    allow_unelevated: bool,
}

impl UnixIpcListener {
    pub fn bind(path: &Path, allow_unelevated: bool, mode: u32) -> io::Result<Self> {
        use std::os::unix::fs::PermissionsExt;

        if let Some(parent) = path.parent()
            && !parent.exists()
        {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::remove_file(path);

        // Restrict umask during bind so the socket is never group/world-accessible
        // before set_permissions applies the configured mode (parity with Windows
        // pipe first-instance SECURITY_ATTRIBUTES; closes the bind→chmod race).
        let previous_umask = nix::sys::stat::umask(nix::sys::stat::Mode::from_bits_truncate(
            crate::consts::BIND_UMASK,
        ));
        let bind_result = tokio::net::UnixListener::bind(path);
        nix::sys::stat::umask(previous_umask);
        let listener = bind_result?;

        // Apply configured mode after bind, before any accept: authorization layer.
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
        let cleanup = scopeguard::guard(path.to_path_buf(), cleanup_unix_ipc as fn(PathBuf));
        Ok(Self {
            listener,
            _cleanup: cleanup,
            allow_unelevated,
        })
    }
}

/// Returns the peer UID for a connected Unix stream.
///
/// Linux/Android: `SO_PEERCRED`. BSD/macOS: `getpeereid(3)`.
/// Any failure is an authorization failure (fail closed).
fn peer_uid(stream: &tokio::net::UnixStream) -> Result<Uid, ProgramError> {
    use std::os::fd::{AsRawFd, BorrowedFd};

    let borrowed_fd = unsafe { BorrowedFd::borrow_raw(stream.as_raw_fd()) };

    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use nix::sys::socket::{getsockopt, sockopt::PeerCredentials};
        let creds = getsockopt(&borrowed_fd, PeerCredentials).map_err(|e| {
            ProgramError::PlatformError(format!("Failed to retrieve peer credentials: {}", e))
        })?;
        Ok(Uid::from_raw(creds.uid()))
    }

    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let (uid, _gid) = nix::unistd::getpeereid(borrowed_fd).map_err(|e| {
            ProgramError::PlatformError(format!("Failed to retrieve peer credentials: {}", e))
        })?;
        Ok(uid)
    }
}

/// Verifies caller peer credentials on Unix domain sockets.
pub fn verify_caller_credentials(
    stream: &tokio::net::UnixStream,
    allow_unelevated: bool,
) -> Result<(), ProgramError> {
    if allow_unelevated {
        return Ok(());
    }

    let caller_uid = peer_uid(stream)?;
    let daemon_uid = nix::unistd::getuid();

    if daemon_uid.is_root() {
        if !caller_uid.is_root() {
            return Err(ProgramError::PlatformError(format!(
                "Access denied: Caller UID {} is not root",
                caller_uid
            )));
        }
    } else if caller_uid != daemon_uid && !caller_uid.is_root() {
        return Err(ProgramError::PlatformError(format!(
            "Access denied: Caller UID {} does not match daemon UID {}",
            caller_uid, daemon_uid
        )));
    }

    Ok(())
}

#[async_trait]
impl PlatformIpcListener for UnixIpcListener {
    async fn accept(&mut self) -> io::Result<Box<dyn AsyncStream>> {
        loop {
            let (stream, _) = self.listener.accept().await?;
            if let Err(e) = verify_caller_credentials(&stream, self.allow_unelevated) {
                tracing::warn!("Rejecting unauthorized UDS connection: {}", e);
                continue;
            }
            return Ok(Box::new(stream));
        }
    }
}

/// Converts generic StopSignal to POSIX Signal.
pub fn to_nix_signal(sig: StopSignal) -> Signal {
    match sig {
        StopSignal::Term => Signal::SIGTERM,
        StopSignal::Int => Signal::SIGINT,
        StopSignal::Quit => Signal::SIGQUIT,
        StopSignal::Kill => Signal::SIGKILL,
        StopSignal::Hup => Signal::SIGHUP,
        StopSignal::CtrlBreak => Signal::SIGTERM,
        StopSignal::CtrlC => Signal::SIGINT,
    }
}

/// Parses a user specification string supporting: "1000", "1000:1000", "username", "username:groupname".
/// When groupname is omitted, resolves the user's primary GID.
fn parse_user_spec(spec: &str) -> Result<(Option<Uid>, Option<Gid>), ProgramError> {
    let parts: Vec<&str> = spec.split(':').collect();
    let uid_part = parts[0].trim();
    let gid_part = if parts.len() > 1 {
        Some(parts[1].trim())
    } else {
        None
    };

    let (uid, default_gid) = if let Ok(num) = uid_part.parse::<u32>() {
        let u = Uid::from_raw(num);
        let primary_gid = nix::unistd::User::from_uid(u)
            .ok()
            .flatten()
            .map(|usr| usr.gid);
        (Some(u), primary_gid)
    } else if let Ok(Some(u)) = nix::unistd::User::from_name(uid_part) {
        (Some(u.uid), Some(u.gid))
    } else {
        return Err(ProgramError::ConfigError(format!(
            "Unknown user or invalid UID: '{}'",
            uid_part
        )));
    };

    let gid = if let Some(g_str) = gid_part {
        if let Ok(num) = g_str.parse::<u32>() {
            Some(Gid::from_raw(num))
        } else if let Ok(Some(g)) = nix::unistd::Group::from_name(g_str) {
            Some(g.gid)
        } else {
            return Err(ProgramError::ConfigError(format!(
                "Unknown group or invalid GID: '{}'",
                g_str
            )));
        }
    } else {
        default_gid
    };

    Ok((uid, gid))
}
