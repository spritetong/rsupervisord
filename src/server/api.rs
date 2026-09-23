// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::config::SupervisorConfig;
use crate::consts::*;
use crate::control::protocol::{
    ActionResponse, ApiResponse, LogLinesResponse, ProgramDetailsDto, ProgramStatusDto,
    ReloadResponse,
};
use crate::error::ProgramError;
use crate::manager::ManagerHandle;
use crate::program::state::ProgramState;
use crate::server::auth::{
    AuthAttempts, ServerAuthState, SessionStore, clear_session_cookie, http_auth_middleware,
    issue_session_cookie, session_id_from_headers,
};
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, Uri};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

/// Shared application state injected into Axum route handlers.
#[derive(Clone)]
pub struct AppState {
    pub manager: ManagerHandle,
    pub config_path: Option<PathBuf>,
    pub auth_token: Option<String>,
    pub basic_auth: Option<crate::server::auth::BasicAuthConfig>,
    pub sessions: Arc<SessionStore>,
}

impl AppState {
    pub fn new(
        manager: ManagerHandle,
        config_path: Option<PathBuf>,
        auth_token: Option<String>,
        basic_auth: Option<crate::server::auth::BasicAuthConfig>,
    ) -> Self {
        Self {
            manager,
            config_path,
            auth_token,
            basic_auth,
            sessions: Arc::new(SessionStore::default()),
        }
    }

    /// Converts this AppState into a fully configured Axum router.
    pub fn into_router(self) -> Router {
        build_router(self)
    }
}

#[derive(Debug, Deserialize)]
pub struct ActionQuery {
    #[serde(default = "bool_value::<true>")]
    pub sync: bool,
    #[serde(default = "default_action_timeout_secs")]
    pub timeout: u64,
}

#[derive(Debug, Deserialize)]
pub struct LogsQuery {
    #[serde(default = "default_log_lines")]
    pub lines: usize,
}

/// Builds the complete Axum router with all v1 REST API endpoints and embedded Web UI.
///
/// The unified auth middleware guards `/api/v1/*` (except the public
/// `/api/v1/auth/*` surface) and `/RPC2` on every listener, while the static
/// shell stays public so the login page can load without a browser native dialog.
pub fn build_router(state: AppState) -> Router {
    let auth_state = ServerAuthState::new(
        state.basic_auth.clone(),
        state.auth_token.clone(),
        state.sessions.clone(),
    );
    Router::new()
        .route("/api/v1/status", get(get_status))
        .route("/api/v1/programs/{name}", get(get_program_details))
        .route("/api/v1/programs/{name}/start", post(start_program))
        .route("/api/v1/programs/{name}/stop", post(stop_program))
        .route("/api/v1/programs/{name}/restart", post(restart_program))
        .route("/api/v1/groups/{group}/start", post(start_group))
        .route("/api/v1/groups/{group}/stop", post(stop_group))
        .route("/api/v1/groups/{group}/restart", post(restart_group))
        .route("/api/v1/all/start", post(start_all))
        .route("/api/v1/all/stop", post(stop_all))
        .route("/api/v1/config/reload", post(reload_config))
        .route("/api/v1/config-reload", post(reload_config))
        .route("/api/v1/reload", post(restart_daemon))
        .route("/api/v1/restart", post(restart_daemon))
        .route("/api/v1/events", get(stream_system_events))
        .route("/api/v1/logs/stream", get(stream_all_logs))
        .route("/api/v1/programs/{name}/logs", get(read_logs))
        .route("/api/v1/programs/{name}/logs/stream", get(stream_logs))
        .route("/api/v1/programs/{name}/stdin", post(send_stdin))
        .route("/api/v1/auth/config", get(auth_config))
        .route("/api/v1/auth/login", post(auth_login))
        .route("/api/v1/auth/logout", post(auth_logout))
        .route("/RPC2", post(crate::compat::xmlrpc::xmlrpc_handler))
        .fallback(crate::server::web::static_handler)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            record_activity_middleware,
        ))
        .layer(axum::middleware::from_fn_with_state(
            auth_state,
            http_auth_middleware,
        ))
        .with_state(state)
}

