// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use windows_service::define_windows_service;
use windows_service::service::{
    ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept,
    ServiceDependency, ServiceErrorControl, ServiceExitCode, ServiceFailureActions,
    ServiceFailureResetPeriod, ServiceInfo, ServiceStartType, ServiceState, ServiceStatus,
    ServiceType,
};
use windows_service::service_control_handler::{
    self, ServiceControlHandlerResult, ServiceStatusHandle,
};
use windows_service::service_dispatcher;
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

use crate::config::SupervisorConfig;
use crate::daemon::DaemonArgs;
use crate::platform::traits::PlatformService;

pub struct WindowsService;

static SERVICE_CONTEXT: OnceLock<(DaemonArgs, PathBuf, String)> = OnceLock::new();

// ---------------------------------------------------------------------------
// Windows Service Control Manager (SCM) timing
// ---------------------------------------------------------------------------

/// Wait hint reported for StopPending so SCM allows child process drain.
const SCM_STOP_WAIT_HINT: Duration = Duration::from_secs(45);
/// Wait hint reported for StartPending until the Tokio runtime is built.
const SCM_START_WAIT_HINT: Duration = Duration::from_secs(30);
/// Checkpoint heartbeat interval while StopPending is reported to SCM.
const SCM_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(3);
/// Crash-recovery `SC_ACTION_RESTART` delays written at install time.
const SC_FAILURE_RESTART_DELAYS: [Duration; 3] = [
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(30),
];
/// How long start/restart wait for the service to reach Running.
const SERVICE_START_TIMEOUT: Duration = Duration::from_secs(15);
/// How long stop/uninstall wait for the service to reach Stopped.
const SERVICE_STOP_TIMEOUT: Duration = Duration::from_secs(30);
/// Status query poll interval inside [`wait_for_state`].
const STATE_POLL_INTERVAL: Duration = Duration::from_millis(200);

define_windows_service!(ffi_service_main, my_service_main);

/// Entry point called by the Windows Service Control Manager on a background thread.
fn my_service_main(_arguments: Vec<OsString>) {
    // 1. Working directory correction:
    // When invoked by SCM under NT AUTHORITY\SYSTEM, current working directory defaults to C:\Windows\System32.
    // Switch to the binary's directory so relative paths and log locations resolve predictably.
    if let Ok(exe_path) = std::env::current_exe()
        && let Some(parent) = exe_path.parent()
    {
        let _ = std::env::set_current_dir(parent);
    }

    // 2. Wrap the entire service execution loop in catch_unwind to prevent unwinding across the FFI boundary
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run_service_loop));

    match result {
        Ok(Ok(())) => {
            tracing::info!("Windows service execution terminated cleanly");
        }
        Ok(Err(e)) => {
            tracing::error!("Windows service loop exited with error: {:#}", e);
            eprintln!("Windows service error: {:#}", e);
        }
        Err(panic_err) => {
            let panic_msg = if let Some(s) = panic_err.downcast_ref::<&str>() {
                *s
            } else if let Some(s) = panic_err.downcast_ref::<String>() {
                s.as_str()
            } else {
                "Unknown panic payload"
            };
            tracing::error!(
                "CRITICAL: Windows service caught panic in service loop: {}",
                panic_msg
            );
            eprintln!("CRITICAL: Windows service caught panic: {}", panic_msg);
        }
    }
}

