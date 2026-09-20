// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use std::path::Path;
use tokio_util::sync::CancellationToken;

#[cfg(unix)]
pub fn verify_caller_credentials(
    stream: &tokio::net::UnixStream,
) -> Result<(), crate::error::ProgramError> {
    use nix::sys::socket::{getsockopt, sockopt::PeerCredentials};
    use nix::unistd::Uid;
    use std::os::fd::{AsRawFd, BorrowedFd};

    let borrowed_fd = unsafe { BorrowedFd::borrow_raw(stream.as_raw_fd()) };
    let creds = getsockopt(&borrowed_fd, PeerCredentials).map_err(|e| {
        crate::error::ProgramError::PlatformError(format!(
            "Failed to retrieve peer credentials: {}",
            e
        ))
    })?;
    let caller_uid = Uid::from_raw(creds.uid());
    let daemon_uid = nix::unistd::getuid();

    if daemon_uid.is_root() {
        if !caller_uid.is_root() {
            return Err(crate::error::ProgramError::PlatformError(format!(
                "Access denied: Caller UID {} is not root",
                caller_uid
            )));
        }
    } else if caller_uid != daemon_uid && !caller_uid.is_root() {
        return Err(crate::error::ProgramError::PlatformError(format!(
            "Access denied: Caller UID {} does not match daemon UID {}",
            caller_uid, daemon_uid
        )));
    }

    Ok(())
}

#[cfg(unix)]
pub async fn run_ipc_listener(
    path: &Path,
    app: axum::Router,
    cancel_token: CancellationToken,
) -> anyhow::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = tokio::fs::remove_file(path).await;

    let listener = tokio::net::UnixListener::bind(path)?;
    tracing::info!("Listening on Unix Domain Socket: {:?}", path);

    loop {
        tokio::select! {
            biased;

            _ = cancel_token.cancelled() => {
                break;
            }

            accept_res = listener.accept() => {
                match accept_res {
                    Ok((stream, _)) => {
                        if let Err(e) = verify_caller_credentials(&stream) {
                            tracing::warn!("Rejecting unauthorized UDS connection: {}", e);
                            continue;
                        }

                        let tower_service = app.clone();
                        tokio::spawn(async move {
                            let socket = hyper_util::rt::TokioIo::new(stream);
                            let hyper_service = hyper_util::service::TowerToHyperService::new(tower_service);
                            let _ = hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new())
                                .serve_connection_with_upgrades(socket, hyper_service)
                                .await;
                        });
                    }
                    Err(e) => {
                        tracing::error!("Error accepting UDS connection: {}", e);
                    }
                }
            }
        }
    }

    let _ = tokio::fs::remove_file(path).await;
    Ok(())
}

#[cfg(windows)]
pub async fn run_ipc_listener(
    path: &Path,
    app: axum::Router,
    cancel_token: CancellationToken,
) -> anyhow::Result<()> {
    if path.to_string_lossy().starts_with(r"\\.\pipe\") {
        run_named_pipe_listener(path, app, cancel_token).await
    } else {
        run_windows_uds_listener(path, app, cancel_token).await
    }
}

#[cfg(windows)]
async fn run_named_pipe_listener(
    path: &Path,
    app: axum::Router,
    cancel_token: CancellationToken,
) -> anyhow::Result<()> {
    let pipe_name = path.to_string_lossy().to_string();
    tracing::info!("Listening on Windows Named Pipe: {}", pipe_name);

    let mut is_first = true;
    loop {
        let server = match tokio::net::windows::named_pipe::ServerOptions::new()
            .first_pipe_instance(is_first)
            .create(&pipe_name)
        {
            Ok(s) => {
                is_first = false;
                s
            }
            Err(e) => {
                tracing::error!("Failed to create named pipe instance: {}", e);
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                continue;
            }
        };

        tokio::select! {
            biased;

            _ = cancel_token.cancelled() => {
                break;
            }

            connect_res = server.connect() => {
                match connect_res {
                    Ok(()) => {
                        let tower_service = app.clone();
                        tokio::spawn(async move {
                            let socket = hyper_util::rt::TokioIo::new(server);
                            let hyper_service = hyper_util::service::TowerToHyperService::new(tower_service);
                            let _ = hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new())
                                .serve_connection_with_upgrades(socket, hyper_service)
                                .await;
                        });
                    }
                    Err(e) => {
                        tracing::debug!("Named pipe client connection dropped or failed: {}", e);
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(windows)]
async fn run_windows_uds_listener(
    path: &Path,
    app: axum::Router,
    cancel_token: CancellationToken,
) -> anyhow::Result<()> {
    use std::os::windows::io::{FromRawSocket, IntoRawSocket};
    use uds_windows::{UnixListener as StdUnixListener, UnixStream as StdUnixStream};

    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::remove_file(path);

    let std_listener = StdUnixListener::bind(path)?;
    tracing::info!("Listening on Windows Unix Domain Socket: {:?}", path);

    let listener = std::sync::Arc::new(std_listener);
    let path_buf = path.to_path_buf();

    // Spawn wakeup task to unblock blocking accept() upon cancellation
    let cancel_watcher = cancel_token.clone();
    let wakeup_path = path_buf.clone();
    tokio::spawn(async move {
        cancel_watcher.cancelled().await;
        let _ = StdUnixStream::connect(&wakeup_path);
    });

    loop {
        if cancel_token.is_cancelled() {
            break;
        }

        let l = listener.clone();
        let accept_res = tokio::task::spawn_blocking(move || l.accept()).await;

        if cancel_token.is_cancelled() {
            break;
        }

        let (std_stream, _) = match accept_res {
            Ok(Ok(pair)) => pair,
            Ok(Err(e)) => {
                if !cancel_token.is_cancelled() {
                    tracing::error!("Error accepting Windows UDS connection: {}", e);
                }
                break;
            }
            Err(_) => break,
        };

        let raw = std_stream.into_raw_socket();
        let std_tcp = unsafe { std::net::TcpStream::from_raw_socket(raw) };
        if let Err(e) = std_tcp.set_nonblocking(true) {
            tracing::error!(
                "Failed to set nonblocking mode on Windows UDS socket: {}",
                e
            );
            continue;
        }
        let async_stream = match tokio::net::TcpStream::from_std(std_tcp) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(
                    "Failed to convert Windows UDS socket to Tokio stream: {}",
                    e
                );
                continue;
            }
        };

        let tower_service = app.clone();
        tokio::spawn(async move {
            let socket = hyper_util::rt::TokioIo::new(async_stream);
            let hyper_service = hyper_util::service::TowerToHyperService::new(tower_service);
            let _ =
                hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new())
                    .serve_connection_with_upgrades(socket, hyper_service)
                    .await;
        });
    }

    let _ = std::fs::remove_file(&path_buf);
    Ok(())
}
