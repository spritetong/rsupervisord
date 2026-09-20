// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use crate::cli::transport::Endpoint;
use crate::control::protocol::{
    ActionResponse, ApiResponse, LogLinesResponse, ProgramDetailsDto, ProgramStatusDto,
    ReloadResponse,
};
use anyhow::{Context, Result, bail};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

/// HTTP REST client communicating with rsupervisord over local IPC or TCP.
#[derive(Debug, Clone)]
pub struct SupervisorClient {
    endpoint: Endpoint,
    auth_token: Option<String>,
}

impl SupervisorClient {
    /// Creates a new client targeting the specified endpoint with an optional token.
    pub fn new(endpoint: Endpoint, auth_token: Option<String>) -> Self {
        Self {
            endpoint,
            auth_token,
        }
    }

    /// Fetches status for all programs, or filters by program names if provided.
    pub async fn status(&self, names: &[String]) -> Result<Vec<ProgramStatusDto>> {
        let programs: Vec<ProgramStatusDto> =
            self.request_json("GET", "/api/v1/status", None).await?;
        if names.is_empty() {
            Ok(programs)
        } else {
            Ok(programs
                .into_iter()
                .filter(|p| names.contains(&p.name))
                .collect())
        }
    }

    /// Fetches detailed status and metrics for a specific program.
    pub async fn get_program(&self, name: &str) -> Result<ProgramDetailsDto> {
        let path = format!("/api/v1/programs/{}", name);
        self.request_json("GET", &path, None).await
    }

    /// Starts a program (or all programs if name is "all").
    pub async fn start(
        &self,
        name: &str,
        sync: bool,
        timeout_secs: u64,
    ) -> Result<Vec<ActionResponse>> {
        if name == "all" {
            self.request_json("POST", "/api/v1/all/start", None).await
        } else {
            let path = format!(
                "/api/v1/programs/{}/start?sync={}&timeout={}",
                name, sync, timeout_secs
            );
            let single: ActionResponse = self.request_json("POST", &path, None).await?;
            Ok(vec![single])
        }
    }

    /// Stops a program (or all programs if name is "all").
    pub async fn stop(
        &self,
        name: &str,
        sync: bool,
        timeout_secs: u64,
    ) -> Result<Vec<ActionResponse>> {
        if name == "all" {
            self.request_json("POST", "/api/v1/all/stop", None).await
        } else {
            let path = format!(
                "/api/v1/programs/{}/stop?sync={}&timeout={}",
                name, sync, timeout_secs
            );
            let single: ActionResponse = self.request_json("POST", &path, None).await?;
            Ok(vec![single])
        }
    }

    /// Restarts a program (or all programs if name is "all").
    pub async fn restart(
        &self,
        name: &str,
        sync: bool,
        timeout_secs: u64,
    ) -> Result<Vec<ActionResponse>> {
        if name == "all" {
            let stopped = self.stop("all", sync, timeout_secs).await?;
            let started = self.start("all", sync, timeout_secs).await?;
            Ok(started.into_iter().chain(stopped).collect())
        } else {
            let path = format!(
                "/api/v1/programs/{}/restart?sync={}&timeout={}",
                name, sync, timeout_secs
            );
            let single: ActionResponse = self.request_json("POST", &path, None).await?;
            Ok(vec![single])
        }
    }

    /// Instructs the daemon to perform a zero-downtime hot reload.
    pub async fn reload(&self) -> Result<ReloadResponse> {
        self.request_json("POST", "/api/v1/reload", None).await
    }

    /// Fetches historical buffered log lines.
    pub async fn read_logs(&self, name: &str, lines: usize) -> Result<Vec<String>> {
        let path = format!("/api/v1/programs/{}/logs?lines={}", name, lines);
        let resp: LogLinesResponse = self.request_json("GET", &path, None).await?;
        Ok(resp.lines)
    }