fn run_service_loop() -> anyhow::Result<()> {
    let (daemon_args, config_path, service_name) = SERVICE_CONTEXT
        .get()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Windows service execution context is not initialized"))?;

    let shutdown_token = CancellationToken::new();
    let shutdown_token_clone = shutdown_token.clone();

    let status_handle_cell: Arc<Mutex<Option<ServiceStatusHandle>>> = Arc::new(Mutex::new(None));
    let status_handle_cell_clone = status_handle_cell.clone();

    // Atomic flag indicating shutdown has been requested
    let is_stopping = Arc::new(AtomicBool::new(false));
    let is_stopping_clone = is_stopping.clone();

    let event_handler = move |control_event| -> ServiceControlHandlerResult {
        match control_event {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                tracing::info!("Received stop/shutdown event from Windows Service Control Manager");
                is_stopping_clone.store(true, Ordering::SeqCst);

                if let Ok(guard) = status_handle_cell_clone.lock()
                    && let Some(handle) = *guard
                {
                    // Report StopPending with a generous wait hint (45s) to allow child process drain
                    let _ = handle.set_service_status(ServiceStatus {
                        service_type: ServiceType::OWN_PROCESS,
                        current_state: ServiceState::StopPending,
                        controls_accepted: ServiceControlAccept::empty(),
                        exit_code: ServiceExitCode::Win32(0),
                        checkpoint: 1,
                        wait_hint: SCM_STOP_WAIT_HINT,
                        process_id: None,
                    });
                }
                shutdown_token_clone.cancel();
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };

    let status_handle = service_control_handler::register(&service_name, event_handler)?;
    if let Ok(mut guard) = status_handle_cell.lock() {
        *guard = Some(status_handle);
    }

    // Step 1: Report StartPending immediately so SCM watchdog knows startup is in progress
    let _ = status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::StartPending,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 1,
        wait_hint: SCM_START_WAIT_HINT,
        process_id: None,
    });

    let mut final_exit_code = ServiceExitCode::Win32(0);

    // Scope guard guaranteeing ServiceState::Stopped is ALWAYS reported to SCM upon abnormal or early exit
    let stopped_guard = scopeguard::guard(status_handle, |handle| {
        let _ = handle.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::Stopped,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code: ServiceExitCode::Win32(1),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        });
    });

    let res = (|| -> anyhow::Result<()> {
        // Step 2: Determine Tokio worker threads and build runtime
        let file_threads = if config_path.exists() {
            SupervisorConfig::from_file(&config_path)
                .ok()
                .and_then(|c| c.worker_threads.map(|w| w as u32))
        } else {
            None
        };

        let worker_threads = daemon_args
            .worker_threads
            .map(|w| w as u32)
            .or(file_threads);

        let rt = crate::build_tokio_runtime(worker_threads)?;

        // Step 3: Transition to Running state before entering supervisor daemon
        status_handle.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::Running,
            controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })?;

        // Step 4: Start background SCM shutdown heartbeat thread
        // If SCM issues Stop, child process termination may take multiple seconds.
        // The heartbeat thread periodically increments the checkpoint with a 45s wait hint
        // so SCM never terminates the service prematurely during a graceful shutdown.
        let status_handle_hb = status_handle_cell.clone();
        let is_stopping_hb = is_stopping.clone();
        let hb_terminated = Arc::new(AtomicBool::new(false));
        let hb_terminated_clone = hb_terminated.clone();

        let hb_thread = thread::Builder::new()
            .name(format!("{}-scm-heartbeat", service_name))
            .spawn(move || {
                let mut checkpoint = 2u32;
                while !hb_terminated_clone.load(Ordering::SeqCst) {
                    thread::sleep(SCM_HEARTBEAT_INTERVAL);
                    if hb_terminated_clone.load(Ordering::SeqCst) {
                        break;
                    }
                    if is_stopping_hb.load(Ordering::SeqCst)
                        && let Ok(guard) = status_handle_hb.lock()
                        && let Some(handle) = *guard
                    {
                        let _ = handle.set_service_status(ServiceStatus {
                            service_type: ServiceType::OWN_PROCESS,
                            current_state: ServiceState::StopPending,
                            controls_accepted: ServiceControlAccept::empty(),
                            exit_code: ServiceExitCode::Win32(0),
                            checkpoint,
                            wait_hint: SCM_STOP_WAIT_HINT,
                            process_id: None,
                        });
                        checkpoint = checkpoint.saturating_add(1);
                    }
                }
            });

        // Step 5: Execute supervisor daemon
        let daemon_result = rt.block_on(crate::daemon::run_daemon(
            daemon_args,
            config_path,
            Some(shutdown_token),
        ));

        // Stop heartbeat thread cleanly
        hb_terminated.store(true, Ordering::SeqCst);
        if let Ok(handle) = hb_thread {
            let _ = handle.join();
        }

        if let Err(e) = daemon_result {
            final_exit_code = ServiceExitCode::Win32(1);
            return Err(e);
        }

        Ok(())
    })();

    if let Err(ref e) = res {
        tracing::error!("Daemon exited with error: {:#}", e);
        if final_exit_code == ServiceExitCode::Win32(0) {
            final_exit_code = ServiceExitCode::Win32(1);
        }
    }

    // Step 6: Disarm scope guard and report Stopped state to SCM with the determined exit code
    let status_handle = scopeguard::ScopeGuard::into_inner(stopped_guard);
    let _ = status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: final_exit_code,
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    });

    res
}