/// Middleware that updates the daemon's client activity tracker on every incoming request.
async fn record_activity_middleware(
    axum::extract::State(state): axum::extract::State<AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    state.manager.activity_tracker().record_activity();
    next.run(req).await
}

/// Auth capability flags reported to the Web UI before login.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfigDto {
    pub basic: bool,
    pub token: bool,
    pub session: bool,
}

/// JSON body accepted by `POST /api/v1/auth/login`.
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: Option<String>,
    pub password: Option<String>,
    pub token: Option<String>,
}

/// GET /api/v1/auth/config — public; tells the UI which login fields to show
/// and whether the current session cookie is already valid.
async fn auth_config(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Json<ApiResponse<AuthConfigDto>> {
    let auth = ServerAuthState::new(
        state.basic_auth.clone(),
        state.auth_token.clone(),
        state.sessions.clone(),
    );
    let session = session_id_from_headers(&headers)
        .map(|sid| state.sessions.validate_session(&sid))
        .unwrap_or(false);
    Json(ApiResponse::ok(AuthConfigDto {
        basic: state.basic_auth.is_some(),
        token: auth.auth_token.is_some(),
        session,
    }))
}

/// POST /api/v1/auth/login — public; validates credentials via the unified
/// OR authorize path and issues an HttpOnly session cookie on success.
async fn auth_login(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    uri: Uri,
    Json(payload): Json<LoginRequest>,
) -> Response {
    let auth = ServerAuthState::new(
        state.basic_auth.clone(),
        state.auth_token.clone(),
        state.sessions.clone(),
    );

    let attempts = AuthAttempts {
        basic: match (payload.username, payload.password) {
            (Some(u), Some(p)) => Some((u, p)),
            _ => None,
        },
        token: payload.token.filter(|t| !t.is_empty()),
        session: None,
    };

    if !auth.authorize(&attempts) {
        // Lightweight delay to slow online brute-force attempts.
        tokio::time::sleep(Duration::from_millis(300)).await;
        return (
            StatusCode::UNAUTHORIZED,
            Json(ApiResponse::<()>::err("Unauthorized: Invalid credentials")),
        )
            .into_response();
    }

    // When nothing is configured the middleware already allows everything;
    // still issue a session so the UI can treat login as a no-op success.
    match state.sessions.create_session() {
        Some(sid) => {
            let cookie = issue_session_cookie(&sid, &headers, &uri);
            (
                StatusCode::OK,
                [cookie],
                Json(ApiResponse::ok(serde_json::json!({ "session": true }))),
            )
                .into_response()
        }
        None => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::err("Failed to create session")),
        )
            .into_response(),
    }
}

/// POST /api/v1/auth/logout — public; drops the session and clears the cookie.
async fn auth_logout(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(sid) = session_id_from_headers(&headers) {
        state.sessions.invalidate_session(&sid);
    }
    let cookie = clear_session_cookie(&headers, &uri);
    (
        StatusCode::OK,
        [cookie],
        Json(ApiResponse::ok(serde_json::json!({ "session": false }))),
    )
        .into_response()
}

