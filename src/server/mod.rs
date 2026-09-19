pub mod api;
pub mod uds;

pub use api::{AppState, build_router};
pub use uds::run_ipc_listener;

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
        Self {
            state: AppState {
                manager,
                config_path,
                auth_token,
            },
            server_config,
        }
    }

    /// Spawns and manages all configured listeners until cancel_token is triggered.
    pub async fn run(self, cancel_token: CancellationToken) -> anyhow::Result<()> {
        let router = build_router(self.state);
        let mut set = tokio::task::JoinSet::new();

        // 1. Local IPC listener (UDS on Unix, Named Pipe on Windows)
        let ipc_path = self.server_config.uds_path.clone();
        let ipc_router = router.clone();
        let ipc_token = cancel_token.clone();
        set.spawn(async move {
            if let Err(e) = run_ipc_listener(&ipc_path, ipc_router, ipc_token).await {
                tracing::error!("run_ipc_listener failed on {:?}: {}", ipc_path, e);
            }
        });

        // 2. Optional TCP listener
        if let Some(ref bind_addr) = self.server_config.http_bind {
            let tcp_router = router.clone();
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

        while (set.join_next().await).is_some() {}

        Ok(())
    }
}