/// Runs the daemon as a Windows Service managed by the SCM.
pub fn run_as_service(
    args: DaemonArgs,
    config_path: PathBuf,
    cmd_name: String,
) -> anyhow::Result<()> {
    SERVICE_CONTEXT
        .set((args, config_path, cmd_name.clone()))
        .map_err(|_| anyhow::anyhow!("Service execution context has already been initialized"))?;

    service_dispatcher::start(&cmd_name, ffi_service_main).map_err(|e| {
        anyhow::anyhow!(
            "Failed to start Windows service dispatcher: {}. \
             Note: --service must be executed by the Windows Service Control Manager.",
            e
        )
    })?;

    Ok(())
}

/// Installs the binary as an auto-start Windows system service with crash recovery.
///
/// `exe_path` and `config_path` are resolved by the caller
/// (`service::run_service_op`) so the companion ctl binary can install
/// the sibling daemon instead of itself.
pub fn install_service(cmd_name: &str, exe_path: &Path, config_path: &Path) -> anyhow::Result<()> {
    if !crate::platform::native_platform().is_elevated() {
        anyhow::bail!(
            "Administrator privileges are required to install the Windows service. Please run as Administrator."
        );
    }

    let launch_arguments = vec![
        OsString::from("--service"),
        OsString::from("-c"),
        config_path.as_os_str().to_os_string(),
    ];

    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )?;

    let display_name = format!("{} Process Supervisor", cmd_name);
    let service_info = ServiceInfo {
        name: OsString::from(cmd_name),
        display_name: OsString::from(&display_name),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe_path.to_path_buf(),
        launch_arguments,
        dependencies: vec![
            ServiceDependency::Group(OsString::from("network")),
            ServiceDependency::Service(OsString::from("Tcpip")),
            ServiceDependency::Service(OsString::from("Dhcp")),
            ServiceDependency::Service(OsString::from("Eventlog")),
        ],
        account_name: None,
        account_password: None,
    };

    let service = match manager.create_service(&service_info, ServiceAccess::CHANGE_CONFIG) {
        Ok(s) => s,
        Err(windows_service::Error::Winapi(ref e)) if e.raw_os_error() == Some(1073) => {
            anyhow::bail!(
                "Windows service '{}' already exists. Uninstall it first or use --restart.",
                cmd_name
            );
        }
        Err(e) => return Err(e.into()),
    };

    let _ = service.set_description(
        "High-performance asynchronous process supervision daemon with DAG orchestration",
    );

    // Configure delayed auto-start so dependencies finish loading before service start
    let _ = service.set_delayed_auto_start(true);

    // Configure SC_ACTION_RESTART crash recovery actions (restart after 5s, 10s, 30s)
    let actions: Vec<ServiceAction> = SC_FAILURE_RESTART_DELAYS
        .into_iter()
        .map(|delay| ServiceAction {
            action_type: ServiceActionType::Restart,
            delay,
        })
        .collect();
    let failure_actions = ServiceFailureActions {
        reset_period: ServiceFailureResetPeriod::After(crate::consts::MAX_TIMEOUT),
        reboot_msg: None,
        command: None,
        actions: Some(actions),
    };
    let _ = service.update_failure_actions(failure_actions);
    let _ = service.set_failure_actions_on_non_crash_failures(true);

    println!("Successfully installed Windows service: {}", cmd_name);
    Ok(())
}

/// Uninstalls the Windows system service.
pub fn uninstall_service(cmd_name: &str) -> anyhow::Result<()> {
    if !crate::platform::native_platform().is_elevated() {
        anyhow::bail!(
            "Administrator privileges are required to uninstall the Windows service. Please run as Administrator."
        );
    }

    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;

    let service = manager.open_service(
        cmd_name,
        ServiceAccess::STOP | ServiceAccess::DELETE | ServiceAccess::QUERY_STATUS,
    )?;

    if let Ok(status) = service.query_status()
        && status.current_state != ServiceState::Stopped
    {
        println!("Stopping service '{}' before uninstallation...", cmd_name);
        let _ = service.stop();
        let _ = wait_for_state(&service, ServiceState::Stopped, SERVICE_STOP_TIMEOUT);
    }

    service.delete()?;
    println!("Successfully uninstalled Windows service: {}", cmd_name);
    Ok(())
}

