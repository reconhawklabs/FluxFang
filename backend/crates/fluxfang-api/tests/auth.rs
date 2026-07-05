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

/// Regression test for the setup TOCTOU: `setup()` used to read
/// `password_hash().is_some()` and, separately, call an unconditional
/// upsert — two concurrent `POST /api/setup` requests could both observe
/// `None` from the read, both pass the check, and both upsert, with the
/// last write silently becoming the admin password (both requests would
/// return `200 OK`). The fix makes `AppConfigRepo::set_password_hash_if_unset`
/// a single atomic statement, so Postgres serializes the two requests on the
/// row and exactly one of them can "win".
///
/// This fires two concurrent setup requests with *different* passwords and
/// asserts exactly one succeeds (`200`) and the other is rejected (`409`),
/// and that only the winning candidate's password actually verifies
/// afterwards. Against the old unconditional-upsert code this test fails
/// (both requests return `200`, and login succeeds with the *second*
/// candidate rather than deterministically with just one of them).
#[tokio::test]
async fn concurrent_setup_requests_only_one_wins() {
    let app = test_app().await;

    let (resp_a, resp_b) = tokio::join!(
        post_json(&app, "/api/setup", r#"{"password":"candidate-aaaa"}"#),
        post_json(&app, "/api/setup", r#"{"password":"candidate-bbbb"}"#),
    );

    let statuses = [resp_a.status(), resp_b.status()];
    let ok_count = statuses.iter().filter(|s| **s == StatusCode::OK).count();
    let conflict_count = statuses
        .iter()
        .filter(|s| **s == StatusCode::CONFLICT)
        .count();

    assert_eq!(
        ok_count, 1,
        "expected exactly one concurrent setup request to succeed, got statuses {statuses:?}"
    );
    assert_eq!(
        conflict_count, 1,
        "expected exactly one concurrent setup request to be rejected as a conflict, got statuses {statuses:?}"
    );

    // Whichever candidate won, only that one's password should verify.
    let a_won = statuses[0] == StatusCode::OK;
    let (winner, loser) = if a_won {
        ("candidate-aaaa", "candidate-bbbb")
    } else {
        ("candidate-bbbb", "candidate-aaaa")
    };

    let resp = post_json(&app, "/api/login", &format!(r#"{{"password":"{winner}"}}"#)).await;
    assert_status(&resp, StatusCode::OK);

    let resp = post_json(&app, "/api/login", &format!(r#"{{"password":"{loser}"}}"#)).await;
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
