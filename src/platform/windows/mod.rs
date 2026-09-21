// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

pub mod service;
pub use service::WindowsService;

use crate::error::ProgramError;
use crate::platform::traits::{
    AsyncStream, PlatformBackend, PlatformIpcListener, PlatformProcessGuard, PlatformService,
};
use crate::program::config::StopSignal;
use async_trait::async_trait;
use std::io;
use std::path::{Path, PathBuf};
use tokio::process::Command as TokioCommand;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};

/// RAII wrapper for a Windows Job Object configured with kill-on-close.
pub struct WinJobGuard {
    job_handle: HANDLE,
}

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

            let job_guard = scopeguard::guard(job, |j| {
                CloseHandle(j);
            });

            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

            let ok = SetInformationJobObject(
                *job_guard,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );

            if ok == 0 {
                let err = std::io::Error::last_os_error();
                return Err(ProgramError::PlatformError(format!(
                    "Failed to set JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: {}",
                    err
                )));
            }

            let job_handle = scopeguard::ScopeGuard::into_inner(job_guard);
            Ok(Self { job_handle })
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

impl Drop for WinJobGuard {
    fn drop(&mut self) {
        if !self.job_handle.is_null() && self.job_handle != INVALID_HANDLE_VALUE {
            unsafe {
                CloseHandle(self.job_handle);
            }
        }
    }
}

unsafe impl Send for WinJobGuard {}
unsafe impl Sync for WinJobGuard {}

/// Guard managing a Windows Job Object and process tree.
pub struct WindowsProcessGuard {
    pub pid: u32,
    job: WinJobGuard,
    last_cpu_sample: std::sync::Mutex<Option<(std::time::Instant, u64)>>,
}

#[async_trait]
impl PlatformProcessGuard for WindowsProcessGuard {
    async fn wait_exit(
        &mut self,
        child: &mut tokio::process::Child,
    ) -> std::io::Result<std::process::ExitStatus> {
        // Tokio on Windows registers a kernel event wait on the Win32 process handle (hProcess)
        // using RegisterWaitForSingleObject. This is 100% event-driven by the NT kernel without polling.
        child.wait().await
    }

    fn send_stop_signal(&self, signal: StopSignal) -> Result<(), ProgramError> {
        match signal {
            StopSignal::Kill => self.job.terminate(1),
            _ => {
                let posted = post_wm_close_to_process(self.pid);
                if posted > 0 {
                    tracing::debug!(
                        pid = self.pid,
                        posted,
                        "Posted WM_CLOSE to GUI application window(s)"
                    );
                    Ok(())
                } else {
                    // Headless / console application: terminate cleanly via Job Object with code 0
                    self.job.terminate(0)
                }
            }
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

/// Posts WM_CLOSE to all top-level windows belonging to the given process ID.
/// Returns the number of windows to which WM_CLOSE was posted.
pub fn post_wm_close_to_process(pid: u32) -> usize {
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, PostMessageW, WM_CLOSE,
    };

    struct EnumContext {
        target_pid: u32,
        posted_count: usize,
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = unsafe { &mut *(lparam as *mut EnumContext) };
        let mut proc_id: u32 = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut proc_id);
            if proc_id == ctx.target_pid {
                PostMessageW(hwnd, WM_CLOSE, 0, 0);
                ctx.posted_count += 1;
            }
        }
        1
    }

    let mut ctx = EnumContext {
        target_pid: pid,
        posted_count: 0,
    };

    unsafe {
        EnumWindows(Some(enum_proc), &mut ctx as *mut _ as LPARAM);
    }

    ctx.posted_count
}

/// Native Windows platform backend implementation.
pub struct WindowsPlatformBackend;

#[async_trait]
impl PlatformBackend for WindowsPlatformBackend {
    fn configure_command(
        &self,
        _cmd: &mut TokioCommand,
        user: Option<&str>,
        umask: Option<u32>,
    ) -> Result<(), ProgramError> {
        if user.is_some() {
            tracing::warn!("'user' configuration is not supported on Windows and will be ignored");
        }
        if umask.is_some() {
            tracing::warn!("'umask' configuration is not supported on Windows and will be ignored");
        }
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
            let _handle_guard = scopeguard::guard(handle, |h| {
                CloseHandle(h);
            });
            job.assign_process(handle)?;
        }

