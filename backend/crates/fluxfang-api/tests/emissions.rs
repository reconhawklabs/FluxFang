//! Task 6.3: `GET /api/emissions` — filter/paginate emissions, driven end
//! to end through the HTTP API. Seeds emissions directly via
//! `EmissionRepo::insert` against the test app's own isolated pool (same
//! pattern `data_sources.rs`'s tests use), then exercises the endpoint.

use std::sync::Arc;

use axum::http::StatusCode;
use chrono::{TimeZone, Utc};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use fluxfang_api::capture::MockCapturerFactory;
use fluxfang_db::models::{NewDataSource, NewEmission};
use fluxfang_db::{DataSourceRepo, EmissionRepo, SessionRepo};

mod common;
use common::{
    assert_status, body_json, get, get_with_cookie, post_json, post_json_with_cookie,
    post_with_cookie, session_cookie, test_app_with_factory,
};

/// Log in against a fresh app and return its session cookie, running setup
/// first since a fresh instance has no password configured yet.
async fn login(app: &axum::Router) -> String {
    common::post_json(app, "/api/setup", r#"{"password":"pw123456"}"#).await;
    let resp = common::post_json(app, "/api/login", r#"{"password":"pw123456"}"#).await;
    assert_status(&resp, StatusCode::OK);
    session_cookie(&resp)
}

async fn seed_data_source(pool: &PgPool) -> Uuid {
    DataSourceRepo::insert(pool, NewDataSource::wifi_monitor("wlan0"))
        .await
        .expect("seed wifi data_source")
        .id
}

async fn seed_session(pool: &PgPool) -> Uuid {
    SessionRepo::close_active(pool)
        .await
        .expect("self-heal: close any active survey_session");
    SessionRepo::open(pool)
        .await
        .expect("seed survey_session")
        .id
}

async fn insert_wifi(
    pool: &PgPool,
    ds: Uuid,
    session: Uuid,
    bssid: &str,
    ssid: &str,
    channel: i64,
    observed_at: chrono::DateTime<Utc>,
) -> Uuid {
    let new = NewEmission {
        observed_at,
        ..NewEmission::wifi(
            ds,
            session,
            json!({"bssid": bssid, "ssid": ssid, "channel": channel}),
        )
    };
    EmissionRepo::insert(pool, new)
        .await
        .expect("insert seed emission")
        .id
}

/// (a) `?cond=channel:gte:6` returns only the channel-6+ emissions, with a
/// `total` matching the filtered count (not the unfiltered one).
#[tokio::test]
async fn cond_channel_gte_filters_to_matching_channel_only() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;
    let ds = seed_data_source(&pool).await;
    let session = seed_session(&pool).await;
    let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();

    let low = insert_wifi(&pool, ds, session, "aa:aa:aa:aa:aa:aa", "Home", 1, base).await;
    let high = insert_wifi(
        &pool,
        ds,
        session,
        "bb:bb:bb:bb:bb:bb",
        "Office",
        6,
        base + chrono::Duration::seconds(1),
    )
    .await;

    let resp = get_with_cookie(&app, "/api/emissions?cond=channel:gte:6", &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let body = body_json(resp).await;

    assert_eq!(body["total"], 1, "body: {body}");
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], high.to_string());
    assert!(items.iter().all(|i| i["id"] != low.to_string()));
}

/// (b) `?q=Free` substring-matches the SSID inside `payload`.
#[tokio::test]
async fn q_param_substring_matches_ssid_in_payload() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;
    let ds = seed_data_source(&pool).await;
    let session = seed_session(&pool).await;
    let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();

    let free = insert_wifi(&pool, ds, session, "aa:aa:aa:aa:aa:aa", "FreeWifi", 1, base).await;
    insert_wifi(
        &pool,
        ds,
        session,
        "bb:bb:bb:bb:bb:bb",
        "HomeNetwork",
        1,
        base + chrono::Duration::seconds(1),
    )
    .await;

    let resp = get_with_cookie(&app, "/api/emissions?q=Free", &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let body = body_json(resp).await;

    assert_eq!(body["total"], 1, "body: {body}");
    let items = body["items"].as_array().unwrap();
    assert_eq!(items[0]["id"], free.to_string());
}

/// (c) `?limit=1&offset=1` returns the second row (by `observed_at DESC`)
/// while `total` still reflects the full, unpaginated count.
#[tokio::test]
async fn pagination_limit_and_offset_return_correct_page_and_full_total() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;
    let ds = seed_data_source(&pool).await;
    let session = seed_session(&pool).await;
    let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();

    let mut ids = Vec::new();
    for i in 0..3i64 {
        let id = insert_wifi(
            &pool,
            ds,
            session,
            "aa:aa:aa:aa:aa:aa",
            "Home",
            i,
            base + chrono::Duration::seconds(i),
        )
        .await;
        ids.push(id);
    }
    // observed_at DESC -> newest (index 2) first, then 1, then 0.
    let expected_second = ids[1];

    let resp = get_with_cookie(&app, "/api/emissions?limit=1&offset=1", &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let body = body_json(resp).await;

    assert_eq!(body["total"], 3, "body: {body}");
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], expected_second.to_string());
}

