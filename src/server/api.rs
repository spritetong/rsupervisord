// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use crate::config::SupervisorConfig;
use crate::control::protocol::{
    ActionResponse, ApiResponse, LogLinesResponse, ProgramDetailsDto, ProgramStatusDto,
    ReloadResponse,
};
use crate::manager::ManagerHandle;
use crate::program::state::ProgramState;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use std::convert::Infallible;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

/// Shared application state injected into Axum route handlers.
#[derive(Clone)]
pub struct AppState {
    pub manager: ManagerHandle,
    pub config_path: Option<PathBuf>,
    pub auth_token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ActionQuery {
    #[serde(default = "default_true")]
    pub sync: bool,
    #[serde(default = "default_timeout")]
    pub timeout: u64,
}

fn default_true() -> bool {
    true
}

fn default_timeout() -> u64 {
    30
}

#[derive(Debug, Deserialize)]
pub struct LogsQuery {
    #[serde(default = "default_log_lines")]
    pub lines: usize,
}

fn default_log_lines() -> usize {
    100
}

/// Builds the complete Axum router with all v1 REST API endpoints and embedded Web UI.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/status", get(get_status))
        .route("/api/v1/programs/{name}", get(get_program_details))
        .route("/api/v1/programs/{name}/start", post(start_program))
        .route("/api/v1/programs/{name}/stop", post(stop_program))
        .route("/api/v1/programs/{name}/restart", post(restart_program))
        .route("/api/v1/all/start", post(start_all))
        .route("/api/v1/all/stop", post(stop_all))
        .route("/api/v1/reload", post(reload_config))
        .route("/api/v1/programs/{name}/logs", get(read_logs))
        .route("/api/v1/programs/{name}/logs/stream", get(stream_logs))
        .fallback(crate::server::web::static_handler)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            record_activity_middleware,
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

/// Validates optional bearer token authentication.
fn check_auth(headers: &HeaderMap, auth_token: &Option<String>) -> Result<(), StatusCode> {
    if let Some(expected_token) = auth_token
        && !expected_token.is_empty()
    {
        let auth_header = headers
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let expected_bearer = format!("Bearer {}", expected_token);
        if auth_header != expected_bearer && auth_header != expected_token {
            return Err(StatusCode::UNAUTHORIZED);
        }
    }
    Ok(())
}

/// Formats duration seconds into human readable format (e.g. 45s, 12m 30s, 1h 45m).
fn format_duration_secs(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// GET /api/v1/status
async fn get_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ApiResponse<Vec<ProgramStatusDto>>>, StatusCode> {
    check_auth(&headers, &state.auth_token)?;

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
                .map(format_duration_secs)
                .unwrap_or_else(|| "-".to_string());

            ProgramStatusDto {
                name: s.name,
                state: format!("{:?}", s.state).to_uppercase(),
                health: s.health.to_string(),
                pid: s
                    .pid
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                cpu,
                mem,
                uptime,
                description: s.description,
            }
        })
        .collect();

    Ok(Json(ApiResponse::ok(dtos)))
}