    /// Streams real-time log lines from the daemon (Server-Sent Events).
    pub async fn stream_logs<F>(&self, name: &str, mut callback: F) -> Result<()>
    where
        F: FnMut(&str),
    {
        let mut stream = tokio::time::timeout(Duration::from_secs(3), self.endpoint.connect())
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "Connection timed out after 3s connecting to rsupervisord daemon at {:?}. Is rsupervisord running?",
                    self.endpoint
                )
            })?
            .with_context(|| format!("Failed to connect to daemon at {:?}", self.endpoint))?;

        let mut req = format!(
            "GET /api/v1/programs/{}/logs/stream HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\n",
            name
        );
        if let Some(ref token) = self.auth_token {
            req.push_str(&format!("Authorization: Bearer {}\r\n", token));
        }
        req.push_str("\r\n");

        stream.write_all(req.as_bytes()).await?;
        stream.flush().await?;

        let mut reader = BufReader::new(stream).lines();
        check_sse_status_and_skip_headers(&mut reader).await?;

        // Stream event lines
        while let Ok(Some(line)) = reader.next_line().await {
            let trimmed = line.trim();
            if let Some(data) = trimmed.strip_prefix("data:") {
                callback(data.trim_start());
            } else if !trimmed.is_empty() && !trimmed.starts_with(':') {
                callback(trimmed);
            }
        }

        Ok(())
    }

    /// Streams all real-time log lines across all managed programs from the daemon (Server-Sent Events).
    pub async fn stream_all_logs<F>(&self, mut callback: F) -> Result<()>
    where
        F: FnMut(&str),
    {
        let mut stream = tokio::time::timeout(Duration::from_secs(3), self.endpoint.connect())
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "Connection timed out after 3s connecting to rsupervisord daemon at {:?}. Is rsupervisord running?",
                    self.endpoint
                )
            })?
            .with_context(|| format!("Failed to connect to daemon at {:?}", self.endpoint))?;

        let mut req =
            "GET /api/v1/logs/stream HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\n"
                .to_string();
        if let Some(ref token) = self.auth_token {
            req.push_str(&format!("Authorization: Bearer {}\r\n", token));
        }
        req.push_str("\r\n");

        stream.write_all(req.as_bytes()).await?;
        stream.flush().await?;

        let mut reader = BufReader::new(stream).lines();
        check_sse_status_and_skip_headers(&mut reader).await?;

        while let Ok(Some(line)) = reader.next_line().await {
            let trimmed = line.trim();
            if let Some(data) = trimmed.strip_prefix("data:") {
                callback(data.trim_start());
            } else if !trimmed.is_empty() && !trimmed.starts_with(':') {
                callback(trimmed);
            }
        }

        Ok(())
    }

    /// Streams real-time system events from the daemon (Server-Sent Events).
    pub async fn stream_events<F>(&self, mut callback: F) -> Result<()>
    where
        F: FnMut(&str),
    {
        let mut stream = tokio::time::timeout(Duration::from_secs(3), self.endpoint.connect())
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "Connection timed out after 3s connecting to rsupervisord daemon at {:?}. Is rsupervisord running?",
                    self.endpoint
                )
            })?
            .with_context(|| format!("Failed to connect to daemon at {:?}", self.endpoint))?;

        let mut req =
            "GET /api/v1/events HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\n"
                .to_string();
        if let Some(ref token) = self.auth_token {
            req.push_str(&format!("Authorization: Bearer {}\r\n", token));
        }
        req.push_str("\r\n");

        stream.write_all(req.as_bytes()).await?;
        stream.flush().await?;

        let mut reader = BufReader::new(stream).lines();
        check_sse_status_and_skip_headers(&mut reader).await?;

        while let Ok(Some(line)) = reader.next_line().await {
            let trimmed = line.trim();
            if let Some(data) = trimmed.strip_prefix("data:") {
                callback(data.trim_start());
            } else if !trimmed.is_empty() && !trimmed.starts_with(':') {
                callback(trimmed);
            }
        }

        Ok(())
    }

    /// Sends an HTTP request and deserializes the JSON response body.
    async fn request_json<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path_and_query: &str,
        body: Option<&[u8]>,
    ) -> Result<T> {
        let body_bytes = body.unwrap_or(b"");
        let mut req_headers = format!(
            "{} {} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
            method,
            path_and_query,
            body_bytes.len()
        );
        if let Some(ref token) = self.auth_token {
            req_headers.push_str(&format!("Authorization: Bearer {}\r\n", token));
        }
        req_headers.push_str("\r\n");

        let mut stream = tokio::time::timeout(Duration::from_secs(3), self.endpoint.connect())
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "Connection timed out after 3s connecting to rsupervisord daemon at {:?}. Is rsupervisord running?",
                    self.endpoint
                )
            })?
            .map_err(|e| {
                anyhow::anyhow!(
                    "Cannot connect to rsupervisord daemon at {:?}: {}. Is rsupervisord running?",
                    self.endpoint,
                    e
                )
            })?;

        stream.write_all(req_headers.as_bytes()).await?;
        if !body_bytes.is_empty() {
            stream.write_all(body_bytes).await?;
        }
        stream.flush().await?;

        let mut raw_response = Vec::new();
        stream.read_to_end(&mut raw_response).await?;

        let response_str = String::from_utf8_lossy(&raw_response);
        let (status_code, body_str) = parse_http_response(&response_str)?;

        if status_code == 403 {
            bail!(
                "Access denied (403 Forbidden): Caller privileges are insufficient to control rsupervisord."
            );
        }
        if status_code == 404 {
            bail!("Not found (404): Resource or program not found on daemon.");
        }

        // Attempt to parse as ApiResponse<T>
        if let Ok(envelope) = serde_json::from_str::<ApiResponse<T>>(body_str) {
            if envelope.success {
                if let Some(data) = envelope.data {
                    return Ok(data);
                }
            } else if let Some(err_msg) = envelope.error {
                bail!("Server error: {}", err_msg);
            }
        }

        // Attempt direct deserialization as T
        match serde_json::from_str::<T>(body_str) {
            Ok(val) => Ok(val),
            Err(e) => {
                if status_code >= 400 {
                    bail!("HTTP error {}: {}", status_code, body_str.trim());
                }
                bail!(
                    "Failed to parse response JSON: {} (raw: '{}')",
                    e,
                    body_str.trim()
                );
            }
        }
    }
}