/// (d) An invalid/unknown field name in `cond` is a `400`, not a `500`.
#[tokio::test]
async fn cond_with_unknown_field_is_bad_request_not_server_error() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let resp = get_with_cookie(&app, "/api/emissions?cond=notafield:eq:x", &cookie).await;
    assert_status(&resp, StatusCode::BAD_REQUEST);
}

/// (e) An op that's invalid for a known field (`ssid` is text, `gte` needs
/// a number) is also a `400`.
#[tokio::test]
async fn cond_with_invalid_op_for_field_is_bad_request() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let resp = get_with_cookie(&app, "/api/emissions?cond=ssid:gte:x", &cookie).await;
    assert_status(&resp, StatusCode::BAD_REQUEST);
}

/// (f) The endpoint is behind auth like every other protected route.
#[tokio::test]
async fn emissions_endpoint_requires_auth() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;

    assert_status(&get(&app, "/api/emissions").await, StatusCode::UNAUTHORIZED);
}

/// A malformed `cond` (missing the `value` segment) is a `400`.
#[tokio::test]
async fn cond_missing_value_segment_is_bad_request() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let resp = get_with_cookie(&app, "/api/emissions?cond=channel:gte", &cookie).await;
    assert_status(&resp, StatusCode::BAD_REQUEST);
}

/// A malformed `bbox` (wrong element count) is a `400`.
#[tokio::test]
async fn malformed_bbox_is_bad_request() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let resp = get_with_cookie(&app, "/api/emissions?bbox=1,2,3", &cookie).await;
    assert_status(&resp, StatusCode::BAD_REQUEST);
}

/// A malformed `time_from` (not RFC3339) is a `400`.
#[tokio::test]
async fn malformed_time_from_is_bad_request() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let resp = get_with_cookie(&app, "/api/emissions?time_from=not-a-date", &cookie).await;
    assert_status(&resp, StatusCode::BAD_REQUEST);
}

/// More than the max allowed `cond` params is a `400`.
#[tokio::test]
async fn too_many_cond_params_is_bad_request() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let conds: Vec<String> = (0..25).map(|i| format!("cond=channel:eq:{i}")).collect();
    let uri = format!("/api/emissions?{}", conds.join("&"));

    let resp = get_with_cookie(&app, &uri, &cookie).await;
    assert_status(&resp, StatusCode::BAD_REQUEST);
}

/// An `in` array with an absurd number of elements is a `400` rather than
/// being allowed to blow past Postgres's bind-count ceiling.
#[tokio::test]
async fn oversized_in_array_is_bad_request() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let elements: Vec<String> = (0..1500).map(|i| i.to_string()).collect();
    let value = format!("[{}]", elements.join(","));
    let uri = format!("/api/emissions?cond=channel:in:{value}");

    let resp = get_with_cookie(&app, &uri, &cookie).await;
    assert_status(&resp, StatusCode::BAD_REQUEST);
}

/// `limit` above the max is clamped rather than rejected, and still returns
/// `200`.
#[tokio::test]
async fn limit_above_max_is_clamped_not_rejected() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;
    let ds = seed_data_source(&pool).await;
    let session = seed_session(&pool).await;
    let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    insert_wifi(&pool, ds, session, "aa:aa:aa:aa:aa:aa", "Home", 1, base).await;

    let resp = get_with_cookie(&app, "/api/emissions?limit=999999", &cookie).await;
    assert_status(&resp, StatusCode::OK);
}

// ---------------------------------------------------------------------
// Phase 1c: POST /api/emissions/bulk-delete and POST /api/emissions/clear.
// ---------------------------------------------------------------------

/// `POST /api/emissions/bulk-delete {ids:[a,b]}` deletes exactly those two
/// rows, leaves the third alone, and reports `{deleted: 2}` — confirmed via
/// a subsequent `GET` no longer listing the deleted ids.
#[tokio::test]
async fn bulk_delete_removes_only_listed_ids_and_reports_count() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;
    let ds = seed_data_source(&pool).await;
    let session = seed_session(&pool).await;
    let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();

    let a = insert_wifi(&pool, ds, session, "aa:aa:aa:aa:aa:aa", "A", 1, base).await;
    let b = insert_wifi(
        &pool,
        ds,
        session,
        "bb:bb:bb:bb:bb:bb",
        "B",
        1,
        base + chrono::Duration::seconds(1),
    )
    .await;
    insert_wifi(
        &pool,
        ds,
        session,
        "cc:cc:cc:cc:cc:cc",
        "C",
        1,
        base + chrono::Duration::seconds(2),
    )
    .await;

    let body = json!({"ids": [a, b]}).to_string();
    let resp = post_json_with_cookie(&app, "/api/emissions/bulk-delete", &body, &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let resp_body = body_json(resp).await;
    assert_eq!(resp_body["deleted"], 2, "body: {resp_body}");

    let list_resp = get_with_cookie(&app, "/api/emissions", &cookie).await;
    let list = body_json(list_resp).await;
    assert_eq!(list["total"], 1, "body: {list}");
    let remaining_ids: Vec<String> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_string())
        .collect();
    assert!(!remaining_ids.contains(&a.to_string()));
    assert!(!remaining_ids.contains(&b.to_string()));
}