        Ok(Box::new(WindowsProcessGuard {
            pid,
            job,
            last_cpu_sample: std::sync::Mutex::new(None),
        }))
    }

    fn default_uds_path(&self) -> PathBuf {
        let cmd_name = crate::config::paths::get_cmd_name();
        self.default_local_ipc_path(&cmd_name, None)
    }

    fn default_local_ipc_path(&self, cmd_name: &str, _config_dir: Option<&Path>) -> PathBuf {
        PathBuf::from(format!(r"\\.\pipe\{}", cmd_name))
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
            let _token_guard = scopeguard::guard(token, |t| {
                CloseHandle(t);
            });
            let mut elevation: TOKEN_ELEVATION = std::mem::zeroed();
            let mut size = std::mem::size_of::<TOKEN_ELEVATION>() as u32;
            let success = GetTokenInformation(
                token,
                TokenElevation,
                &mut elevation as *mut _ as *mut _,
                size,
                &mut size,
            );
            success != 0 && elevation.TokenIsElevated != 0
        }
    }

    fn validate_caller_privileges(&self, allow_unelevated: bool) -> Result<(), ProgramError> {
        if !self.is_elevated() && !allow_unelevated {
            tracing::debug!("Caller process is not running as Administrator");
        }
        Ok(())
    }

    fn default_stop_signal(&self) -> StopSignal {
        StopSignal::CtrlBreak
    }

    fn build_shell_command(&self, command: &str) -> TokioCommand {
        use std::os::windows::process::CommandExt;
        let mut cmd = TokioCommand::new("cmd");
        cmd.as_std_mut().raw_arg(format!("/C \"{}\"", command));
        cmd
    }

    async fn connect_ipc(&self, path: &Path) -> io::Result<Box<dyn AsyncStream>> {
        use std::os::windows::io::{FromRawSocket, IntoRawSocket};
        let std_stream = uds_windows::UnixStream::connect(path)?;
        let raw = std_stream.into_raw_socket();
        let std_tcp = unsafe { std::net::TcpStream::from_raw_socket(raw) };
        std_tcp.set_nonblocking(true)?;
        let stream = tokio::net::TcpStream::from_std(std_tcp)?;
        Ok(Box::new(stream))
    }

    async fn connect_named_pipe(&self, path: &Path) -> io::Result<Box<dyn AsyncStream>> {
        let client = tokio::net::windows::named_pipe::ClientOptions::new().open(path)?;
        Ok(Box::new(client))
    }

    fn bind_ipc_listener(&self, path: &Path) -> io::Result<Box<dyn PlatformIpcListener>> {
        if path.to_string_lossy().starts_with(r"\\.\pipe\") {
            let listener = WindowsNamedPipeListener::bind(path)?;
            Ok(Box::new(listener))
        } else {
            let listener = WindowsUdsListener::bind(path)?;
            Ok(Box::new(listener))
        }
    }

    fn default_daemon_log_path(&self, cmd_name: &str, config_dir: Option<&Path>) -> PathBuf {
        let dir = config_dir.unwrap_or_else(|| Path::new("."));
        dir.join("logs").join(format!("{}.log", cmd_name))
    }

    fn default_program_log_path(
        &self,
        _cmd_name: &str,
        program_name: &str,
        config_dir: Option<&Path>,
    ) -> PathBuf {
        let dir = config_dir.unwrap_or_else(|| Path::new("."));
        let sanitized = program_name.replace(':', "_");
        dir.join("logs").join(format!("{}.log", sanitized))
    }

    fn default_system_config_dir(&self, _cmd_name: &str) -> Option<PathBuf> {
        None
    }

    fn hostname(&self) -> String {
        std::env::var("COMPUTERNAME").unwrap_or_else(|_| "localhost".to_string())
    }

    fn resolve_executable(&self, command: &str, working_dir: Option<&Path>) -> Option<PathBuf> {
        let base_path = Path::new(command);
        let extensions = ["", ".exe", ".cmd", ".bat"];

        let try_dir = |dir: &Path| -> Option<PathBuf> {
            for ext in &extensions {
                let candidate = if ext.is_empty() || command.ends_with(ext) {
                    dir.join(base_path)
                } else {
                    dir.join(format!("{}{}", command, ext))
                };
                if candidate.is_file() {
                    return candidate.canonicalize().ok().or(Some(candidate));
                }
            }
            None
        };

        if base_path.is_absolute() {
            for ext in &extensions {
                let candidate = if ext.is_empty() || command.ends_with(ext) {
                    base_path.to_path_buf()
                } else {
                    PathBuf::from(format!("{}{}", command, ext))
                };
                if candidate.is_file() {
                    return candidate.canonicalize().ok().or(Some(candidate));
                }
            }
            return None;
        }

        if command.contains('/') || command.contains('\\') {
            let wd = working_dir.unwrap_or_else(|| Path::new("."));
            return try_dir(wd);
        }

        if let Some(wd) = working_dir
            && let Some(p) = try_dir(wd)
        {
            return Some(p);
        }

        if let Some(p) = try_dir(Path::new(".")) {
            return Some(p);
        }

        if let Some(paths) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&paths) {
                if let Some(p) = try_dir(&dir) {
                    return Some(p);
                }
            }
        }

        None
    }

    fn service(&self) -> &dyn PlatformService {
        &WindowsService
    }
}

