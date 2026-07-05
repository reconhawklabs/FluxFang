//! Shared test harness for `fluxfang-api` integration tests.
//!
//! Builds a fresh, schema-isolated `AppState`/`Router` per test using the
//! same one-Postgres-schema-per-test isolation strategy as `fluxfang-db`'s
//! own test harness (see `fluxfang-db/tests/common/mod.rs` for the full
//! rationale — this is a trimmed copy, since `fluxfang-db`'s test-only
//! helpers aren't part of its public API and so aren't reusable from here
//! directly). Isolation matters even more for this crate's tests than most:
//! `app_config` is a process-wide singleton row, and several `auth.rs`
//! tests assert on its "has a password been set yet?" state, which would
//! race against each other (and against `health.rs`'s test) if they shared
//! a schema.

#![allow(dead_code)]

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use axum::Router;
use fluxfang_api::{app, AppState};
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use sqlx::{Executor, PgPool};
use tokio::sync::OnceCell;
use tower::ServiceExt;
use uuid::Uuid;

static SWEEP_DONE: OnceCell<()> = OnceCell::const_new();

/// Best-effort cleanup of `test_*` schemas left behind by earlier test
/// runs of this crate's test binaries. See `fluxfang-db`'s equivalent for
/// why this ordering (sweep once, before this binary creates its own
/// schema) is safe under `cargo test`'s default execution model.
async fn sweep_leftover_test_schemas(database_url: &str) {
    let Ok(admin) = PgPoolOptions::new()
        .max_connections(1)
        .connect(database_url)
        .await
    else {
        return;
    };

    let schemas: Result<Vec<(String,)>, _> = sqlx::query_as(
        "SELECT schema_name FROM information_schema.schemata \
         WHERE schema_name LIKE 'test\\_%' ESCAPE '\\'",
    )
    .fetch_all(&admin)
    .await;

    if let Ok(schemas) = schemas {
        for (schema,) in schemas {
            let _ = admin
                .execute(format!(r#"DROP SCHEMA IF EXISTS "{schema}" CASCADE"#).as_str())
                .await;
        }
    }

    admin.close().await;
}

/// Build a pool bound to a fresh, isolated schema with all migrations
/// applied.
async fn fresh_pool() -> PgPool {
    let database_url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set for fluxfang-api tests (see task-2.2-report.md)");

    SWEEP_DONE
        .get_or_init(|| sweep_leftover_test_schemas(&database_url))
        .await;

    let schema = format!("test_{}", Uuid::new_v4().simple());

    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("connect to DATABASE_URL to create test schema");
    admin
        .execute(format!(r#"CREATE SCHEMA "{schema}""#).as_str())
        .await
        .expect("create isolated test schema");
    admin.close().await;

    let search_path = format!(r#""{schema}", public"#);
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .after_connect(move |conn, _meta| {
            let search_path = search_path.clone();
            Box::pin(async move {
                conn.execute(format!("SET search_path TO {search_path}").as_str())
                    .await?;
                Ok(())
            })
        })
        .connect(&database_url)
        .await
        .expect("connect to DATABASE_URL with isolated search_path");

    fluxfang_db::run_migrations(&pool)
        .await
        .expect("run migrations into isolated test schema");

    pool
}

/// A fully wired app (fresh isolated DB schema + fresh in-memory session
/// store + fresh login rate limiter) for one test.
pub async fn test_app() -> Router {
    app(AppState::new(fresh_pool().await))
}

/// Run a request against the app via `tower::ServiceExt::oneshot`.
pub async fn call(app: &Router, req: Request<Body>) -> Response<Body> {
    app.clone().oneshot(req).await.expect("request failed")
}

pub async fn get(app: &Router, uri: &str) -> Response<Body> {
    call(
        app,
        Request::builder()
            .method("GET")
            .uri(uri)
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

pub async fn get_with_cookie(app: &Router, uri: &str, cookie: &str) -> Response<Body> {
    call(
        app,
        Request::builder()
            .method("GET")
            .uri(uri)
            .header("cookie", cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

pub async fn post_json(app: &Router, uri: &str, body: &str) -> Response<Body> {
    call(
        app,
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}

pub async fn post_json_with_cookie(
    app: &Router,
    uri: &str,
    body: &str,
    cookie: &str,
) -> Response<Body> {
    call(
        app,
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .header("cookie", cookie)
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}

/// Extract the `name=value` pair from a response's `Set-Cookie` header,
/// dropping cookie attributes (`Path`, `HttpOnly`, ...) — that's all a real
/// client would echo back in its own `Cookie` request header.
pub fn session_cookie(resp: &Response<Body>) -> String {
    resp.headers()
        .get(axum::http::header::SET_COOKIE)
        .expect("response should set a session cookie")
        .to_str()
        .expect("Set-Cookie header should be valid UTF-8")
        .split(';')
        .next()
        .expect("Set-Cookie header should have at least a name=value pair")
        .to_string()
}

pub async fn body_json(resp: Response<Body>) -> Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("read response body");
    serde_json::from_slice(&bytes).expect("response body should be valid JSON")
}

pub fn assert_status(resp: &Response<Body>, expected: StatusCode) {
    assert_eq!(
        resp.status(),
        expected,
        "expected status {expected}, got {}",
        resp.status()
    );
}
