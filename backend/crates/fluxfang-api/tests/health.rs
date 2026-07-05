use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt; // oneshot

mod common;

/// Health stays PUBLIC even after Task 2.2 wires up `require_auth` for
/// everything else — it's an infra check and must never depend on session
/// state, so this still hits it with no cookie at all.
#[tokio::test]
async fn health_returns_ok() {
    let app = common::test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&bytes[..], br#"{"status":"ok"}"#);
}
