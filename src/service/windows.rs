// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use windows_service::define_windows_service;
use windows_service::service::{
    ServiceAccess, ServiceControl, ServiceControlAccept, ServiceDependency, ServiceErrorControl,
    ServiceExitCode, ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{
    self, ServiceControlHandlerResult, ServiceStatusHandle,
};
use windows_service::service_dispatcher;
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

use crate::config::SupervisorConfig;
use crate::daemon::DaemonArgs;

static SERVICE_CONTEXT: OnceLock<(DaemonArgs, PathBuf, String)> = OnceLock::new();

define_windows_service!(ffi_service_main, my_service_main);

/// Entry point called by the Windows Service Control Manager on a background thread.
fn my_service_main(_arguments: Vec<OsString>) {
    if let Err(e) = run_service_loop() {
        tracing::error!("Windows service loop terminated with error: {}", e);
    }
}

fn run_service_loop() -> anyhow::Result<()> {
    let (daemon_args, config_path, service_name) = SERVICE_CONTEXT
        .get()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Service execution context is not initialized"))?;

    let shutdown_token = CancellationToken::new();
    let shutdown_token_clone = shutdown_token.clone();

    let status_handle_cell: Arc<Mutex<Option<ServiceStatusHandle>>> = Arc::new(Mutex::new(None));
    let status_handle_cell_clone = status_handle_cell.clone();

    let event_handler = move |control_event| -> ServiceControlHandlerResult {
        match control_event {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                tracing::info!("Received stop/shutdown event from Windows Service Control Manager");
                if let Ok(guard) = status_handle_cell_clone.lock()
                    && let Some(handle) = *guard
                {
                    let _ = handle.set_service_status(ServiceStatus {
                        service_type: ServiceType::OWN_PROCESS,
                        current_state: ServiceState::StopPending,
                        controls_accepted: ServiceControlAccept::empty(),
                        exit_code: ServiceExitCode::Win32(0),
                        checkpoint: 1,
                        wait_hint: Duration::from_secs(30),
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

    // Report running state to SCM
    status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;

    // Determine Tokio worker threads
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
    let daemon_result = rt.block_on(crate::daemon::run_daemon(
        daemon_args,
        config_path,
        Some(shutdown_token),
    ));

    let exit_code = match daemon_result {
        Ok(()) => ServiceExitCode::Win32(0),
        Err(e) => {
            tracing::error!("Daemon exited with error: {}", e);
            ServiceExitCode::Win32(1)
        }
    };

    // Report stopped state to SCM
    let _ = status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code,
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    });

    Ok(())
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

/// Installs the binary as an auto-start Windows system service.
pub fn install_service(cmd_name: &str, config_path: Option<&Path>) -> anyhow::Result<()> {
    if !crate::platform::native_platform().is_elevated() {
        anyhow::bail!(
            "Administrator privileges are required to install the Windows service. Please run as Administrator."
        );
    }

    let exe_path = std::env::current_exe()?;
    let mut launch_arguments = vec![OsString::from("--service")];

    if let Some(cfg) = config_path {
        let abs_path = if cfg.is_absolute() {
            cfg.to_path_buf()
        } else {
            std::env::current_dir()?.join(cfg)
        };
        if !abs_path.exists() {
            eprintln!(
                "Warning: Configuration file {:?} does not exist yet. Please ensure it is present before starting the service.",
                abs_path
            );
        }
        launch_arguments.push(OsString::from("-c"));
        launch_arguments.push(abs_path.into_os_string());
    }

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
        executable_path: exe_path,
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
        let _ = wait_for_state(&service, ServiceState::Stopped, Duration::from_secs(10));
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

    if wait_for_state(&service, ServiceState::Running, Duration::from_secs(10))? {
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

    if wait_for_state(&service, ServiceState::Stopped, Duration::from_secs(10))? {
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
        let _ = wait_for_state(&service, ServiceState::Stopped, Duration::from_secs(10));
    }

    println!("Starting service '{}'...", cmd_name);
    service.start(&[] as &[&OsStr])?;

    if wait_for_state(&service, ServiceState::Running, Duration::from_secs(10))? {
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
        thread::sleep(Duration::from_millis(200));
        let status = service.query_status()?;
        if status.current_state == target {
            return Ok(true);
        }
    }
    Ok(false)
}
