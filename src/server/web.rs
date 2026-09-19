use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

/// Embedded static web assets compiled directly into the binary.
#[derive(RustEmbed)]
#[folder = "web/"]
pub struct WebAssets;

/// Axum fallback handler for serving embedded static assets and SPA routing.
pub async fn static_handler(uri: axum::http::Uri) -> Response {
    let mut path = uri.path().trim_start_matches('/').to_string();
    if path.is_empty() {
        path = "index.html".to_string();
    }

    // Do not intercept API requests with HTML
    if path.starts_with("api/") {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header(header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(
                r#"{"success":false,"error":"Endpoint not found"}"#,
            ))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    }

    match WebAssets::get(&path) {
        Some(file) => {
            let mime = mime_guess::from_path(&path).first_or_octet_stream();
            let cache_control = if path == "index.html" {
                "no-cache, no-store, must-revalidate"
            } else {
                "public, max-age=86400"
            };
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime.as_ref())
                .header(header::CACHE_CONTROL, cache_control)
                .body(axum::body::Body::from(file.data))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        None => {
            // SPA fallback: if not found and has no extension, serve index.html
            if !path.contains('.')
                && let Some(index_file) = WebAssets::get("index.html")
            {
                return Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                    .header(header::CACHE_CONTROL, "no-cache, no-store, must-revalidate")
                    .body(axum::body::Body::from(index_file.data))
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
            }
            Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                .body(axum::body::Body::from("404 Not Found"))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
    }
}