/// Windows named pipe IPC listener.
pub struct WindowsNamedPipeListener {
    pipe_name: String,
    is_first: bool,
}

impl WindowsNamedPipeListener {
    pub fn bind(path: &Path) -> io::Result<Self> {
        Ok(Self {
            pipe_name: path.to_string_lossy().to_string(),
            is_first: true,
        })
    }
}

#[async_trait]
impl PlatformIpcListener for WindowsNamedPipeListener {
    async fn accept(&mut self) -> io::Result<Box<dyn AsyncStream>> {
        let server = tokio::net::windows::named_pipe::ServerOptions::new()
            .first_pipe_instance(self.is_first)
            .create(&self.pipe_name)?;
        self.is_first = false;
        server.connect().await?;
        Ok(Box::new(server))
    }
}

fn cleanup_windows_uds(p: PathBuf) {
    let _ = uds_windows::UnixStream::connect(&p);
    let _ = std::fs::remove_file(&p);
}

/// Windows Unix Domain Socket (AF_UNIX) listener.
pub struct WindowsUdsListener {
    listener: std::sync::Arc<uds_windows::UnixListener>,
    _cleanup: scopeguard::ScopeGuard<PathBuf, fn(PathBuf)>,
}

impl WindowsUdsListener {
    pub fn bind(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent()
            && !parent.exists()
        {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::remove_file(path);

        let listener = uds_windows::UnixListener::bind(path)?;
        let cleanup = scopeguard::guard(path.to_path_buf(), cleanup_windows_uds as fn(PathBuf));
        Ok(Self {
            listener: std::sync::Arc::new(listener),
            _cleanup: cleanup,
        })
    }
}

#[async_trait]
impl PlatformIpcListener for WindowsUdsListener {
    async fn accept(&mut self) -> io::Result<Box<dyn AsyncStream>> {
        use std::os::windows::io::{FromRawSocket, IntoRawSocket};

        let l = self.listener.clone();
        let (std_stream, _) = tokio::task::spawn_blocking(move || l.accept())
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::Interrupted, e))??;

        let raw = std_stream.into_raw_socket();
        let std_tcp = unsafe { std::net::TcpStream::from_raw_socket(raw) };
        std_tcp.set_nonblocking(true)?;
        let async_stream = tokio::net::TcpStream::from_std(std_tcp)?;
        Ok(Box::new(async_stream))
    }
}

static GUI_SHUTDOWN_NOTIFIER: parking_lot::Mutex<
    Option<tokio::sync::mpsc::UnboundedSender<&'static str>>,
> = parking_lot::Mutex::new(None);