/// GET /api/v1/programs/:name
async fn get_program_details(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<(StatusCode, Json<ApiResponse<ProgramDetailsDto>>), StatusCode> {
    check_auth(&headers, &state.auth_token)?;

    let status = match state.manager.get_status(&name).await {
        Ok(s) => s,
        Err(_) => {
            return Ok((
                StatusCode::NOT_FOUND,
                Json(ApiResponse::err(format!("Program '{}' not found", name))),
            ));
        }
    };

    let (cpu_percent, memory_rss_bytes) = match status.metrics {
        Some(m) => (Some(m.cpu_percent), Some(m.memory_rss_bytes)),
        None => (None, None),
    };

    let dto = ProgramDetailsDto {
        name: status.name,
        state: status.state,
        health: status.health.to_string(),
        pid: status.pid,
        uptime_secs: status.uptime_secs,
        exit_code: status.exit_code,
        cpu_percent,
        memory_rss_bytes,
        description: status.description,
    };

    Ok((StatusCode::OK, Json(ApiResponse::ok(dto))))
}

/// POST /api/v1/programs/:name/start
async fn start_program(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Query(query): Query<ActionQuery>,
) -> Result<(StatusCode, Json<ApiResponse<ActionResponse>>), StatusCode> {
    check_auth(&headers, &state.auth_token)?;

    let start_time = Instant::now();
    if let Err(e) = state.manager.start_program(&name).await {
        return Ok((
            StatusCode::BAD_REQUEST,
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

    // Synchronous mode: wait until status settles into Running, Fatal, or Exited
    let timeout_dur = Duration::from_secs(query.timeout);
    while start_time.elapsed() < timeout_dur {
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
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Return current state upon timeout
    let final_status = state
        .manager
        .get_status(&name)
        .await
        .unwrap_or_else(|_| crate::program::state::ProgramStatus::new_stopped(&name));

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
    headers: HeaderMap,
    Path(name): Path<String>,
    Query(query): Query<ActionQuery>,
) -> Result<(StatusCode, Json<ApiResponse<ActionResponse>>), StatusCode> {
    check_auth(&headers, &state.auth_token)?;

    let start_time = Instant::now();
    let grace = Some(Duration::from_secs(query.timeout));
    if let Err(e) = state.manager.stop_program(&name, grace).await {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::err(format!("Stop failed: {}", e))),
        ));
    }

    let final_status = state
        .manager
        .get_status(&name)
        .await
        .unwrap_or_else(|_| crate::program::state::ProgramStatus::new_stopped(&name));

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
    headers: HeaderMap,
    Path(name): Path<String>,
    Query(query): Query<ActionQuery>,
) -> Result<(StatusCode, Json<ApiResponse<ActionResponse>>), StatusCode> {
    check_auth(&headers, &state.auth_token)?;

    let start_time = Instant::now();
    let grace = Some(Duration::from_secs(query.timeout));
    if let Err(e) = state.manager.restart_program(&name, grace).await {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::err(format!("Restart failed: {}", e))),
        ));
    }

    let final_status = state
        .manager
        .get_status(&name)
        .await
        .unwrap_or_else(|_| crate::program::state::ProgramStatus::new_stopped(&name));

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

/// POST /api/v1/all/start
async fn start_all(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<ApiResponse<Vec<ActionResponse>>>), StatusCode> {
    check_auth(&headers, &state.auth_token)?;

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
    headers: HeaderMap,
) -> Result<(StatusCode, Json<ApiResponse<Vec<ActionResponse>>>), StatusCode> {
    check_auth(&headers, &state.auth_token)?;

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
    headers: HeaderMap,
) -> Result<(StatusCode, Json<ApiResponse<ReloadResponse>>), StatusCode> {
    check_auth(&headers, &state.auth_token)?;

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

    let content = match std::fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(e) => {
            return Ok((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::err(format!(
                    "Failed to read config file: {}",
                    e
                ))),
            ));
        }
    };

    let new_config = match SupervisorConfig::from_yaml_str(&content) {
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

/// GET /api/v1/programs/:name/logs
async fn read_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Query(query): Query<LogsQuery>,
) -> Result<(StatusCode, Json<ApiResponse<LogLinesResponse>>), StatusCode> {
    check_auth(&headers, &state.auth_token)?;

    match state.manager.read_logs(&name, Some(query.lines)).await {
        Ok(lines) => Ok((
            StatusCode::OK,
            Json(ApiResponse::ok(LogLinesResponse { name, lines })),
        )),
        Err(e) => Ok((
            StatusCode::NOT_FOUND,
            Json(ApiResponse::err(format!("Failed to read logs: {}", e))),
        )),
    }
}

/// GET /api/v1/programs/:name/logs/stream
async fn stream_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>>, StatusCode> {
    check_auth(&headers, &state.auth_token)?;

    let rx = state
        .manager
        .subscribe_logs(&name)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let stream = BroadcastStream::new(rx).filter_map(|item| match item {
        Ok(line) => Some(Ok(Event::default().data(line))),
        Err(_) => None,
    });

    Ok(Sse::new(stream))
}