/// GET /api/v1/status
async fn get_status(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Vec<ProgramStatusDto>>>, StatusCode> {
    let statuses = state
        .manager
        .get_all_status()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let dtos = statuses
        .into_iter()
        .map(|s| {
            let cpu = s
                .metrics
                .map(|m| format!("{:.1}%", m.cpu_percent))
                .unwrap_or_else(|| "-".to_string());
            let mem = s
                .metrics
                .map(|m| format!("{:.1} MB", m.memory_rss_bytes as f64 / 1024.0 / 1024.0))
                .unwrap_or_else(|| "-".to_string());
            let uptime = s
                .uptime_secs
                .map(crate::serde_util::duration_secs_to_string)
                .unwrap_or_else(|| "-".to_string());

            ProgramStatusDto {
                name: s.name,
                group: s.group,
                state: format!("{:?}", s.state).to_uppercase(),
                health: s.health.to_string(),
                pid: s
                    .pid
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                cpu,
                mem,
                uptime,
                cron: s.cron.unwrap_or_else(|| "-".to_string()),
                description: s.description,
            }
        })
        .collect();

    Ok(Json(ApiResponse::ok(dtos)))
}

/// GET /api/v1/programs/:name
async fn get_program_details(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<(StatusCode, Json<ApiResponse<ProgramDetailsDto>>), StatusCode> {
    let status = match state.manager.get_status(&name).await {
        Ok(s) => s,
        Err(e) => {
            let status_code = match &e {
                ProgramError::NotFound { .. } => StatusCode::NOT_FOUND,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            return Ok((status_code, Json(ApiResponse::err(format!("{}", e)))));
        }
    };

    let (cpu_percent, memory_rss_bytes) = match status.metrics {
        Some(m) => (Some(m.cpu_percent), Some(m.memory_rss_bytes)),
        None => (None, None),
    };

    let (pre_start, pre_stop) = if let Some(ref path) = state.config_path {
        if let Ok(cfg) = SupervisorConfig::from_file(path) {
            let prog_cfg = cfg.programs.get(&status.name);
            (
                prog_cfg.and_then(|p| p.pre_start.clone()),
                prog_cfg.and_then(|p| p.pre_stop.clone()),
            )
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    let dto = ProgramDetailsDto {
        name: status.name,
        group: status.group,
        state: status.state,
        health: status.health.to_string(),
        pid: status.pid,
        uptime_secs: status.uptime_secs,
        exit_code: status.exit_code,
        cpu_percent,
        memory_rss_bytes,
        description: status.description,
        cron: status.cron,
        next_cron_run: status.next_cron_run,
        pre_start,
        pre_stop,
    };

    Ok((StatusCode::OK, Json(ApiResponse::ok(dto))))
}

/// POST /api/v1/programs/:name/start
async fn start_program(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<ActionQuery>,
) -> Result<(StatusCode, Json<ApiResponse<ActionResponse>>), StatusCode> {
    let start_time = Instant::now();
    if let Err(e) = state.manager.start_program(&name).await {
        let status_code = match &e {
            ProgramError::NotFound { .. } => StatusCode::NOT_FOUND,
            _ => StatusCode::BAD_REQUEST,
        };
        return Ok((
            status_code,
            Json(ApiResponse::err(format!("Start failed: {}", e))),
        ));
    }

    if !query.sync {
        return Ok((
            StatusCode::ACCEPTED,
            Json(ApiResponse::ok(ActionResponse {
                name,
                state: ProgramState::Starting,
                pid: None,
                description: "Start requested".to_string(),
                elapsed_ms: start_time.elapsed().as_millis() as u64,
            })),
        ));
    }

    // Synchronous mode: wait reactively until status settles into Running, Fatal, or Exited
    let timeout_secs = query.timeout.min(86400);
    let timeout_dur = Duration::from_secs(timeout_secs);
    let mut rx = state.manager.subscribe_events();

    // Check if process settled immediately
    if let Ok(st) = state.manager.get_status(&name).await
        && (st.state == ProgramState::Running
            || st.state == ProgramState::Fatal
            || st.state == ProgramState::Exited)
    {
        return Ok((
            StatusCode::OK,
            Json(ApiResponse::ok(ActionResponse {
                name,
                state: st.state,
                pid: st.pid,
                description: st.description,
                elapsed_ms: start_time.elapsed().as_millis() as u64,
            })),
        ));
    }

    // Reactive event wait without busy-polling
    let _ = tokio::time::timeout(timeout_dur, async {
        loop {
            match rx.recv().await {
                Ok(crate::manager::SystemEvent::StateChanged {
                    name: ref ev_name,
                    new_state,
                    ..
                }) if ev_name == &name
                    && (new_state == ProgramState::Running
                        || new_state == ProgramState::Fatal
                        || new_state == ProgramState::Exited) =>
                {
                    break;
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    if let Ok(st) = state.manager.get_status(&name).await
                        && (st.state == ProgramState::Running
                            || st.state == ProgramState::Fatal
                            || st.state == ProgramState::Exited)
                    {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    })
    .await;

    // Return current state upon settling or timeout
    let final_status = match state.manager.get_status(&name).await {
        Ok(st) => st,
        Err(e) => {
            return Ok((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::err(format!(
                    "Failed to retrieve status: {}",
                    e
                ))),
            ));
        }
    };

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(ActionResponse {
            name,
            state: final_status.state,
            pid: final_status.pid,
            description: final_status.description,
            elapsed_ms: start_time.elapsed().as_millis() as u64,
        })),
    ))
}

/// POST /api/v1/programs/:name/stop
async fn stop_program(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<ActionQuery>,
) -> Result<(StatusCode, Json<ApiResponse<ActionResponse>>), StatusCode> {
    let start_time = Instant::now();
    let timeout_secs = query.timeout.min(86400);
    let grace = Some(Duration::from_secs(timeout_secs));

    // Verify program exists before accepting async or sync request
    if let Err(e) = state.manager.get_status(&name).await {
        let status_code = match &e {
            ProgramError::NotFound { .. } => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        return Ok((
            status_code,
            Json(ApiResponse::err(format!("Stop failed: {}", e))),
        ));
    }

    if !query.sync {
        let mgr = state.manager.clone();
        let name_clone = name.clone();
        let cancel = mgr.cancel_token();
        tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {},
                res = mgr.stop_program(&name_clone, grace) => {
                    if let Err(e) = res {
                        tracing::warn!("Async stop for '{}' failed or timed out: {}", name_clone, e);
                    }
                }
            }
        });

        return Ok((
            StatusCode::ACCEPTED,
            Json(ApiResponse::ok(ActionResponse {
                name,
                state: ProgramState::Stopping,
                pid: None,
                description: "Stop requested".to_string(),
                elapsed_ms: start_time.elapsed().as_millis() as u64,
            })),
        ));
    }

    if let Err(e) = state.manager.stop_program(&name, grace).await {
        let status_code = match &e {
            ProgramError::NotFound { .. } => StatusCode::NOT_FOUND,
            _ => StatusCode::BAD_REQUEST,
        };
        return Ok((
            status_code,
            Json(ApiResponse::err(format!("Stop failed: {}", e))),
        ));
    }

    let final_status = match state.manager.get_status(&name).await {
        Ok(st) => st,
        Err(e) => {
            return Ok((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::err(format!(
                    "Failed to retrieve status: {}",
                    e
                ))),
            ));
        }
    };

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(ActionResponse {
            name,
            state: final_status.state,
            pid: final_status.pid,
            description: final_status.description,
            elapsed_ms: start_time.elapsed().as_millis() as u64,
        })),
    ))
}

