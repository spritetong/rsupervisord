use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use rsupervisord::config::SupervisorConfig;
use rsupervisord::manager::SupervisorManager;
use rsupervisord::server::{AppState, build_router};
use tower::ServiceExt;

fn create_test_app_state() -> AppState {
    let config = SupervisorConfig::default();
    let manager = SupervisorManager::new(&config).expect("create supervisor manager");
    AppState {
        manager: manager.handle(),
        config_path: None,
        auth_token: None,
    }
}

#[tokio::test]
async fn test_web_index_html_serving() {
    let state = create_test_app_state();
    let app = build_router(state);

    // 1. GET /
    let response = app
        .clone()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(content_type.contains("text/html"));

    let body_bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body_str = String::from_utf8_lossy(&body_bytes);
    assert!(body_str.contains("rsupervisord"));
    assert!(body_str.contains("vue.global.prod.js"));
    assert!(body_str.contains("id=\"app\""));

    // 2. GET /index.html
    let response_index = app
        .oneshot(
            Request::builder()
                .uri("/index.html")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response_index.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_web_vue_js_serving() {
    let state = create_test_app_state();
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/vue.global.prod.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(content_type.contains("javascript"));

    let body_bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    // Standalone Vue 3 production runtime should be > 100KB
    assert!(body_bytes.len() > 100_000);
}

#[tokio::test]
async fn test_web_spa_fallback() {
    let state = create_test_app_state();
    let app = build_router(state);

    // Non-file route should fall back to index.html for client-side SPA routing
    let response = app
        .oneshot(
            Request::builder()
                .uri("/programs/my-worker")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(content_type.contains("text/html"));

    let body_bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body_str = String::from_utf8_lossy(&body_bytes);
    assert!(body_str.contains("rsupervisord"));
}

#[tokio::test]
async fn test_web_api_404_not_intercepted_by_spa() {
    let state = create_test_app_state();
    let app = build_router(state);

    // Unknown API endpoints must NOT return index.html
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/nonexistent_endpoint")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(content_type.contains("application/json"));

    let body_bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body_str = String::from_utf8_lossy(&body_bytes);
    assert!(body_str.contains("Endpoint not found"));
}

#[tokio::test]
async fn test_web_static_missing_file_returns_404() {
    let state = create_test_app_state();
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/favicon.nonexistent.ico")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
