use axum::body::Body;
use axum::http::{Request, StatusCode};
use fluxfang_api::app;
use tower::ServiceExt; // oneshot

#[tokio::test]
async fn health_returns_ok() {
    let resp = app()
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
