// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use axum::extract::{Request, State};
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE, COOKIE, SET_COOKIE, WWW_AUTHENTICATE};
use axum::http::{HeaderValue, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use dashmap::DashMap;
use sha1::{Digest, Sha1};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Session cookie name used by the Web UI login flow.
pub const SESSION_COOKIE_NAME: &str = "rsupervisord_session";

/// Sliding session lifetime (7 days).
pub const SESSION_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

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
        if !constant_time_eq(self.username.as_bytes(), candidate_user.as_bytes()) {
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
            constant_time_eq(
                computed.as_bytes(),
                expected_sha.trim().to_ascii_lowercase().as_bytes(),
            )
        } else {
            constant_time_eq(self.password.as_bytes(), candidate_pass.as_bytes())
        }
    }
}

/// Length-tolerant constant-time byte equality (avoids early-exit timing leaks).
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
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

/// In-memory sliding-expiry session store shared by all listeners.
#[derive(Debug, Default)]
pub struct SessionStore {
    sessions: DashMap<String, Instant>,
}

impl SessionStore {
    /// Issues a new cryptographically random session id (64 hex chars).
    pub fn create_session(&self) -> Option<String> {
        let mut buf = [0u8; 32];
        getrandom::fill(&mut buf).ok()?;
        let mut id = String::with_capacity(64);
        for b in buf {
            use std::fmt::Write;
            let _ = write!(id, "{:02x}", b);
        }
        self.sessions
            .insert(id.clone(), Instant::now() + SESSION_TTL);
        Some(id)
    }

    /// Validates a session id, applying sliding renewal and lazy pruning of expired entries.
    pub fn validate_session(&self, id: &str) -> bool {
        if id.is_empty() {
            return false;
        }
        let now = Instant::now();
        self.sessions.retain(|_, expires_at| *expires_at > now);
        match self.sessions.get_mut(id) {
            Some(mut entry) if *entry > now => {
                *entry = now + SESSION_TTL;
                true
            }
            Some(entry) => {
                let id = entry.key().clone();
                drop(entry);
                self.sessions.remove(&id);
                false
            }
            None => false,
        }
    }

    /// Removes a session id (logout / clear).
    pub fn invalidate_session(&self, id: &str) {
        self.sessions.remove(id);
    }
}

/// Authentication inputs gathered from either an HTTP request or a login body.
#[derive(Debug, Default, Clone)]
pub struct AuthAttempts {
    /// Candidate (username, password) pair for Basic Auth.
    pub basic: Option<(String, String)>,
    /// Candidate bearer/token value (Authorization header, raw header, or `?token=`).
    pub token: Option<String>,
    /// Candidate Web UI session cookie value.
    pub session: Option<String>,
}

/// Authentication state shared by the unified path-tier middleware.
#[derive(Debug, Clone)]
pub struct ServerAuthState {
    pub basic_auth: Option<BasicAuthConfig>,
    pub auth_token: Option<String>,
    pub sessions: Arc<SessionStore>,
}

impl ServerAuthState {
    /// Builds auth state, normalizing an empty token to `None`.
    pub fn new(
        basic_auth: Option<BasicAuthConfig>,
        auth_token: Option<String>,
        sessions: Arc<SessionStore>,
    ) -> Self {
        Self {
            basic_auth,
            auth_token: auth_token.filter(|t| !t.is_empty()),
            sessions,
        }
    }

    /// Returns true if either Basic Auth or token authentication is enabled.
    #[inline]
    pub fn is_auth_configured(&self) -> bool {
        self.basic_auth.is_some() || self.auth_token.is_some()
    }

    /// Unified OR authorization: pass when nothing is configured, when the
    /// session cookie is valid, when the token matches, or when basic verifies.
    pub fn authorize(&self, attempts: &AuthAttempts) -> bool {
        if !self.is_auth_configured() {
            return true;
        }
        if let Some(session_id) = attempts.session.as_deref()
            && self.sessions.validate_session(session_id)
        {
            return true;
        }
        if let (Some(expected), Some(candidate)) =
            (self.auth_token.as_deref(), attempts.token.as_deref())
            && constant_time_eq(expected.as_bytes(), candidate.as_bytes())
        {
            return true;
        }
        if let (Some(basic), Some((user, pass))) =
            (self.basic_auth.as_ref(), attempts.basic.as_ref())
            && basic.verify(user, pass)
        {
            return true;
        }
        false
    }

    /// Collects candidate credentials from an incoming HTTP request.
    pub fn attempts_from_request(&self, req: &Request) -> AuthAttempts {
        let mut attempts = AuthAttempts::default();

        if let Some(auth_val) = req
            .headers()
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
        {
            let trimmed = auth_val.trim();
            if let Some((u, p)) = extract_basic_auth(trimmed) {
                attempts.basic = Some((u, p));
            }
            if let Some(token) = trimmed
                .strip_prefix("Bearer ")
                .or_else(|| trimmed.strip_prefix("bearer "))
            {
                attempts.token = Some(token.trim().to_string());
            } else if !trimmed.starts_with("Basic ") && !trimmed.starts_with("basic ") {
                attempts.token = Some(trimmed.to_string());
            }
        }

        if attempts.token.is_none()
            && let Some(query) = req.uri().query()
        {
            for pair in query.split('&') {
                if let Some((k, v)) = pair.split_once('=')
                    && k == "token"
                {
                    attempts.token = Some(percent_decode_query_param(v));
                    break;
                }
            }
        }

        attempts.session = session_id_from_headers(req.headers());
        attempts
    }
}