/// POST /api/v1/programs/:name/restart
async fn restart_program(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<ActionQuery>,
) -> Result<(StatusCode, Json<ApiResponse<ActionResponse>>), StatusCode> {
    let start_time = Instant::now();
    let timeout_secs = query.timeout.min(86400);
    let grace = Some(Duration::from_secs(timeout_secs));

    // Verify program exists before accepting async or sync request
    if let Err(e) = state.manager.get_status(&name).await {
        let status_code = match &e {
            ProgramError::NotFound { .. } => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        return Ok((
            status_code,
            Json(ApiResponse::err(format!("Restart failed: {}", e))),
        ));
    }

    if !query.sync {
        let mgr = state.manager.clone();
        let name_clone = name.clone();
        let cancel = mgr.cancel_token();
        tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {},
                res = mgr.restart_program(&name_clone, grace) => {
                    if let Err(e) = res {
                        tracing::warn!(
                            "Async restart for '{}' failed or timed out: {}",
                            name_clone,
                            e
                        );
                    }
                }
            }
        });

        return Ok((
            StatusCode::ACCEPTED,
            Json(ApiResponse::ok(ActionResponse {
                name,
                state: ProgramState::Starting,
                pid: None,
                description: "Restart requested".to_string(),
                elapsed_ms: start_time.elapsed().as_millis() as u64,
            })),
        ));
    }

    if let Err(e) = state.manager.restart_program(&name, grace).await {
        let status_code = match &e {
            ProgramError::NotFound { .. } => StatusCode::NOT_FOUND,
            _ => StatusCode::BAD_REQUEST,
        };
        return Ok((
            status_code,
            Json(ApiResponse::err(format!("Restart failed: {}", e))),
        ));
    }

    let final_status = match state.manager.get_status(&name).await {
        Ok(st) => st,
        Err(e) => {
            return Ok((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::err(format!(
                    "Failed to retrieve status: {}",
                    e
                ))),
            ));
        }
    };

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(ActionResponse {
            name,
            state: final_status.state,
            pid: final_status.pid,
            description: final_status.description,
            elapsed_ms: start_time.elapsed().as_millis() as u64,
        })),
    ))
}

