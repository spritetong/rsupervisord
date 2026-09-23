// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use std::path::Path;
use tokio_util::sync::CancellationToken;

/// Local IPC server driver (Unix Domain Socket on Unix, Named Pipe or UDS on Windows).
pub struct IpcServer<'a> {
    path: &'a Path,
    router: axum::Router,
    allow_unelevated: bool,
    mode: u32,
}

impl<'a> IpcServer<'a> {
    /// Creates a new IpcServer bound to the target IPC path, router, and elevation policy.
    pub fn new(path: &'a Path, router: axum::Router, allow_unelevated: bool, mode: u32) -> Self {
        Self {
            path,
            router,
            allow_unelevated,
            mode,
        }
    }

    /// Serves incoming IPC connections until the cancellation token is triggered.
    pub async fn run(self, cancel_token: CancellationToken) -> anyhow::Result<()> {
        let mut listener = crate::platform::native_platform()
            .bind_ipc_listener(self.path, self.allow_unelevated, self.mode)
            .map_err(|e| {
                anyhow::anyhow!("Failed to bind IPC listener at {:?}: {}", self.path, e)
            })?;

        tracing::info!("Listening on local IPC: {:?}", self.path);

        loop {
            tokio::select! {
                biased;

                _ = cancel_token.cancelled() => {
                    break;
                }

                accept_res = listener.accept() => {
                    match accept_res {
                        Ok(stream) => {
                            let tower_service = self.router.clone();
                            let conn_token = cancel_token.clone();
                            tokio::spawn(async move {
                                let socket = hyper_util::rt::TokioIo::new(stream);
                                let hyper_service =
                                    hyper_util::service::TowerToHyperService::new(tower_service);
                                let builder = hyper_util::server::conn::auto::Builder::new(
                                    hyper_util::rt::TokioExecutor::new(),
                                );
                                tokio::select! {
                                    biased;
                                    _ = conn_token.cancelled() => {},
                                    _ = builder.serve_connection_with_upgrades(socket, hyper_service) => {},
                                }
                            });
                        }
                        Err(e) => {
                            if !cancel_token.is_cancelled() {
                                tracing::error!("Error accepting IPC connection: {}", e);
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

/// Spawns and manages the local IPC listener (Unix Domain Socket on Unix, Named Pipe or UDS on Windows).
#[inline]
pub async fn run_ipc_listener(
    path: &Path,
    app: axum::Router,
    cancel_token: CancellationToken,
    allow_unelevated: bool,
    mode: u32,
) -> anyhow::Result<()> {
    IpcServer::new(path, app, allow_unelevated, mode)
        .run(cancel_token)
        .await
}
