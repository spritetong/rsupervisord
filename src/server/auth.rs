// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use axum::extract::{Request, State};
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE, WWW_AUTHENTICATE};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use sha1::{Digest, Sha1};

/// Configuration for HTTP Basic Authentication (username & password).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasicAuthConfig {
    pub username: String,
    pub password: String,
}

impl BasicAuthConfig {
    /// Creates a new BasicAuthConfig if at least one credential is non-empty.
    pub fn new(username: Option<String>, password: Option<String>) -> Option<Self> {
        let u = username.unwrap_or_default();
        let p = password.unwrap_or_default();
        if u.is_empty() && p.is_empty() {
            None
        } else {
            Some(Self {
                username: u,
                password: p,
            })
        }
    }

    /// Verifies candidate credentials against configured username and password.
    /// Supports both plaintext and `{SHA}` prefixed SHA-1 hash strings (case-insensitive).
    pub fn verify(&self, candidate_user: &str, candidate_pass: &str) -> bool {
        if self.username != candidate_user {
            return false;
        }

        if let Some(expected_sha) = self
            .password
            .strip_prefix("{SHA}")
            .or_else(|| self.password.strip_prefix("{sha}"))
        {
            let mut hasher = Sha1::new();
            hasher.update(candidate_pass.as_bytes());
            let result = hasher.finalize();
            let mut computed = String::with_capacity(40);
            for b in result {
                use std::fmt::Write;
                let _ = write!(computed, "{:02x}", b);
            }
            computed.eq_ignore_ascii_case(expected_sha.trim())
        } else {
            self.password == candidate_pass
        }
    }
}

/// Extracts (username, password) from an Authorization header value (e.g. `Basic dXNlcjpwYXNz`).
pub fn extract_basic_auth(auth_header: &str) -> Option<(String, String)> {
    let trimmed = auth_header.trim();
    let b64_payload = trimmed
        .strip_prefix("Basic ")
        .or_else(|| trimmed.strip_prefix("basic "))?
        .trim();

    let decoded_bytes = BASE64_STANDARD.decode(b64_payload).ok()?;
    let decoded_str = String::from_utf8(decoded_bytes).ok()?;
    let (user, pass) = decoded_str.split_once(':')?;
    Some((user.to_string(), pass.to_string()))
}

/// Authentication state injected into TCP router middleware.
#[derive(Debug, Clone, Default)]
pub struct ServerAuthState {
    pub basic_auth: Option<BasicAuthConfig>,
    pub auth_token: Option<String>,
}

impl ServerAuthState {
    /// Returns true if either Basic Auth or Bearer Token authentication is enabled.
    #[inline]
    pub fn is_auth_configured(&self) -> bool {
        self.basic_auth.is_some() || self.auth_token.is_some()
    }

    /// Evaluates candidate request against configured Basic Auth and Bearer Token credentials.
    pub fn authenticate_request(&self, req: &Request) -> bool {
        if !self.is_auth_configured() {
            return true;
        }

        // Check HTTP Basic Authentication if configured
        if let Some(ref basic) = self.basic_auth
            && let Some(auth_val) = req
                .headers()
                .get(AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
            && let Some((u, p)) = extract_basic_auth(auth_val)
            && basic.verify(&u, &p)
        {
            return true;
        }

        // Check Bearer Token if configured
        if let Some(ref token) = self.auth_token
            && verify_bearer_token(req, token)
        {
            return true;
        }

        false
    }
}

/// Checks whether an incoming request satisfies token authentication.
pub fn verify_bearer_token(req: &Request, expected_token: &str) -> bool {
    if expected_token.is_empty() {
        return true;
    }
    // Check Authorization header
    if let Some(auth_val) = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        let trimmed = auth_val.trim();
        if let Some(token) = trimmed
            .strip_prefix("Bearer ")
            .or_else(|| trimmed.strip_prefix("bearer "))
            && token.trim() == expected_token
        {
            return true;
        }
        if trimmed == expected_token {
            return true;
        }
    }
    // Check URL query string `?token=...`
    if let Some(query) = req.uri().query() {
        for pair in query.split('&') {
            if let Some((k, v)) = pair.split_once('=')
                && k == "token"
                && v == expected_token
            {
                return true;
            }
        }
    }
    false
}

/// Axum middleware that guards TCP HTTP traffic with HTTP Basic Authentication and/or Bearer Token.
pub async fn inet_http_auth_middleware(
    State(auth_state): State<ServerAuthState>,
    req: Request,
    next: Next,
) -> Response {
    // If no authentication is configured, allow all requests immediately
    if !auth_state.is_auth_configured() {
        return next.run(req).await;
    }

    if auth_state.authenticate_request(&req) {
        return next.run(req).await;
    }

    // Unauthorized challenge response
    if auth_state.basic_auth.is_some() {
        Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(
                WWW_AUTHENTICATE,
                HeaderValue::from_static("Basic realm=\"supervisor\""),
            )
            .header(CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(
                r#"{"success":false,"error":"Unauthorized: Invalid basic authentication credentials"}"#,
            ))
            .unwrap_or_else(|_| StatusCode::UNAUTHORIZED.into_response())
    } else {
        Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(
                r#"{"success":false,"error":"Unauthorized: Invalid token"}"#,
            ))
            .unwrap_or_else(|_| StatusCode::UNAUTHORIZED.into_response())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_auth_plaintext_verification() {
        let auth = BasicAuthConfig {
            username: "admin".to_string(),
            password: "thepassword".to_string(),
        };

        assert!(auth.verify("admin", "thepassword"));
        assert!(!auth.verify("admin", "wrongpassword"));
        assert!(!auth.verify("wronguser", "thepassword"));
    }

    #[test]
    fn test_basic_auth_sha1_verification() {
        // sha1("thepassword") = 82ab876d1387bfafe46cc1c8a2ef074eae50cb1d
        let auth = BasicAuthConfig {
            username: "test1".to_string(),
            password: "{SHA}82ab876d1387bfafe46cc1c8a2ef074eae50cb1d".to_string(),
        };

        assert!(auth.verify("test1", "thepassword"));
        assert!(!auth.verify("test1", "wrongpassword"));
        assert!(!auth.verify("otheruser", "thepassword"));

        // Case insensitivity of hex hash and {sha} prefix
        let auth_upper = BasicAuthConfig {
            username: "test1".to_string(),
            password: "{sha}82AB876D1387BFAFE46CC1C8A2EF074EAE50CB1D".to_string(),
        };
        assert!(auth_upper.verify("test1", "thepassword"));
    }

    #[test]
    fn test_extract_basic_auth() {
        // base64("admin:123456") = "YWRtaW46MTIzNDU2"
        let header = "Basic YWRtaW46MTIzNDU2";
        let (user, pass) = extract_basic_auth(header).expect("valid basic auth header");
        assert_eq!(user, "admin");
        assert_eq!(pass, "123456");

        // Invalid format
        assert!(extract_basic_auth("Bearer token").is_none());
        assert!(extract_basic_auth("Basic !!!invalid_base64").is_none());
    }
}
