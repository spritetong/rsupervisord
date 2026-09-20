// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use std::path::Path;
use tokio_util::sync::CancellationToken;

/// Spawns and manages the local IPC listener (Unix Domain Socket on Unix, Named Pipe or UDS on Windows).
pub async fn run_ipc_listener(
    path: &Path,
    app: axum::Router,
    cancel_token: CancellationToken,
) -> anyhow::Result<()> {
    let mut listener = crate::platform::native_platform()
        .bind_ipc_listener(path)
        .map_err(|e| anyhow::anyhow!("Failed to bind IPC listener at {:?}: {}", path, e))?;

    tracing::info!("Listening on local IPC: {:?}", path);

    loop {
        tokio::select! {
            biased;

            _ = cancel_token.cancelled() => {
                break;
            }

            accept_res = listener.accept() => {
                match accept_res {
                    Ok(stream) => {
                        let tower_service = app.clone();
                        tokio::spawn(async move {
                            let socket = hyper_util::rt::TokioIo::new(stream);
                            let hyper_service =
                                hyper_util::service::TowerToHyperService::new(tower_service);
                            let _ = hyper_util::server::conn::auto::Builder::new(
                                hyper_util::rt::TokioExecutor::new(),
                            )
                            .serve_connection_with_upgrades(socket, hyper_service)
                            .await;
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