/// POST /api/v1/groups/:group/start
async fn start_group(
    State(state): State<AppState>,
    Path(group): Path<String>,
    Query(_query): Query<ActionQuery>,
) -> Result<(StatusCode, Json<ApiResponse<Vec<ActionResponse>>>), StatusCode> {
    let start_time = Instant::now();
    let procs = match state.manager.start_group(&group).await {
        Ok(p) => p,
        Err(e) => {
            let status_code = match &e {
                ProgramError::NotFound { .. } => StatusCode::NOT_FOUND,
                _ => StatusCode::BAD_REQUEST,
            };
            return Ok((
                status_code,
                Json(ApiResponse::err(format!("Start group failed: {}", e))),
            ));
        }
    };
    let mut responses = Vec::new();
    for name in procs {
        let (p_state, pid, desc) = if let Ok(st) = state.manager.get_status(&name).await {
            (st.state, st.pid, st.description)
        } else {
            (ProgramState::Starting, None, "Start requested".to_string())
        };
        responses.push(ActionResponse {
            name,
            state: p_state,
            pid,
            description: desc,
            elapsed_ms: start_time.elapsed().as_millis() as u64,
        });
    }
    Ok((StatusCode::OK, Json(ApiResponse::ok(responses))))
}

/// POST /api/v1/groups/:group/stop
async fn stop_group(
    State(state): State<AppState>,
    Path(group): Path<String>,
    Query(_query): Query<ActionQuery>,
) -> Result<(StatusCode, Json<ApiResponse<Vec<ActionResponse>>>), StatusCode> {
    let start_time = Instant::now();
    let procs = match state.manager.stop_group(&group, None).await {
        Ok(p) => p,
        Err(e) => {
            let status_code = match &e {
                ProgramError::NotFound { .. } => StatusCode::NOT_FOUND,
                _ => StatusCode::BAD_REQUEST,
            };
            return Ok((
                status_code,
                Json(ApiResponse::err(format!("Stop group failed: {}", e))),
            ));
        }
    };
    let mut responses = Vec::new();
    for name in procs {
        let (p_state, pid, desc) = if let Ok(st) = state.manager.get_status(&name).await {
            (st.state, st.pid, st.description)
        } else {
            (ProgramState::Stopped, None, "Stop requested".to_string())
        };
        responses.push(ActionResponse {
            name,
            state: p_state,
            pid,
            description: desc,
            elapsed_ms: start_time.elapsed().as_millis() as u64,
        });
    }
    Ok((StatusCode::OK, Json(ApiResponse::ok(responses))))
}

