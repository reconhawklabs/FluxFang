//! MAC persistence: the `mac_persistence=` filters on
//! `GET /api/emitters` / `GET /api/emissions`, and the per-data-source
//! retention gate that decides whether an emission is stored at all.

use std::sync::Arc;

use axum::http::StatusCode;
use chrono::Utc;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use fluxfang_api::capture::MockCapturerFactory;
use fluxfang_db::models::{NewDataSource, NewEmission, NewEmitter};
use fluxfang_db::{DataSourceRepo, EmissionRepo, EmitterRepo, SessionRepo};

mod common;
use common::{
    assert_status, body_json, get_with_cookie, post_json, post_json_with_cookie, session_cookie,
    test_app_with_factory,
};

async fn login(app: &axum::Router) -> String {
    post_json(app, "/api/setup", r#"{"password":"pw123456"}"#).await;
    let resp = post_json(app, "/api/login", r#"{"password":"pw123456"}"#).await;
    assert_status(&resp, StatusCode::OK);
    session_cookie(&resp)
}

/// Seed an auto-created emitter of persistence class `class`, plus one
/// emission attached to it. Returns the emitter id.
///
/// The data source and session are passed in rather than created here:
/// `one_active_session` (migration 0002) permits exactly one open session
/// per node, so opening one per emitter would collide.
async fn seed(pool: &PgPool, ds: Uuid, session: Uuid, mac: &str, class: &str) -> Uuid {
    let emitter = EmitterRepo::insert(
        pool,
        NewEmitter {
            name: format!("WiFi Client {mac}"),
            emitter_type: Some("wifi_client".to_string()),
            attributes: json!({"src_mac": mac, "mac_persistence": class}),
            identity_key: Some(format!("wifi_client:{mac}")),
            match_criteria: json!({}),
            ..Default::default()
        },
    )
    .await
    .expect("emitter");
    EmissionRepo::insert(
        pool,
        NewEmission {
            data_source_id: Some(ds),
            emitter_id: Some(emitter.id),
            session_id: Some(session),
            observed_at: Utc::now(),
            signal_strength: Some(-50),
            location: None,
            location_quality: "none".to_string(),
            kind: "wifi".to_string(),
            payload: json!({"frame_type": "probe_request", "src_mac": mac}),
            sensor_id: "local".to_string(),
        },
    )
    .await
    .expect("emission");
    emitter.id
}

/// Seed one emitter per class and return the app + cookie.
async fn app_with_one_of_each() -> (axum::Router, String) {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::default())).await;
    let cookie = login(&app).await;
    let ds = DataSourceRepo::insert(&pool, NewDataSource::wifi_monitor("wlan0"))
        .await
        .expect("data source")
        .id;
    let session = SessionRepo::open(&pool).await.expect("session").id;
    for (i, class) in [
        "stable",
        "per_network",
        "session",
        "ephemeral",
        "unlinkable",
    ]
    .iter()
    .enumerate()
    {
        seed(
            &pool,
            ds,
            session,
            &format!("3a:00:00:00:00:{i:02x}"),
            class,
        )
        .await;
    }
    (app, cookie)
}

