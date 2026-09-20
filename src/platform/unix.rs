// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use crate::error::ProgramError;
use crate::platform::traits::{
    AsyncStream, PlatformBackend, PlatformIpcListener, PlatformProcessGuard,
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
    last_cpu_sample: std::sync::Mutex<Option<(std::time::Instant, u64)>>,
}

impl UnixProcessGuard {
    pub fn new(pid: u32) -> Self {
        Self {
            pid,
            pgid: pid as i32,
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
        self.send_signal_to_group(nix_sig)
    }

    fn force_kill(&self) -> Result<(), ProgramError> {
        self.send_signal_to_group(Signal::SIGKILL)
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
    ) -> Result<Box<dyn PlatformProcessGuard>, ProgramError> {
        Ok(Box::new(UnixProcessGuard::new(pid)))
    }

    fn default_uds_path(&self) -> PathBuf {
        PathBuf::from("/var/run/rsupervisord.sock")
    }

    fn is_elevated(&self) -> bool {
        nix::unistd::getuid().is_root()
    }

    fn validate_caller_privileges(&self, allow_unelevated: bool) -> Result<(), ProgramError> {
        if !self.is_elevated() && !allow_unelevated {
            tracing::debug!("Caller process is not running as root");
        }
        Ok(())
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

    fn bind_ipc_listener(&self, path: &Path) -> io::Result<Box<dyn PlatformIpcListener>> {
        let listener = UnixIpcListener::bind(path)?;
        Ok(Box::new(listener))
    }
}

fn cleanup_unix_ipc(p: PathBuf) {
    let _ = std::fs::remove_file(&p);
}

/// Unix domain socket IPC listener.
pub struct UnixIpcListener {
    listener: tokio::net::UnixListener,
    _cleanup: scopeguard::ScopeGuard<PathBuf, fn(PathBuf)>,
}

impl UnixIpcListener {
    pub fn bind(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent()
            && !parent.exists()
        {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::remove_file(path);

        let listener = tokio::net::UnixListener::bind(path)?;
        let cleanup = scopeguard::guard(path.to_path_buf(), cleanup_unix_ipc as fn(PathBuf));
        Ok(Self {
            listener,
            _cleanup: cleanup,
        })
    }
}

/// Verifies caller peer credentials on Unix domain sockets.
pub fn verify_caller_credentials(stream: &tokio::net::UnixStream) -> Result<(), ProgramError> {
    use nix::sys::socket::{getsockopt, sockopt::PeerCredentials};
    use std::os::fd::{AsRawFd, BorrowedFd};

    let borrowed_fd = unsafe { BorrowedFd::borrow_raw(stream.as_raw_fd()) };
    let creds = getsockopt(&borrowed_fd, PeerCredentials).map_err(|e| {
        ProgramError::PlatformError(format!("Failed to retrieve peer credentials: {}", e))
    })?;
    let caller_uid = Uid::from_raw(creds.uid());
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
            if let Err(e) = verify_caller_credentials(&stream) {
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
        StopSignal::CtrlBreak => Signal::SIGTERM,
        StopSignal::CtrlC => Signal::SIGINT,
    }
}

/// Parses a user specification string supporting: "1000", "1000:1000", "username", "username:groupname".
fn parse_user_spec(spec: &str) -> Result<(Option<Uid>, Option<Gid>), ProgramError> {
    let parts: Vec<&str> = spec.split(':').collect();
    let uid_part = parts[0].trim();
    let gid_part = if parts.len() > 1 {
        Some(parts[1].trim())
    } else {
        None
    };

    let uid = if let Ok(num) = uid_part.parse::<u32>() {
        Some(Uid::from_raw(num))
    } else if let Ok(Some(u)) = nix::unistd::User::from_name(uid_part) {
        Some(u.uid)
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
        None
    };

    Ok((uid, gid))
}

/// Configures Linux PR_SET_CHILD_SUBREAPER to adopt orphaned grandchild processes.
#[cfg(all(unix, target_os = "linux"))]
pub fn setup_subreaper() -> Result<(), ProgramError> {
    unsafe {
        if libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) != 0 {
            let err = std::io::Error::last_os_error();
            return Err(ProgramError::PlatformError(format!(
                "Failed to enable PR_SET_CHILD_SUBREAPER: {}",
                err
            )));
        }
    }
    Ok(())
}

#[cfg(all(unix, not(target_os = "linux")))]
pub fn setup_subreaper() -> Result<(), ProgramError> {
    Ok(())
}