/// POST /api/v1/groups/:group/restart
async fn restart_group(
    State(state): State<AppState>,
    Path(group): Path<String>,
    Query(_query): Query<ActionQuery>,
) -> Result<(StatusCode, Json<ApiResponse<Vec<ActionResponse>>>), StatusCode> {
    let start_time = Instant::now();
    let procs = match state.manager.restart_group(&group, None).await {
        Ok(p) => p,
        Err(e) => {
            let status_code = match &e {
                ProgramError::NotFound { .. } => StatusCode::NOT_FOUND,
                _ => StatusCode::BAD_REQUEST,
            };
            return Ok((
                status_code,
                Json(ApiResponse::err(format!("Restart group failed: {}", e))),
            ));
        }
    };
    let mut responses = Vec::new();
    for name in procs {
        let (p_state, pid, desc) = if let Ok(st) = state.manager.get_status(&name).await {
            (st.state, st.pid, st.description)
        } else {
            (
                ProgramState::Starting,
                None,
                "Restart requested".to_string(),
            )
        };
        responses.push(ActionResponse {
            name,
            state: p_state,
            pid,
            description: desc,
            elapsed_ms: start_time.elapsed().as_millis() as u64,
        });
    }
    Ok((StatusCode::OK, Json(ApiResponse::ok(responses))))
}

/// POST /api/v1/all/start
async fn start_all(
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<ApiResponse<Vec<ActionResponse>>>), StatusCode> {
    let start_time = Instant::now();
    if let Err(e) = state.manager.start_all().await {
        return Ok((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::err(format!("Start all failed: {}", e))),
        ));
    }

    let statuses = state.manager.get_all_status().await.unwrap_or_default();

    let responses = statuses
        .into_iter()
        .map(|s| ActionResponse {
            name: s.name,
            state: s.state,
            pid: s.pid,
            description: s.description,
            elapsed_ms: start_time.elapsed().as_millis() as u64,
        })
        .collect();

    Ok((StatusCode::OK, Json(ApiResponse::ok(responses))))
}

/// POST /api/v1/all/stop
async fn stop_all(
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<ApiResponse<Vec<ActionResponse>>>), StatusCode> {
    let start_time = Instant::now();
    if let Err(e) = state.manager.stop_all(None).await {
        return Ok((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::err(format!("Stop all failed: {}", e))),
        ));
    }

    let statuses = state.manager.get_all_status().await.unwrap_or_default();

    let responses = statuses
        .into_iter()
        .map(|s| ActionResponse {
            name: s.name,
            state: s.state,
            pid: s.pid,
            description: s.description,
            elapsed_ms: start_time.elapsed().as_millis() as u64,
        })
        .collect();

    Ok((StatusCode::OK, Json(ApiResponse::ok(responses))))
}

/// POST /api/v1/reload
async fn reload_config(
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<ApiResponse<ReloadResponse>>), StatusCode> {
    let config_path = match state.config_path {
        Some(ref p) => p.clone(),
        None => {
            return Ok((
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::err(
                    "No configuration file path specified for reload",
                )),
            ));
        }
    };

    // Dispatch on the file extension so `.conf`/`.ini` reloads go through the
    // compatibility INI pipeline (mirrors the initial load in main.rs).
    let new_config = match SupervisorConfig::from_file(&config_path) {
        Ok(c) => c,
        Err(e) => {
            return Ok((
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::err(format!("Invalid configuration: {}", e))),
            ));
        }
    };

    match state.manager.reload_config(new_config).await {
        Ok(summary) => Ok((
            StatusCode::OK,
            Json(ApiResponse::ok(ReloadResponse {
                added: summary.added,
                removed: summary.removed,
                modified: summary.modified,
                unchanged: summary.unchanged,
            })),
        )),
        Err(e) => Ok((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::err(format!("Hot reload failed: {}", e))),
        )),
    }
}

/// POST /api/v1/reload or POST /api/v1/restart
async fn restart_daemon(
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<ApiResponse<String>>), StatusCode> {
    let config_path = match state.config_path {
        Some(ref p) => p.clone(),
        None => {
            return Ok((
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::err(
                    "No configuration file path specified for reload",
                )),
            ));
        }
    };

    let new_config = match SupervisorConfig::from_file(&config_path) {
        Ok(c) => c,
        Err(e) => {
            return Ok((
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::err(format!("Invalid configuration: {}", e))),
            ));
        }
    };

    match state.manager.restart_daemon(new_config).await {
        Ok(()) => Ok((
            StatusCode::OK,
            Json(ApiResponse::ok("Daemon restarted successfully".to_string())),
        )),
        Err(e) => Ok((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::err(format!("Daemon restart failed: {}", e))),
        )),
    }
}