/// Decodes a percent-encoded query parameter string (`%XX` hex and `+` as space).
pub fn percent_decode_query_param(s: &str) -> String {
    let mut bytes = Vec::with_capacity(s.len());
    let mut chars = s.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let next_two = (chars.next(), chars.next());
            match next_two {
                (Some(h1), Some(h2)) => {
                    let hex = [h1, h2];
                    if let Ok(hex_str) = std::str::from_utf8(&hex)
                        && let Ok(val) = u8::from_str_radix(hex_str, 16)
                    {
                        bytes.push(val);
                        continue;
                    }
                    bytes.push(b'%');
                    bytes.push(h1);
                    bytes.push(h2);
                }
                (Some(h1), None) => {
                    bytes.push(b'%');
                    bytes.push(h1);
                }
                _ => {
                    bytes.push(b'%');
                }
            }
        } else if b == b'+' {
            bytes.push(b' ');
        } else {
            bytes.push(b);
        }
    }
    String::from_utf8(bytes).unwrap_or_else(|_| s.to_string())
}

/// Parses the Web UI session id out of a `Cookie` header.
pub fn session_id_from_headers(headers: &axum::http::HeaderMap) -> Option<String> {
    let cookie = headers.get(COOKIE)?.to_str().ok()?;
    for part in cookie.split(';') {
        let part = part.trim();
        if let Some((name, value)) = part.split_once('=')
            && name.trim() == SESSION_COOKIE_NAME
            && !value.is_empty()
        {
            return Some(value.to_string());
        }
    }
    None
}

/// Detects whether the client reached us over HTTPS (directly or via proxy).
fn request_is_https(headers: &axum::http::HeaderMap, uri: &Uri) -> bool {
    if uri.scheme_str() == Some("https") {
        return true;
    }
    // TODO: Add reverse proxy trust configuration / CIDR whitelist before trusting X-Forwarded-Proto blindly.
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("https"))
        .unwrap_or(false)
}

/// Builds a `Set-Cookie` value for a newly issued session.
fn build_session_cookie(session_id: &str, headers: &axum::http::HeaderMap, uri: &Uri) -> String {
    let secure = if request_is_https(headers, uri) {
        "; Secure"
    } else {
        ""
    };
    format!(
        "{name}={id}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{secure}",
        name = SESSION_COOKIE_NAME,
        id = session_id,
        max_age = SESSION_TTL.as_secs(),
        secure = secure,
    )
}

/// Builds a `Set-Cookie` value that clears the session cookie.
fn build_clear_cookie(headers: &axum::http::HeaderMap, uri: &Uri) -> String {
    let secure = if request_is_https(headers, uri) {
        "; Secure"
    } else {
        ""
    };
    format!(
        "{name}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{secure}",
        name = SESSION_COOKIE_NAME,
        secure = secure,
    )
}

/// Issues a session cookie header pair for a successful login response.
pub fn issue_session_cookie(
    session_id: &str,
    headers: &axum::http::HeaderMap,
    uri: &Uri,
) -> (axum::http::HeaderName, HeaderValue) {
    let value = build_session_cookie(session_id, headers, uri);
    (
        SET_COOKIE,
        HeaderValue::from_str(&value).unwrap_or_else(|_| HeaderValue::from_static("invalid=1")),
    )
}

/// Builds a cookie-clearing header pair for logout responses.
pub fn clear_session_cookie(
    headers: &axum::http::HeaderMap,
    uri: &Uri,
) -> (axum::http::HeaderName, HeaderValue) {
    let value = build_clear_cookie(headers, uri);
    (
        SET_COOKIE,
        HeaderValue::from_str(&value).unwrap_or_else(|_| HeaderValue::from_static("invalid=1")),
    )
}

/// Returns true when the path must be authenticated (REST API or XMLRPC).
fn is_protected_path(path: &str) -> bool {
    path.starts_with("/api/") || path == "/RPC2"
}

/// Returns true for the public login-surface endpoints.
fn is_public_auth_path(path: &str) -> bool {
    matches!(
        path,
        "/api/v1/auth/config" | "/api/v1/auth/login" | "/api/v1/auth/logout"
    )
}