/// Starts the Windows system service.
pub fn start_service(cmd_name: &str) -> anyhow::Result<()> {
    if !crate::platform::native_platform().is_elevated() {
        anyhow::bail!(
            "Administrator privileges are required to start the Windows service. Please run as Administrator."
        );
    }

    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;

    let service =
        manager.open_service(cmd_name, ServiceAccess::START | ServiceAccess::QUERY_STATUS)?;

    let status = service.query_status()?;
    if status.current_state == ServiceState::Running {
        println!("Service '{}' is already running.", cmd_name);
        return Ok(());
    }

    println!("Starting service '{}'...", cmd_name);
    service.start(&[] as &[&OsStr])?;

    if wait_for_state(&service, ServiceState::Running, SERVICE_START_TIMEOUT)? {
        println!("Service '{}' started successfully.", cmd_name);
    } else {
        println!("Service start requested, but service state is still pending.");
    }

    Ok(())
}

/// Stops the Windows system service.
pub fn stop_service(cmd_name: &str) -> anyhow::Result<()> {
    if !crate::platform::native_platform().is_elevated() {
        anyhow::bail!(
            "Administrator privileges are required to stop the Windows service. Please run as Administrator."
        );
    }

    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;

    let service =
        manager.open_service(cmd_name, ServiceAccess::STOP | ServiceAccess::QUERY_STATUS)?;

    let status = service.query_status()?;
    if status.current_state == ServiceState::Stopped {
        println!("Service '{}' is already stopped.", cmd_name);
        return Ok(());
    }

    println!("Stopping service '{}'...", cmd_name);
    service.stop()?;

    if wait_for_state(&service, ServiceState::Stopped, SERVICE_STOP_TIMEOUT)? {
        println!("Service '{}' stopped successfully.", cmd_name);
    } else {
        println!("Service stop requested, but service state is still pending.");
    }

    Ok(())
}

/// Restarts the Windows system service.
pub fn restart_service(cmd_name: &str) -> anyhow::Result<()> {
    if !crate::platform::native_platform().is_elevated() {
        anyhow::bail!(
            "Administrator privileges are required to restart the Windows service. Please run as Administrator."
        );
    }

    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;

    let service = manager.open_service(
        cmd_name,
        ServiceAccess::START | ServiceAccess::STOP | ServiceAccess::QUERY_STATUS,
    )?;

    let status = service.query_status()?;
    if status.current_state != ServiceState::Stopped {
        println!("Stopping service '{}'...", cmd_name);
        let _ = service.stop();
        let _ = wait_for_state(&service, ServiceState::Stopped, SERVICE_STOP_TIMEOUT);
    }

    println!("Starting service '{}'...", cmd_name);
    service.start(&[] as &[&OsStr])?;

    if wait_for_state(&service, ServiceState::Running, SERVICE_START_TIMEOUT)? {
        println!("Service '{}' restarted successfully.", cmd_name);
    } else {
        println!("Service restart requested, but service state is still pending.");
    }

    Ok(())
}

/// Polls the service status until it matches `target` or times out.
fn wait_for_state(
    service: &windows_service::service::Service,
    target: ServiceState,
    timeout: Duration,
) -> anyhow::Result<bool> {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        thread::sleep(STATE_POLL_INTERVAL);
        let status = service.query_status()?;
        if status.current_state == target {
            return Ok(true);
        }
    }
    Ok(false)
}

impl PlatformService for WindowsService {
    fn install(&self, cmd_name: &str, exe_path: &Path, config_path: &Path) -> anyhow::Result<()> {
        install_service(cmd_name, exe_path, config_path)
    }

    fn uninstall(&self, cmd_name: &str) -> anyhow::Result<()> {
        uninstall_service(cmd_name)
    }

    fn start(&self, cmd_name: &str) -> anyhow::Result<()> {
        start_service(cmd_name)
    }

    fn stop(&self, cmd_name: &str) -> anyhow::Result<()> {
        stop_service(cmd_name)
    }

    fn restart(&self, cmd_name: &str) -> anyhow::Result<()> {
        restart_service(cmd_name)
    }

    fn run_service(
        &self,
        daemon_args: DaemonArgs,
        config_path: PathBuf,
        cmd_name: String,
    ) -> anyhow::Result<()> {
        run_as_service(daemon_args, config_path, cmd_name)
    }
}