/// Helper function to parse HTTP status code and body from raw response text.
fn parse_http_response(raw: &str) -> Result<(u16, &str)> {
    let mut parts = raw.splitn(2, "\r\n\r\n");
    let header_part = parts.next().unwrap_or("");
    let body_part = parts
        .next()
        .or_else(|| {
            let mut alt = raw.splitn(2, "\n\n");
            let _ = alt.next();
            alt.next()
        })
        .unwrap_or("");

    let status_line = header_part.lines().next().unwrap_or("");
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(200);

    Ok((status_code, body_part.trim()))
}

/// Reads and verifies the HTTP status line of an SSE response stream, consuming headers up to the body.
async fn check_sse_status_and_skip_headers<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut tokio::io::Lines<R>,
) -> Result<()> {
    let status_line = match reader.next_line().await? {
        Some(line) => line,
        None => bail!("Empty response received from daemon"),
    };

    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(200);

    if status_code >= 400 {
        let mut error_body = String::new();
        while let Ok(Some(line)) = reader.next_line().await {
            if !line.trim().is_empty() {
                error_body.push_str(&line);
                error_body.push('\n');
            }
        }
        if status_code == 401 || status_code == 403 {
            bail!("Access denied ({}): {}", status_code, error_body.trim());
        }
        if status_code == 404 {
            bail!("Resource not found (404): {}", error_body.trim());
        }
        bail!("HTTP error {}: {}", status_code, error_body.trim());
    }

    // Consume remaining headers until blank line
    while let Ok(Some(line)) = reader.next_line().await {
        if line.is_empty() || line == "\r" {
            break;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_http_response() {
        let raw = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 15\r\n\r\n{\"success\":true}";
        let (code, body) = parse_http_response(raw).unwrap();
        assert_eq!(code, 200);
        assert_eq!(body, "{\"success\":true}");
    }

    #[test]
    fn test_parse_http_error_response() {
        let raw = "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n";
        let (code, body) = parse_http_response(raw).unwrap();
        assert_eq!(code, 403);
        assert_eq!(body, "");
    }
}
