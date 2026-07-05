use axum::http::StatusCode;

mod common;
use common::{
    assert_status, body_json, get, get_with_cookie, post_json, post_json_with_cookie,
    session_cookie, test_app,
};

async fn needs_setup(app: &axum::Router) -> bool {
    let resp = get(app, "/api/setup/status").await;
    assert_status(&resp, StatusCode::OK);
    body_json(resp).await["needs_setup"].as_bool().unwrap()
}

/// The brief's core flow: fresh instance needs setup -> setup -> no longer
/// needs setup -> protected route rejected without a cookie -> login ->
/// protected route accepted with the cookie login handed back.
#[tokio::test]
async fn setup_then_login_flow() {
    let app = test_app().await;

    assert!(needs_setup(&app).await, "fresh instance should need setup");

    let resp = post_json(&app, "/api/setup", r#"{"password":"pw123456"}"#).await;
    assert_status(&resp, StatusCode::OK);

    assert!(
        !needs_setup(&app).await,
        "setup should have cleared needs_setup"
    );

    // Protected route rejected without any cookie at all.
    let resp = get(&app, "/api/entities").await;
    assert_status(&resp, StatusCode::UNAUTHORIZED);

    // Setup itself doesn't leave *this* client authenticated (no cookie was
    // captured from it above) — login explicitly and use its cookie.
    let resp = post_json(&app, "/api/login", r#"{"password":"pw123456"}"#).await;
    assert_status(&resp, StatusCode::OK);
    let cookie = session_cookie(&resp);

    let resp = get_with_cookie(&app, "/api/entities", &cookie).await;
    assert_status(&resp, StatusCode::OK);
    assert_eq!(body_json(resp).await, serde_json::json!([]));
}

/// `/api/setup` also logs the caller in directly (no separate login round
/// trip required right after first-run setup).
#[tokio::test]
async fn setup_logs_in_directly() {
    let app = test_app().await;

    let resp = post_json(&app, "/api/setup", r#"{"password":"pw123456"}"#).await;
    assert_status(&resp, StatusCode::OK);
    let cookie = session_cookie(&resp);

    let resp = get_with_cookie(&app, "/api/entities", &cookie).await;
    assert_status(&resp, StatusCode::OK);
}

#[tokio::test]
async fn setup_rejected_when_already_configured() {
    let app = test_app().await;

    let resp = post_json(&app, "/api/setup", r#"{"password":"pw123456"}"#).await;
    assert_status(&resp, StatusCode::OK);

    let resp = post_json(&app, "/api/setup", r#"{"password":"different1"}"#).await;
    assert_status(&resp, StatusCode::CONFLICT);

    // Original password should still be the one that works.
    let resp = post_json(&app, "/api/login", r#"{"password":"pw123456"}"#).await;
    assert_status(&resp, StatusCode::OK);
}

#[tokio::test]
async fn setup_rejects_empty_and_absurdly_long_passwords() {
    let app = test_app().await;

    let resp = post_json(&app, "/api/setup", r#"{"password":""}"#).await;
    assert_status(&resp, StatusCode::BAD_REQUEST);

    let too_long = "a".repeat(1025);
    let resp = post_json(
        &app,
        "/api/setup",
        &format!(r#"{{"password":"{too_long}"}}"#),
    )
    .await;
    assert_status(&resp, StatusCode::BAD_REQUEST);

    // Still needs setup — neither rejected attempt should have taken.
    assert!(needs_setup(&app).await);
}

#[tokio::test]
async fn login_with_wrong_password_is_401() {
    let app = test_app().await;

    post_json(&app, "/api/setup", r#"{"password":"pw123456"}"#).await;

    let resp = post_json(&app, "/api/login", r#"{"password":"wrong-password"}"#).await;
    assert_status(&resp, StatusCode::UNAUTHORIZED);

    // And the protected route stays locked out.
    let resp = get(&app, "/api/entities").await;
    assert_status(&resp, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn logout_then_protected_route_401_again() {
    let app = test_app().await;

    post_json(&app, "/api/setup", r#"{"password":"pw123456"}"#).await;
    let resp = post_json(&app, "/api/login", r#"{"password":"pw123456"}"#).await;
    let cookie = session_cookie(&resp);

    let resp = get_with_cookie(&app, "/api/entities", &cookie).await;
    assert_status(&resp, StatusCode::OK);

    let resp = post_json_with_cookie(&app, "/api/logout", "", &cookie).await;
    assert_status(&resp, StatusCode::OK);

    let resp = get_with_cookie(&app, "/api/entities", &cookie).await;
    assert_status(&resp, StatusCode::UNAUTHORIZED);
}

/// A simple sanity check on the rate limiter: enough failed attempts in a
/// row eventually get `429` instead of `401`, distinguishing "wrong
/// password" from "you're being throttled".
#[tokio::test]
async fn repeated_failed_logins_get_rate_limited() {
    let app = test_app().await;
    post_json(&app, "/api/setup", r#"{"password":"pw123456"}"#).await;

    let mut saw_429 = false;
    for _ in 0..20 {
        let resp = post_json(&app, "/api/login", r#"{"password":"wrong-password"}"#).await;
        match resp.status() {
            StatusCode::UNAUTHORIZED => {}
            StatusCode::TOO_MANY_REQUESTS => {
                saw_429 = true;
                break;
            }
            other => panic!("unexpected status {other}"),
        }
    }
    assert!(
        saw_429,
        "expected to eventually see 429 after repeated failed logins"
    );
}
