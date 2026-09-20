// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

#[cfg(windows)]
use crate::error::ProgramError;
#[cfg(windows)]
use crate::platform::traits::{PlatformBackend, PlatformProcessGuard};
#[cfg(windows)]
use crate::program::config::StopSignal;
#[cfg(windows)]
use std::path::PathBuf;
#[cfg(windows)]
use tokio::process::Command as TokioCommand;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
#[cfg(windows)]
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};

/// RAII wrapper for a Windows Job Object configured with kill-on-close.
#[cfg(windows)]
pub struct WinJobGuard {
    job_handle: HANDLE,
}

#[cfg(windows)]
impl WinJobGuard {
    pub fn new() -> Result<Self, ProgramError> {
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() || job == INVALID_HANDLE_VALUE {
                let err = std::io::Error::last_os_error();
                return Err(ProgramError::PlatformError(format!(
                    "Failed to create Windows Job Object: {}",
                    err
                )));
            }

            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );

            if ok == 0 {
                let err = std::io::Error::last_os_error();
                CloseHandle(job);
                return Err(ProgramError::PlatformError(format!(
                    "Failed to set JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: {}",
                    err
                )));
            }

            Ok(Self { job_handle: job })
        }
    }

    /// Assigns a process handle to this Job Object.
    ///
    /// # Safety
    ///
    /// `process_handle` must be a valid Windows process handle with `PROCESS_SET_QUOTA` and `PROCESS_TERMINATE`.
    pub unsafe fn assign_process(&self, process_handle: HANDLE) -> Result<(), ProgramError> {
        unsafe {
            let ok = AssignProcessToJobObject(self.job_handle, process_handle);
            if ok == 0 {
                let err = std::io::Error::last_os_error();
                return Err(ProgramError::PlatformError(format!(
                    "Failed to assign process to Job Object: {}",
                    err
                )));
            }
            Ok(())
        }
    }

    /// Terminates all processes assigned to the Job Object.
    pub fn terminate(&self, exit_code: u32) -> Result<(), ProgramError> {
        unsafe {
            let ok = TerminateJobObject(self.job_handle, exit_code);
            if ok == 0 {
                let err = std::io::Error::last_os_error();
                return Err(ProgramError::PlatformError(format!(
                    "Failed to terminate Job Object: {}",
                    err
                )));
            }
            Ok(())
        }
    }
}

#[cfg(windows)]
impl Drop for WinJobGuard {
    fn drop(&mut self) {
        if !self.job_handle.is_null() && self.job_handle != INVALID_HANDLE_VALUE {
            unsafe {
                CloseHandle(self.job_handle);
            }
        }
    }
}

#[cfg(windows)]
unsafe impl Send for WinJobGuard {}
#[cfg(windows)]
unsafe impl Sync for WinJobGuard {}

/// Guard managing a Windows Job Object and process tree.
#[cfg(windows)]
pub struct WindowsProcessGuard {
    pub pid: u32,
    job: WinJobGuard,
    last_cpu_sample: std::sync::Mutex<Option<(std::time::Instant, u64)>>,
}

#[cfg(windows)]
impl PlatformProcessGuard for WindowsProcessGuard {
    fn send_stop_signal(&self, signal: StopSignal) -> Result<(), ProgramError> {
        match signal {
            StopSignal::CtrlBreak | StopSignal::CtrlC => {
                // For console-attached programs, Ctrl events can be dispatched.
                // Otherwise fallback to terminating with clean code 0.
                self.job.terminate(0)
            }
            _ => self.job.terminate(0),
        }
    }

    fn force_kill(&self) -> Result<(), ProgramError> {
        self.job.terminate(1)
    }

    fn pid(&self) -> u32 {
        self.pid
    }

