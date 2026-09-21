// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

pub mod api;
pub mod auth;
pub mod uds;
pub mod web;

pub use api::{AppState, build_router};
pub use auth::{BasicAuthConfig, ServerAuthState, inet_http_auth_middleware};
pub use uds::run_ipc_listener;
pub use web::WebAssets;

use crate::config::schema::ServerConfig;
use crate::manager::ManagerHandle;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

/// Orchestrates local IPC and optional TCP listeners for the rsupervisord daemon.
pub struct ServerEngine {
    state: AppState,
    server_config: ServerConfig,
}

impl ServerEngine {
    /// Creates a new ServerEngine instance.
    pub fn new(
        manager: ManagerHandle,
        config_path: Option<PathBuf>,
        server_config: ServerConfig,
    ) -> Self {
        let auth_token = server_config.auth_token.clone();
        let basic_auth = BasicAuthConfig::new(
            server_config.username.clone(),
            server_config.password.clone(),
        );
        Self {
            state: AppState {
                manager,
                config_path,
                auth_token,
                basic_auth,
            },
            server_config,
        }
    }

    /// Spawns and manages all configured listeners until cancel_token is triggered.
    pub async fn run(self, cancel_token: CancellationToken) -> anyhow::Result<()> {
        let router = build_router(self.state.clone());
        let mut set = tokio::task::JoinSet::new();

        // 1. Local IPC listener (UDS on Unix, Named Pipe on Windows)
        let ipc_path = self.server_config.uds_path.clone();
        if !ipc_path.as_os_str().is_empty() {
            let ipc_router = router.clone();
            let ipc_token = cancel_token.clone();
            set.spawn(async move {
                if let Err(e) = run_ipc_listener(&ipc_path, ipc_router, ipc_token).await {
                    tracing::error!("run_ipc_listener failed on {:?}: {}", ipc_path, e);
                }
            });
        }

        // 2. Optional TCP listener
        if let Some(ref bind_addr) = self.server_config.http_bind {
            let auth_state = ServerAuthState {
                basic_auth: self.state.basic_auth.clone(),
                auth_token: self.state.auth_token.clone(),
            };
            let tcp_router = router.clone().layer(axum::middleware::from_fn_with_state(
                auth_state,
                inet_http_auth_middleware,
            ));
            let tcp_token = cancel_token.clone();
            let addr = bind_addr.clone();
            set.spawn(async move {
                match tokio::net::TcpListener::bind(&addr).await {
                    Ok(listener) => {
                        tracing::info!("Listening on TCP HTTP: {}", addr);
                        let _ = axum::serve(listener, tcp_router)
                            .with_graceful_shutdown(async move {
                                tcp_token.cancelled().await;
                            })
                            .await;
                    }
                    Err(e) => {
                        tracing::error!("Failed to bind TCP listener on {}: {}", addr, e);
                    }
                }
            });
        }

        tokio::select! {
            biased;

            _ = cancel_token.cancelled() => {
                tracing::info!("Server engine shutdown triggered");
            }
            _ = set.join_next() => {}
        }

        // Immediately abort remaining listener tasks so blocking accepts don't stall daemon teardown
        set.abort_all();

        // Bounded drain window for gracefully completing in-flight handlers
        let drain_timeout = std::time::Duration::from_secs(2);
        let _ = tokio::time::timeout(drain_timeout, async {
            while (set.join_next().await).is_some() {}
        })
        .await;

        Ok(())
    }
}
