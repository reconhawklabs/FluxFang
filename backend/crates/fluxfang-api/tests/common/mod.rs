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

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use axum::Router;
use fluxfang_api::capture::CapturerFactory;
use fluxfang_api::{app, AppState};
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use sqlx::{Executor, PgPool};
use tokio::sync::OnceCell;
use tower::ServiceExt;
use uuid::Uuid;

/// Fixed test key for `AppState::with_capture` — this crate's integration
/// tests never exercise alert dispatch decryption, so any 32 bytes will do.
const TEST_SECRET_KEY: [u8; 32] = [0x33u8; 32];

static SWEEP_DONE: OnceCell<()> = OnceCell::const_new();

/// A leftover `test_*` schema is only swept once it's at least this old.
/// No single test (let alone a whole binary's run) should take anywhere
/// near this long, so a schema still around after this window has to
/// belong to a process that already exited without cleaning up after
/// itself — never one that's still in flight.
const SWEEP_MAX_AGE_MILLIS: u128 = 15 * 60 * 1000;

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after the UNIX epoch")
        .as_millis()
}

/// Parses the creation timestamp embedded in a `test_<epoch_millis>_<uuid>`
/// schema name (see [`fresh_pool`]). Schemas that don't match this scheme
/// return `None` and are left alone by the sweep rather than guessed at.
fn parse_created_millis(schema: &str) -> Option<u128> {
    let rest = schema.strip_prefix("test_")?;
    let (millis, _uuid) = rest.split_once('_')?;
    millis.parse().ok()
}

/// Best-effort cleanup of `test_*` schemas left behind by earlier test
/// runs of this crate's test binaries.
///
/// Age-gated, not merely ordering-gated: a schema is only dropped once
/// [`parse_created_millis`] shows it's older than [`SWEEP_MAX_AGE_MILLIS`].
/// A schema younger than that could belong to a *concurrently-running*
/// sibling process — another integration-test binary in this crate,
/// `fluxfang-api`'s in-crate unit tests, or `fluxfang-db`'s integration
/// tests, all of which run this same sweep independently against the same
/// database — that is actively migrating/using it, so this process
/// running its sweep first must not be enough justification to drop it.
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
        let now = now_millis();
        for (schema,) in schemas {
            let is_stale = matches!(
                parse_created_millis(&schema),
                Some(created) if now.saturating_sub(created) > SWEEP_MAX_AGE_MILLIS
            );
            if is_stale {
                let _ = admin
                    .execute(format!(r#"DROP SCHEMA IF EXISTS "{schema}" CASCADE"#).as_str())
                    .await;
            }
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

    let schema = format!("test_{}_{}", now_millis(), Uuid::new_v4().simple());

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

/// Same as [`test_app`], but with a caller-supplied `CapturerFactory` (e.g.
/// `fluxfang_api::capture::MockCapturerFactory`) wired into the
/// `CaptureSupervisor` instead of the real, hardware-touching one --
/// `data_sources.rs`'s start/stop tests need this so `POST
/// /api/data-sources/:id/start` never touches real wifi/gps hardware.
/// Returns the pool alongside the app so tests can assert directly against
/// the DB (e.g. `EmissionRepo::query`, `LocationRepo::list_for_session`)
/// without threading a second connection through `AppState`.
pub async fn test_app_with_factory(factory: Arc<dyn CapturerFactory>) -> (Router, PgPool) {
    let pool = fresh_pool().await;
    let state = AppState::with_capture(pool.clone(), TEST_SECRET_KEY, factory);
    (app(state), pool)
}

/// Build an `AppState` on a caller-supplied pool + factory (same fixed test
/// key as [`test_app_with_factory`]) *without* wrapping it in a router, so a
/// test can reach `state.capture` directly — e.g. to drive
/// `CaptureSupervisor::resume_running`, which has no HTTP route. Pair with
/// [`fresh_pool_shared`] to build two states on the *same* schema and model a
/// process restart: the in-memory supervisor state resets while the DB
/// persists.
pub fn state_with_factory(pool: PgPool, factory: Arc<dyn CapturerFactory>) -> AppState {
    AppState::with_capture(pool, TEST_SECRET_KEY, factory)
}

/// A fresh, schema-isolated, migrated pool — exposed so a test can hand the
/// *same* pool to two [`state_with_factory`] calls (see its doc comment).
pub async fn fresh_pool_shared() -> PgPool {
    fresh_pool().await
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

pub async fn patch_json_with_cookie(
    app: &Router,
    uri: &str,
    body: &str,
    cookie: &str,
) -> Response<Body> {
    call(
        app,
        Request::builder()
            .method("PATCH")
            .uri(uri)
            .header("content-type", "application/json")
            .header("cookie", cookie)
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}

pub async fn delete_with_cookie(app: &Router, uri: &str, cookie: &str) -> Response<Body> {
    call(
        app,
        Request::builder()
            .method("DELETE")
            .uri(uri)
            .header("cookie", cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

pub async fn post_with_cookie(app: &Router, uri: &str, cookie: &str) -> Response<Body> {
    call(
        app,
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("cookie", cookie)
            .body(Body::empty())
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

/// Spawn `app` on a real, ephemeral TCP port and return its address.
///
/// Every other helper here drives the router in-process via
/// `tower::ServiceExt::oneshot` (see [`call`]), which is enough for plain
/// request/response endpoints but can't perform an HTTP `Upgrade` (a
/// WebSocket handshake needs a genuine bidirectional byte stream to switch
/// protocols on top of, not just one request paired with one response) —
/// `tests/ws.rs` needs this to drive `GET /ws` with a real WS client
/// (`tokio-tungstenite`). The spawned `axum::serve` task is simply detached;
/// it's cleaned up when the test process exits, same as every other
/// fire-and-forget background task this crate's tests already spawn (e.g.
/// `CaptureSupervisor`'s reader tasks in `data_sources.rs`).
pub async fn spawn_server(app: Router) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind an ephemeral TCP port");
    let addr = listener.local_addr().expect("read the bound local addr");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("axum::serve should not fail");
    });
    addr
}