unsafe extern "system" fn window_proc(
    hwnd: windows_sys::Win32::Foundation::HWND,
    msg: u32,
    wparam: windows_sys::Win32::Foundation::WPARAM,
    lparam: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::LRESULT {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DefWindowProcW, DestroyWindow, PostQuitMessage, WM_CLOSE, WM_DESTROY, WM_ENDSESSION,
        WM_QUERYENDSESSION,
    };

    match msg {
        WM_CLOSE => {
            tracing::info!("Received WM_CLOSE window message, initiating graceful shutdown");
            if let Some(ref sender) = *GUI_SHUTDOWN_NOTIFIER.lock() {
                let _ = sender.send("WM_CLOSE");
            }
            unsafe {
                DestroyWindow(hwnd);
            }
            0
        }
        WM_QUERYENDSESSION => {
            // Return 1 (TRUE) indicating the application agrees to terminate
            1
        }
        WM_ENDSESSION => {
            if wparam != 0 {
                tracing::info!(
                    "Received WM_ENDSESSION window message, initiating graceful shutdown"
                );
                if let Some(ref sender) = *GUI_SHUTDOWN_NOTIFIER.lock() {
                    let _ = sender.send("WM_ENDSESSION");
                }
            }
            0
        }
        WM_DESTROY => {
            unsafe {
                PostQuitMessage(0);
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Helper struct that ensures the hidden GUI listener window and message pump thread
/// are cleanly terminated and destroyed when dropped.
struct GuiWindowGuard {
    hwnd: isize,
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl Drop for GuiWindowGuard {
    fn drop(&mut self) {
        // Clear global sender
        *GUI_SHUTDOWN_NOTIFIER.lock() = None;

        if self.hwnd != 0 {
            unsafe {
                use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
                PostMessageW(
                    self.hwnd as windows_sys::Win32::Foundation::HWND,
                    WM_CLOSE,
                    0,
                    0,
                );
            }
        }
        if let Some(h) = self.thread_handle.take() {
            let _ = h.join();
        }
    }
}

fn spawn_gui_message_window(
    tx: tokio::sync::mpsc::UnboundedSender<&'static str>,
) -> Option<GuiWindowGuard> {
    use std::sync::mpsc::sync_channel;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DispatchMessageW, GetMessageW, MSG, RegisterClassW, TranslateMessage,
        WNDCLASSW,
    };

    *GUI_SHUTDOWN_NOTIFIER.lock() = Some(tx);

    let (ready_tx, ready_rx) = sync_channel::<isize>(1);

    let thread_handle = std::thread::Builder::new()
        .name("gui-msg-listener".to_string())
        .spawn(move || unsafe {
            let class_name = format!("rsupervisord_hidden_class_{}\0", std::process::id())
                .encode_utf16()
                .collect::<Vec<u16>>();
            let window_title = "rsupervisord_shutdown_listener\0"
                .encode_utf16()
                .collect::<Vec<u16>>();

            let hinstance = GetModuleHandleW(std::ptr::null());

            let wnd_class = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(window_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinstance,
                hIcon: std::ptr::null_mut(),
                hCursor: std::ptr::null_mut(),
                hbrBackground: std::ptr::null_mut(),
                lpszMenuName: std::ptr::null(),
                lpszClassName: class_name.as_ptr(),
            };

            let _ = RegisterClassW(&wnd_class);

            let hwnd = CreateWindowExW(
                0,
                class_name.as_ptr(),
                window_title.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                hinstance,
                std::ptr::null_mut(),
            );

            let _ = ready_tx.send(hwnd as isize);

            if !hwnd.is_null() {
                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        })
        .ok()?;

    let hwnd = ready_rx.recv().unwrap_or(0);
    if hwnd == 0 {
        tracing::debug!("Failed to create hidden GUI window for shutdown notifications");
        return None;
    }

    Some(GuiWindowGuard {
        hwnd,
        thread_handle: Some(thread_handle),
    })
}

/// Asynchronously waits for any Windows shutdown signal:
/// - Windows console signals: Ctrl+C, Ctrl+Break, Ctrl+Close (console window close "X"), Ctrl+Shutdown, Ctrl+Logoff
/// - Windows GUI window messages: WM_CLOSE (e.g. from taskkill or GUI window close), WM_ENDSESSION
pub async fn wait_for_windows_shutdown_signal() {
    let (gui_tx, mut gui_rx) = tokio::sync::mpsc::unbounded_channel::<&'static str>();
    let _gui_guard = spawn_gui_message_window(gui_tx);

    let mut ctrl_c = tokio::signal::windows::ctrl_c().ok();
    let mut ctrl_break = tokio::signal::windows::ctrl_break().ok();
    let mut ctrl_close = tokio::signal::windows::ctrl_close().ok();
    let mut ctrl_shutdown = tokio::signal::windows::ctrl_shutdown().ok();
    let mut ctrl_logoff = tokio::signal::windows::ctrl_logoff().ok();

    tokio::select! {
        Some(msg) = gui_rx.recv() => {
            tracing::info!("Received GUI window message ({}), initiating graceful shutdown", msg);
        }
        _ = async {
            if let Some(ref mut s) = ctrl_c {
                s.recv().await;
            } else {
                std::future::pending::<()>().await;
            }
        } => {
            tracing::info!("Received Ctrl+C, initiating graceful shutdown");
        }
        _ = async {
            if let Some(ref mut s) = ctrl_break {
                s.recv().await;
            } else {
                std::future::pending::<()>().await;
            }
        } => {
            tracing::info!("Received Ctrl+Break, initiating graceful shutdown");
        }
        _ = async {
            if let Some(ref mut s) = ctrl_close {
                s.recv().await;
            } else {
                std::future::pending::<()>().await;
            }
        } => {
            tracing::info!("Received console/GUI window close event, initiating graceful shutdown");
        }
        _ = async {
            if let Some(ref mut s) = ctrl_shutdown {
                s.recv().await;
            } else {
                std::future::pending::<()>().await;
            }
        } => {
            tracing::info!("Received system shutdown event, initiating graceful shutdown");
        }
        _ = async {
            if let Some(ref mut s) = ctrl_logoff {
                s.recv().await;
            } else {
                std::future::pending::<()>().await;
            }
        } => {
            tracing::info!("Received user logoff event, initiating graceful shutdown");
        }
    }
}