    fn query_metrics(&self) -> Result<crate::platform::traits::ProcessMetrics, ProgramError> {
        use windows_sys::Win32::System::JobObjects::{
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
            QueryInformationJobObject,
        };

        unsafe {
            let mut ret_len = 0;

            // 1. Query Memory
            let mut limit_info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            let mem_ok = QueryInformationJobObject(
                self.job.job_handle,
                JobObjectExtendedLimitInformation,
                &mut limit_info as *mut _ as *mut _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                &mut ret_len,
            );
            let memory_rss_bytes = if mem_ok != 0 {
                limit_info
                    .PeakJobMemoryUsed
                    .max(limit_info.PeakProcessMemoryUsed) as u64
            } else {
                0
            };

            // 2. Query CPU
            let mut acct_info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed();
            let cpu_ok = QueryInformationJobObject(
                self.job.job_handle,
                JobObjectBasicAccountingInformation,
                &mut acct_info as *mut _ as *mut _,
                std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                &mut ret_len,
            );

            let mut cpu_percent = 0.0f32;
            if cpu_ok != 0 {
                let user_time = acct_info.TotalUserTime.max(0) as u64;
                let kernel_time = acct_info.TotalKernelTime.max(0) as u64;
                let total_cpu_100ns = user_time + kernel_time;
                let now = std::time::Instant::now();

                if let Ok(mut lock) = self.last_cpu_sample.lock() {
                    if let Some((prev_instant, prev_cpu)) = *lock {
                        let delta_cpu = total_cpu_100ns.saturating_sub(prev_cpu);
                        let elapsed_100ns =
                            (now.duration_since(prev_instant).as_nanos() / 100) as u64;
                        if elapsed_100ns > 0 {
                            let cpus = std::thread::available_parallelism()
                                .map(|n| n.get())
                                .unwrap_or(1) as f64;
                            let pct = (delta_cpu as f64 / elapsed_100ns as f64) * 100.0 / cpus;
                            cpu_percent = (pct as f32).max(0.0);
                        }
                    }
                    *lock = Some((now, total_cpu_100ns));
                }
            }

            Ok(crate::platform::traits::ProcessMetrics {
                memory_rss_bytes,
                cpu_percent,
            })
        }
    }
}

/// Native Windows platform backend implementation.
#[cfg(windows)]
pub struct WindowsPlatformBackend;

#[cfg(windows)]
impl PlatformBackend for WindowsPlatformBackend {
    fn configure_command(
        &self,
        _cmd: &mut TokioCommand,
        _user: Option<&str>,
        _umask: Option<u32>,
    ) -> Result<(), ProgramError> {
        // Windows-specific process creation flags can be configured here if necessary.
        Ok(())
    }

    fn attach_child(
        &self,
        _child: &tokio::process::Child,
        pid: u32,
    ) -> Result<Box<dyn PlatformProcessGuard>, ProgramError> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
        };

        let job = WinJobGuard::new()?;

        unsafe {
            let handle = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            if handle.is_null() {
                let err = std::io::Error::last_os_error();
                return Err(ProgramError::PlatformError(format!(
                    "Failed to OpenProcess for PID {}: {}",
                    pid, err
                )));
            }
            let res = job.assign_process(handle);
            CloseHandle(handle);
            res?;
        }

        Ok(Box::new(WindowsProcessGuard {
            pid,
            job,
            last_cpu_sample: std::sync::Mutex::new(None),
        }))
    }

    fn default_uds_path(&self) -> PathBuf {
        PathBuf::from("C:\\ProgramData\\rsupervisord\\rsupervisord.sock")
    }

    fn is_elevated(&self) -> bool {
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
        use windows_sys::Win32::Security::{
            GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
        };
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

        unsafe {
            let mut token: HANDLE = std::mem::zeroed();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return false;
            }
            let mut elevation: TOKEN_ELEVATION = std::mem::zeroed();
            let mut size = std::mem::size_of::<TOKEN_ELEVATION>() as u32;
            let success = GetTokenInformation(
                token,
                TokenElevation,
                &mut elevation as *mut _ as *mut _,
                size,
                &mut size,
            );
            CloseHandle(token);
            success != 0 && elevation.TokenIsElevated != 0
        }
    }
}