/// GET /api/v1/programs/:name/logs
async fn read_logs(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<LogsQuery>,
) -> Result<(StatusCode, Json<ApiResponse<LogLinesResponse>>), StatusCode> {
    match state.manager.read_logs(&name, Some(query.lines)).await {
        Ok(lines) => Ok((
            StatusCode::OK,
            Json(ApiResponse::ok(LogLinesResponse { name, lines })),
        )),
        Err(e) => {
            let status = match &e {
                ProgramError::NotFound { .. } => StatusCode::NOT_FOUND,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            Ok((
                status,
                Json(ApiResponse::err(format!("Failed to read logs: {}", e))),
            ))
        }
    }
}

/// GET /api/v1/programs/:name/logs/stream
async fn stream_logs(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>>, StatusCode> {
    let rx = state
        .manager
        .subscribe_logs(&name)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let stream_guard = state.manager.activity_tracker().enter_stream();
    let stream = BroadcastStream::new(rx).filter_map(move |item| {
        let _guard = &stream_guard;
        match item {
            Ok(line) => Some(Ok(Event::default().data(line))),
            Err(_) => None,
        }
    });

    Ok(Sse::new(stream))
}

/// GET /api/v1/events
async fn stream_system_events(
    State(state): State<AppState>,
) -> Result<Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>>, StatusCode> {
    let rx = state.manager.subscribe_events();
    let stream_guard = state.manager.activity_tracker().enter_stream();
    let stream = BroadcastStream::new(rx).filter_map(move |item| {
        let _guard = &stream_guard;
        match item {
            Ok(evt) => {
                let json = serde_json::to_string(&evt).unwrap_or_default();
                Some(Ok(Event::default().data(json)))
            }
            Err(_) => None,
        }
    });

    Ok(Sse::new(stream)
        .keep_alive(axum::response::sse::KeepAlive::default().interval(SSE_KEEPALIVE)))
}

/// GET /api/v1/logs/stream
async fn stream_all_logs(
    State(state): State<AppState>,
) -> Result<Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>>, StatusCode> {
    let rx = state.manager.subscribe_all_logs();
    let stream_guard = state.manager.activity_tracker().enter_stream();
    let stream = BroadcastStream::new(rx).filter_map(move |item| {
        let _guard = &stream_guard;
        match item {
            Ok(entry) => {
                let json = serde_json::to_string(&entry).unwrap_or_default();
                Some(Ok(Event::default().data(json)))
            }
            Err(_) => None,
        }
    });

    Ok(Sse::new(stream)
        .keep_alive(axum::response::sse::KeepAlive::default().interval(SSE_KEEPALIVE)))
}

#[derive(Debug, Deserialize)]
pub struct SendStdinPayload {
    pub chars: Option<String>,
    pub data: Option<String>,
}

/// POST /api/v1/programs/{name}/stdin
async fn send_stdin(
    Path(name): Path<String>,
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> Result<(StatusCode, Json<ApiResponse<serde_json::Value>>), StatusCode> {
    let input_bytes = if let Ok(payload) = serde_json::from_slice::<SendStdinPayload>(&body) {
        if let Some(c) = payload.chars {
            c.into_bytes()
        } else if let Some(d) = payload.data {
            d.into_bytes()
        } else {
            body.to_vec()
        }
    } else {
        body.to_vec()
    };

    match state.manager.send_stdin(&name, input_bytes).await {
        Ok(()) => Ok((
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "success": true }))),
        )),
        Err(e) => {
            let status_code = match &e {
                ProgramError::NotFound { .. } => StatusCode::NOT_FOUND,
                ProgramError::NotRunning { .. } => StatusCode::BAD_REQUEST,
                ProgramError::StdinWriteTimeout { .. } => StatusCode::GATEWAY_TIMEOUT,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            Ok((status_code, Json(ApiResponse::err(e.to_string()))))
        }
    }
}
