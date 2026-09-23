// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

pub mod api;
pub mod auth;
pub mod uds;
pub mod web;

pub use api::{AppState, build_router};
pub use auth::{BasicAuthConfig, ServerAuthState, SessionStore, http_auth_middleware};
pub use uds::run_ipc_listener;
pub use web::WebAssets;

use crate::config::schema::ServerConfig;
use crate::manager::ManagerHandle;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

/// Orchestrates local IPC and optional TCP listeners for the supervisord daemon.
pub struct ServerEngine {
    ipc_state: AppState,
    tcp_state: AppState,
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
        let uds_basic_auth = BasicAuthConfig::new(
            server_config.uds_username.clone(),
            server_config.uds_password.clone(),
        );
        let inet_basic_auth = BasicAuthConfig::new(
            server_config.username.clone(),
            server_config.password.clone(),
        );
        let sessions = std::sync::Arc::new(SessionStore::default());

        Self {
            ipc_state: AppState {
                manager: manager.clone(),
                config_path: config_path.clone(),
                auth_token: auth_token.clone(),
                basic_auth: uds_basic_auth,
                sessions: sessions.clone(),
            },
            tcp_state: AppState {
                manager,
                config_path,
                auth_token,
                basic_auth: inet_basic_auth,
                sessions,
            },
            server_config,
        }
    }

    /// Spawns and manages all configured listeners until cancel_token is triggered.
    pub async fn run(self, cancel_token: CancellationToken) -> anyhow::Result<()> {
        let ipc_router = self.ipc_state.clone().into_router();
        let mut set = tokio::task::JoinSet::new();

        // 1. Local IPC listener (UDS on Unix, Named Pipe on Windows)
        let ipc_path = self.server_config.uds_path.clone();
        let allow_unelevated = self.server_config.allow_unelevated;
        let ipc_mode = self.server_config.resolved_uds_chmod()?;
        if !ipc_path.as_os_str().is_empty() {
            let ipc_router = ipc_router.clone();
            let ipc_token = cancel_token.clone();
            set.spawn(async move {
                if let Err(e) =
                    run_ipc_listener(&ipc_path, ipc_router, ipc_token, allow_unelevated, ipc_mode)
                        .await
                {
                    tracing::error!("run_ipc_listener failed on {:?}: {}", ipc_path, e);
                }
            });
        }

        // 2. Optional TCP listener (auth middleware is applied inside build_router)
        if let Some(ref bind_addr) = self.server_config.http_bind {
            let tcp_router = self.tcp_state.clone().into_router();
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
        let drain_timeout = crate::consts::DRAIN_TIMEOUT;
        let _ = tokio::time::timeout(drain_timeout, async {
            while (set.join_next().await).is_some() {}
        })
        .await;

        Ok(())
    }
}