/// Unified authentication middleware guarding `/api/v1/*` and `/RPC2` on every listener.
///
/// Public paths: static shell (anything outside `/api/` and `/RPC2`) plus the
/// three `/api/v1/auth/*` endpoints. Failure returns bare JSON 401 except on
/// `/RPC2`, which carries a `WWW-Authenticate: Basic realm="supervisor"` challenge.
pub async fn http_auth_middleware(
    State(auth_state): State<ServerAuthState>,
    req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path();
    if !is_protected_path(path) || is_public_auth_path(path) {
        return next.run(req).await;
    }

    let attempts = auth_state.attempts_from_request(&req);
    if auth_state.authorize(&attempts) {
        return next.run(req).await;
    }

    let is_rpc = path == "/RPC2";
    let error = if auth_state.basic_auth.is_some() {
        "Unauthorized: Invalid basic authentication credentials"
    } else {
        "Unauthorized: Invalid token"
    };

    let mut response = Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(CONTENT_TYPE, "application/json");
    if is_rpc && auth_state.basic_auth.is_some() {
        response = response.header(
            WWW_AUTHENTICATE,
            HeaderValue::from_static("Basic realm=\"supervisor\""),
        );
    }
    let body = format!(r#"{{"success":false,"error":"{}"}}"#, error);
    response
        .body(axum::body::Body::from(body))
        .unwrap_or_else(|_| StatusCode::UNAUTHORIZED.into_response())
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

    fn store() -> Arc<SessionStore> {
        Arc::new(SessionStore::default())
    }

    fn attempts(
        basic: Option<(&str, &str)>,
        token: Option<&str>,
        session: Option<&str>,
    ) -> AuthAttempts {
        AuthAttempts {
            basic: basic.map(|(u, p)| (u.to_string(), p.to_string())),
            token: token.map(str::to_string),
            session: session.map(str::to_string),
        }
    }

    #[test]
    fn test_empty_token_normalized_to_none() {
        let state = ServerAuthState::new(None, Some(String::new()), store());
        assert!(!state.is_auth_configured());
        assert!(state.authorize(&AuthAttempts::default()));
    }

    #[test]
    fn test_authorize_open_when_unconfigured() {
        let state = ServerAuthState::new(None, None, store());
        assert!(state.authorize(&AuthAttempts::default()));
    }

    #[test]
    fn test_authorize_token_only() {
        let state = ServerAuthState::new(None, Some("s3cret".into()), store());
        assert!(!state.authorize(&AuthAttempts::default()));
        assert!(state.authorize(&attempts(None, Some("s3cret"), None)));
        assert!(!state.authorize(&attempts(None, Some("wrong"), None)));
    }

    #[test]
    fn test_authorize_basic_only() {
        let basic = BasicAuthConfig::new(Some("admin".into()), Some("pw".into()));
        let state = ServerAuthState::new(basic, None, store());
        assert!(!state.authorize(&AuthAttempts::default()));
        assert!(state.authorize(&attempts(Some(("admin", "pw")), None, None)));
        assert!(!state.authorize(&attempts(Some(("admin", "bad")), None, None)));
    }

    #[test]
    fn test_authorize_or_semantics_when_both_configured() {
        let basic = BasicAuthConfig::new(Some("admin".into()), Some("pw".into()));
        let state = ServerAuthState::new(basic, Some("tok".into()), store());

        // Right basic + wrong token -> pass (OR)
        assert!(state.authorize(&attempts(Some(("admin", "pw")), Some("wrong"), None)));
        // Wrong basic + right token -> pass (OR)
        assert!(state.authorize(&attempts(Some(("admin", "bad")), Some("tok"), None)));
        // Both wrong -> fail
        assert!(!state.authorize(&attempts(Some(("admin", "bad")), Some("wrong"), None)));
    }

    #[test]
    fn test_authorize_session_cookie() {
        let basic = BasicAuthConfig::new(Some("admin".into()), Some("pw".into()));
        let sessions = store();
        let state = ServerAuthState::new(basic, None, sessions.clone());

        let sid = sessions.create_session().expect("create session");
        assert!(state.authorize(&attempts(None, None, Some(&sid))));
        assert!(!state.authorize(&attempts(None, None, Some("deadbeef"))));

        sessions.invalidate_session(&sid);
        assert!(!state.authorize(&attempts(None, None, Some(&sid))));
    }

    #[test]
    fn test_session_sliding_renewal_and_expiration() {
        let store = SessionStore::default();
        let sid = store.create_session().expect("create session");
        assert!(store.validate_session(&sid));
        assert!(store.validate_session(&sid));

        // Inject an already-expired entry and ensure it is pruned.
        store
            .sessions
            .insert("expired".into(), Instant::now() - Duration::from_secs(1));
        assert!(!store.validate_session("expired"));
        assert!(store.sessions.get("expired").is_none());
    }

    #[test]
    fn test_percent_decode_query_param() {
        assert_eq!(percent_decode_query_param("hello"), "hello");
        assert_eq!(percent_decode_query_param("hello%20world"), "hello world");
        assert_eq!(percent_decode_query_param("a%2Bb%3Dc"), "a+b=c");
        assert_eq!(percent_decode_query_param("a+b"), "a b");
        assert_eq!(percent_decode_query_param("invalid%2"), "invalid%2");
    }
}