#[tokio::test]
async fn emitters_randomized_badge_filter_selects_only_the_short_lived_classes() {
    let (app, cookie) = app_with_one_of_each().await;

    let resp = get_with_cookie(&app, "/api/emitters?mac_persistence=randomized", &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let body = body_json(resp).await;

    let classes: Vec<String> = body["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|e| e["attributes"]["mac_persistence"].as_str().unwrap().into())
        .collect();
    let mut sorted = classes.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        vec!["ephemeral".to_string(), "unlinkable".to_string()],
        "'randomized' is the short-lived badge, not 'any randomized address'"
    );
    assert_eq!(body["total"], 2);
}

#[tokio::test]
async fn emitters_longterm_badge_filter_selects_per_network_and_session() {
    let (app, cookie) = app_with_one_of_each().await;

    let resp = get_with_cookie(
        &app,
        "/api/emitters?mac_persistence=randomized-longterm",
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::OK);
    let body = body_json(resp).await;

    let mut classes: Vec<String> = body["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|e| e["attributes"]["mac_persistence"].as_str().unwrap().into())
        .collect();
    classes.sort();
    assert_eq!(
        classes,
        vec!["per_network".to_string(), "session".to_string()]
    );
}

#[tokio::test]
async fn emitters_exact_class_filter_selects_only_that_class() {
    let (app, cookie) = app_with_one_of_each().await;

    let resp = get_with_cookie(&app, "/api/emitters?mac_persistence=session", &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["attributes"]["mac_persistence"], "session");
}

#[tokio::test]
async fn unknown_persistence_token_is_a_400_not_an_empty_result() {
    let (app, cookie) = app_with_one_of_each().await;

    for bad in ["randomised", "longterm", "true"] {
        let resp = get_with_cookie(
            &app,
            &format!("/api/emitters?mac_persistence={bad}"),
            &cookie,
        )
        .await;
        assert_status(&resp, StatusCode::BAD_REQUEST);
    }

    let resp = get_with_cookie(&app, "/api/emissions?mac_persistence=nope", &cookie).await;
    assert_status(&resp, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn emissions_filter_matches_via_the_emitters_class() {
    let (app, cookie) = app_with_one_of_each().await;

    let resp = get_with_cookie(
        &app,
        "/api/emissions?mac_persistence=randomized-longterm",
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(
        body["total"], 2,
        "one emission each from the per_network and session emitters"
    );
}

// -- retention config validation ----------------------------------------

#[tokio::test]
async fn data_source_accepts_every_retention_level_and_the_age_out_flag() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::default())).await;
    let cookie = login(&app).await;

    for (i, level) in [
        "stable",
        "per_network",
        "session",
        "ephemeral",
        "unlinkable",
    ]
    .iter()
    .enumerate()
    {
        let body = json!({
            "kind": "wifi",
            "mode": "monitor",
            "interface": format!("wlan{i}"),
            "config": {"mac_retention_level": level, "age_out_ephemeral": true},
        });
        let resp =
            post_json_with_cookie(&app, "/api/data-sources", &body.to_string(), &cookie).await;
        assert_status(&resp, StatusCode::CREATED);
    }
}

#[tokio::test]
async fn data_source_rejects_an_unknown_retention_level() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::default())).await;
    let cookie = login(&app).await;

    let body = json!({
        "kind": "wifi", "mode": "monitor", "interface": "wlan0",
        "config": {"mac_retention_level": "randomized"},
    });
    let resp = post_json_with_cookie(&app, "/api/data-sources", &body.to_string(), &cookie).await;
    assert_status(&resp, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn data_source_rejects_retention_keys_on_a_kind_without_randomized_addresses() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::default())).await;
    let cookie = login(&app).await;

    // GPS has no addresses at all -- accepting the key would look like the
    // operator had limited retention when nothing is being filtered.
    let body = json!({
        "kind": "gps", "mode": "gpsd",
        "config": {"host": "localhost", "port": 2947, "mac_retention_level": "session"},
    });
    let resp = post_json_with_cookie(&app, "/api/data-sources", &body.to_string(), &cookie).await;
    assert_status(&resp, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn omitting_the_retention_level_keeps_the_store_everything_default() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::default())).await;
    let cookie = login(&app).await;

    let body = json!({
        "kind": "wifi", "mode": "monitor", "interface": "wlan0",
        "config": {"auto_create_emitters": true, "age_out_ephemeral": false},
    });
    let resp = post_json_with_cookie(&app, "/api/data-sources", &body.to_string(), &cookie).await;
    assert_status(&resp, StatusCode::CREATED);
    let created = body_json(resp).await;
    assert!(
        created["config"].get("mac_retention_level").is_none(),
        "an absent level is what 'store everything' means; it must not be materialized"
    );
}