/// An empty `ids` list deletes nothing and is not an error.
#[tokio::test]
async fn bulk_delete_with_empty_ids_deletes_nothing() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;
    let ds = seed_data_source(&pool).await;
    let session = seed_session(&pool).await;
    let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    insert_wifi(&pool, ds, session, "aa:aa:aa:aa:aa:aa", "A", 1, base).await;

    let body = json!({"ids": []}).to_string();
    let resp = post_json_with_cookie(&app, "/api/emissions/bulk-delete", &body, &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let resp_body = body_json(resp).await;
    assert_eq!(resp_body["deleted"], 0, "body: {resp_body}");

    let list_resp = get_with_cookie(&app, "/api/emissions", &cookie).await;
    let list = body_json(list_resp).await;
    assert_eq!(list["total"], 1, "body: {list}");
}

/// `POST /api/emissions/clear` deletes every emission and reports the total
/// count, confirmed via a subsequent `GET` showing an empty list.
#[tokio::test]
async fn clear_deletes_all_emissions() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;
    let ds = seed_data_source(&pool).await;
    let session = seed_session(&pool).await;
    let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();

    insert_wifi(&pool, ds, session, "aa:aa:aa:aa:aa:aa", "A", 1, base).await;
    insert_wifi(
        &pool,
        ds,
        session,
        "bb:bb:bb:bb:bb:bb",
        "B",
        1,
        base + chrono::Duration::seconds(1),
    )
    .await;

    let resp = post_with_cookie(&app, "/api/emissions/clear", &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let resp_body = body_json(resp).await;
    assert_eq!(resp_body["deleted"], 2, "body: {resp_body}");

    let list_resp = get_with_cookie(&app, "/api/emissions", &cookie).await;
    let list = body_json(list_resp).await;
    assert_eq!(list["total"], 0, "body: {list}");
    assert_eq!(list["items"].as_array().unwrap().len(), 0, "body: {list}");
}

/// Both bulk-delete and clear are behind auth.
#[tokio::test]
async fn bulk_delete_and_clear_require_auth() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;

    assert_status(
        &post_json(&app, "/api/emissions/bulk-delete", r#"{"ids":[]}"#).await,
        StatusCode::UNAUTHORIZED,
    );
    assert_status(
        &post_json(&app, "/api/emissions/clear", "").await,
        StatusCode::UNAUTHORIZED,
    );
}

/// `?sensor_id=<id>` filters to a single sensor's emissions, and
/// `GET /api/emissions/sensor-ids` lists the distinct sensors present.
#[tokio::test]
async fn filters_by_sensor_id_and_lists_sensor_ids() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;
    let ds = seed_data_source(&pool).await;
    let session = seed_session(&pool).await;
    let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();

    // One local capture, one forwarded from a distributed sensor "frontgate".
    insert_wifi(&pool, ds, session, "aa:aa:aa:aa:aa:aa", "Home", 1, base).await;
    let remote = EmissionRepo::insert(
        &pool,
        NewEmission {
            sensor_id: "frontgate".to_string(),
            ..NewEmission::wifi(ds, session, json!({"bssid": "bb:bb:bb:bb:bb:bb", "channel": 1}))
        },
    )
    .await
    .expect("insert remote emission")
    .id;

    // Filter to the remote sensor only.
    let resp = get_with_cookie(&app, "/api/emissions?sensor_id=frontgate", &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["total"], 1, "body: {body}");
    assert_eq!(body["items"][0]["id"], remote.to_string());

    // Distinct sensor ids include both.
    let resp = get_with_cookie(&app, "/api/emissions/sensor-ids", &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let ids = body_json(resp).await;
    let ids = ids.as_array().unwrap();
    assert!(ids.iter().any(|v| v == "local"), "ids: {ids:?}");
    assert!(ids.iter().any(|v| v == "frontgate"), "ids: {ids:?}");
}

/// The sensor-ids endpoint is auth-gated like the rest of the emissions API.
#[tokio::test]
async fn sensor_ids_endpoint_requires_auth() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    assert_status(
        &get(&app, "/api/emissions/sensor-ids").await,
        StatusCode::UNAUTHORIZED,
    );
}
