//! `POST /api/sensor/request-approval` — the operator-driven enrollment
//! attempt.
//!
//! The background loop already retries, but on a jittered ~30s schedule the
//! operator cannot see. After clicking Approve on the Standalone there is an
//! unexplained pause before the sensor notices, which reads as a failure and
//! sends people looking for bugs that aren't there. This endpoint makes the
//! round trip deliberate: press it, get the answer.

mod common;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::http::StatusCode;
use fluxfang_api::capture::MockCapturerFactory;
use fluxfang_db::{
    AppConfigRepo, DataSourceRepo, NewDataSource, NodeConfig, NodeRole, SensorConfig, SensorRepo,
};
use sqlx::PgPool;

async fn free_port() -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p = l.local_addr().unwrap().port();
    drop(l);
    p
}

/// Point this node at `port` as its Standalone, as first-run setup would.
async fn provision_sensor(pool: &PgPool, port: u16, key: &fluxfang_sensor_proto::Key) {
    AppConfigRepo::set_node_config(
        pool,
        &NodeConfig {
            role: NodeRole::Sensor,
            node_sensor_id: "frontgate".to_string(),
            sensor: Some(SensorConfig {
                host: "127.0.0.1".to_string(),
                port,
                key: fluxfang_sensor_proto::encode_key(key),
                cache_ttl_secs: 604_800,
            }),
        },
    )
    .await
    .unwrap();
}

async fn login(app: &axum::Router) -> String {
    common::post_json(
        app,
        "/api/setup",
        r#"{"password":"pw123456","role":"standalone","node_sensor_id":"local"}"#,
    )
    .await;
    let resp = common::post_json(app, "/api/login", r#"{"password":"pw123456"}"#).await;
    common::session_cookie(&resp)
}

/// Pressing the button must enroll immediately, rather than leaving the
/// operator waiting on the invisible background schedule.
#[tokio::test]
async fn requesting_approval_enrolls_with_the_standalone_immediately() {
    let pool = common::fresh_pool_shared().await;
    let state = common::state_with_factory(pool.clone(), Arc::new(MockCapturerFactory::new()));
    let app = fluxfang_api::app(state);
    let cookie = login(&app).await;

    // A Standalone listening with an open enrollment window.
    let port = free_port().await;
    let ds = DataSourceRepo::insert(
        &pool,
        NewDataSource {
            kind: "sensor".into(),
            mode: "listener".into(),
            interface: None,
            config: serde_json::json!({"bind_ip":"127.0.0.1","bind_port":port}),
        },
    )
    .await
    .unwrap();
    let mgr = common::sensor_manager(pool.clone());
    mgr.start(ds.id).await;
    mgr.open_enrollment_window(ds.id).await;

    let key = fluxfang_sensor_proto::generate_key();
    provision_sensor(&pool, port, &key).await;

    // Nothing has enrolled yet -- the background loop is not running here, so
    // any enrollment that appears is this request's doing.
    assert!(SensorRepo::list(&pool).await.unwrap().is_empty());

    let resp = common::post_with_cookie(&app, "/api/sensor/request-approval", &cookie).await;
    common::assert_status(&resp, StatusCode::OK);
    let body = common::body_json(resp).await;

    assert_eq!(
        body["status"], "pending",
        "a fresh enrollment lands as pending, awaiting the operator: {body}",
    );
    assert_eq!(body["sensor_id"], "frontgate");
    assert_eq!(
        body["fingerprint"],
        fluxfang_sensor_proto::fingerprint("frontgate", &key),
        "the response must carry the fingerprint the operator has to match",
    );

    let enrolled = SensorRepo::get_by_sensor_id(&pool, ds.id, "frontgate")
        .await
        .unwrap()
        .expect("the request must have registered this sensor");
    assert_eq!(enrolled.status, "pending");

    mgr.stop(ds.id).await;
}

/// Once approved, the same button must report that plainly -- this is how an
/// operator confirms the approval actually took, instead of waiting to see
/// whether emissions eventually appear.
#[tokio::test]
async fn requesting_approval_reports_approved_once_the_operator_has_approved() {
    let pool = common::fresh_pool_shared().await;
    let state = common::state_with_factory(pool.clone(), Arc::new(MockCapturerFactory::new()));
    let app = fluxfang_api::app(state);
    let cookie = login(&app).await;

    let port = free_port().await;
    let ds = DataSourceRepo::insert(
        &pool,
        NewDataSource {
            kind: "sensor".into(),
            mode: "listener".into(),
            interface: None,
            config: serde_json::json!({"bind_ip":"127.0.0.1","bind_port":port}),
        },
    )
    .await
    .unwrap();
    let mgr = common::sensor_manager(pool.clone());
    mgr.start(ds.id).await;

    let key = fluxfang_sensor_proto::generate_key();
    let fp = fluxfang_sensor_proto::fingerprint("frontgate", &key);
    let s = SensorRepo::insert_pending(&pool, ds.id, "frontgate", &fp, None)
        .await
        .unwrap();
    SensorRepo::set_key(&pool, s.id, &fluxfang_sensor_proto::encode_key(&key), &fp)
        .await
        .unwrap();
    SensorRepo::set_status(&pool, s.id, "approved", true)
        .await
        .unwrap();
    provision_sensor(&pool, port, &key).await;

    let resp = common::post_with_cookie(&app, "/api/sensor/request-approval", &cookie).await;
    common::assert_status(&resp, StatusCode::OK);
    let body = common::body_json(resp).await;
    assert_eq!(body["status"], "approved", "got {body}");
}

/// A half-configured node must explain itself rather than erroring: this is a
/// normal state between choosing the Sensor role and entering a key, and the
/// button is visible throughout.
#[tokio::test]
async fn requesting_approval_on_an_unconfigured_node_explains_what_is_missing() {
    let pool = common::fresh_pool_shared().await;
    let state = common::state_with_factory(pool.clone(), Arc::new(MockCapturerFactory::new()));
    let app = fluxfang_api::app(state);
    let cookie = login(&app).await;

    let resp = common::post_with_cookie(&app, "/api/sensor/request-approval", &cookie).await;
    common::assert_status(&resp, StatusCode::OK);
    let body = common::body_json(resp).await;
    assert_eq!(body["status"], "not_configured");
    assert!(
        body["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("Settings"),
        "must point the operator somewhere actionable: {body}",
    );
}

/// The endpoint sits behind the session guard like every other operator
/// action -- it causes an outbound network request, so it must not be
/// reachable unauthenticated.
#[tokio::test]
async fn requesting_approval_requires_authentication() {
    let app = common::test_app().await;
    let resp = common::call(
        &app,
        axum::http::Request::builder()
            .method("POST")
            .uri("/api/sensor/request-approval")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_ne!(
        resp.status(),
        StatusCode::OK,
        "must not be callable without a session",
    );
}
