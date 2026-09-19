#[cfg(unix)]
use crate::error::ProgramError;
#[cfg(unix)]
use crate::platform::traits::{PlatformBackend, PlatformProcessGuard};
#[cfg(unix)]
use crate::program::config::StopSignal;
#[cfg(unix)]
use nix::sys::signal::{self, Signal};
#[cfg(unix)]
use nix::unistd::{Gid, Pid, Uid};
#[cfg(unix)]
use std::path::PathBuf;
#[cfg(unix)]
use tokio::process::Command as TokioCommand;

/// Guard managing a Unix process group.
#[cfg(unix)]
pub struct UnixProcessGuard {
    pub pid: u32,
    pub pgid: i32,
    last_cpu_sample: std::sync::Mutex<Option<(std::time::Instant, u64)>>,
}

#[cfg(unix)]
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
}

#[cfg(unix)]
impl PlatformProcessGuard for UnixProcessGuard {
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
#[cfg(unix)]
pub struct UnixPlatformBackend;

#[cfg(unix)]
impl PlatformBackend for UnixPlatformBackend {
    fn configure_command(
        &self,
        cmd: &mut TokioCommand,
        user: Option<&str>,
        umask_val: Option<u32>,
    ) -> Result<(), ProgramError> {
        let (uid, gid) = if let Some(user_str) = user {
            parse_user_spec(user_str)?
        } else {
            (None, None)
        };

        unsafe {
            cmd.pre_exec(move || {
                // 1. Establish an independent process group (setpgid(0, 0))
                nix::unistd::setpgid(Pid::from_raw(0), Pid::from_raw(0))
                    .map_err(|e| std::io::Error::other(format!("Failed to setpgid: {}", e)))?;

                // 2. Set file mode creation mask (umask)
                if let Some(mask) = umask_val {
                    nix::sys::stat::umask(nix::sys::stat::Mode::from_bits_truncate(mask));
                }

                // 3. Drop privileges (setgid first, then setuid)
                if let Some(g) = gid {
                    nix::unistd::setgid(g).map_err(|e| {
                        std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            format!("Failed to setgid({}): {}", g, e),
                        )
                    })?;
                }

                if let Some(u) = uid {
                    nix::unistd::setuid(u).map_err(|e| {
                        std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            format!("Failed to setuid({}): {}", u, e),
                        )
                    })?;
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
}

/// Converts generic StopSignal to POSIX Signal.
#[cfg(unix)]
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
#[cfg(unix)]
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
