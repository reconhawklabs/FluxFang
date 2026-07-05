//! Round-trip tests for `SessionRepo`.

mod common;

use common::fresh_pool;
use fluxfang_db::SessionRepo;

#[tokio::test]
async fn active_is_none_when_no_session_has_been_opened() {
    let pool = fresh_pool().await;
    assert!(SessionRepo::active(&pool).await.unwrap().is_none());
}

#[tokio::test]
async fn open_creates_a_session_with_no_ended_at() {
    let pool = fresh_pool().await;

    let session = SessionRepo::open(&pool).await.unwrap();
    assert!(session.ended_at.is_none());

    let active = SessionRepo::active(&pool).await.unwrap();
    assert_eq!(active.unwrap().id, session.id);
}

#[tokio::test]
async fn close_active_sets_ended_at_and_clears_active() {
    let pool = fresh_pool().await;
    let session = SessionRepo::open(&pool).await.unwrap();

    let closed = SessionRepo::close_active(&pool).await.unwrap();
    assert_eq!(closed.unwrap().id, session.id);

    assert!(SessionRepo::active(&pool).await.unwrap().is_none());
}

#[tokio::test]
async fn close_active_is_none_when_nothing_is_open() {
    let pool = fresh_pool().await;
    assert!(SessionRepo::close_active(&pool).await.unwrap().is_none());
}

/// Regression test for the self-heal fix: `close_active` must close
/// *every* row with `ended_at IS NULL`, not just the most recent. Since
/// Task 5.1 also added a DB-level partial unique index preventing more
/// than one such row from ever being *inserted* normally (see
/// `0002_single_active_session.sql`), this test drops that index first to
/// reconstruct the "multiple dangling open sessions" state the old code
/// mishandled (e.g. from data that predates the index, or any write path
/// that bypasses `SessionRepo`) — this test's schema is thrown away
/// afterward (per-test isolated schema), so dropping the index here has
/// no effect on any other test.
#[tokio::test]
async fn close_active_closes_every_open_session_not_just_the_newest() {
    let pool = fresh_pool().await;
    sqlx::query("DROP INDEX one_active_session")
        .execute(&pool)
        .await
        .unwrap();

    let older = SessionRepo::open(&pool).await.unwrap();
    let newer = SessionRepo::open(&pool).await.unwrap();
    assert!(SessionRepo::active(&pool).await.unwrap().is_some());

    let returned = SessionRepo::close_active(&pool).await.unwrap().unwrap();
    assert_eq!(
        returned.id, newer.id,
        "returns the most-recently-started session"
    );

    // Both rows must now be closed.
    assert!(SessionRepo::active(&pool).await.unwrap().is_none());
    let (older_ended, newer_ended): (
        Option<chrono::DateTime<chrono::Utc>>,
        Option<chrono::DateTime<chrono::Utc>>,
    ) = {
        let row: (Option<chrono::DateTime<chrono::Utc>>,) =
            sqlx::query_as("SELECT ended_at FROM survey_session WHERE id = $1")
                .bind(older.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let row2: (Option<chrono::DateTime<chrono::Utc>>,) =
            sqlx::query_as("SELECT ended_at FROM survey_session WHERE id = $1")
                .bind(newer.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        (row.0, row2.0)
    };
    assert!(
        older_ended.is_some(),
        "older dangling-open session must be closed too"
    );
    assert!(newer_ended.is_some());
}

/// `0002_single_active_session.sql`'s partial unique index: a second row
/// with `ended_at IS NULL` must be rejected outright by Postgres, not just
/// cleaned up after the fact by `close_active`.
#[tokio::test]
async fn single_active_session_index_rejects_a_second_active_row() {
    let pool = fresh_pool().await;
    let _first = SessionRepo::open(&pool).await.unwrap();

    let err = sqlx::query("INSERT INTO survey_session (started_at) VALUES (now())")
        .execute(&pool)
        .await
        .unwrap_err();

    let db_err = err.as_database_error().expect("should be a database error");
    assert_eq!(db_err.code().as_deref(), Some("23505"), "unique_violation");
}

/// `one_active_session` is a *partial* index (`WHERE ended_at IS NULL`):
/// it must not constrain *closed* rows at all. Two (or more) sessions with
/// `ended_at IS NOT NULL` existing at once -- the normal steady state once
/// a survey has run more than once -- must be perfectly fine.
#[tokio::test]
async fn multiple_closed_sessions_can_coexist() {
    let pool = fresh_pool().await;

    let first = SessionRepo::open(&pool).await.unwrap();
    SessionRepo::close_active(&pool).await.unwrap();
    let second = SessionRepo::open(&pool).await.unwrap();
    SessionRepo::close_active(&pool).await.unwrap();

    assert_ne!(first.id, second.id);
    assert!(SessionRepo::active(&pool).await.unwrap().is_none());

    let (first_ended, second_ended): (
        Option<chrono::DateTime<chrono::Utc>>,
        Option<chrono::DateTime<chrono::Utc>>,
    ) = {
        let row: (Option<chrono::DateTime<chrono::Utc>>,) =
            sqlx::query_as("SELECT ended_at FROM survey_session WHERE id = $1")
                .bind(first.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let row2: (Option<chrono::DateTime<chrono::Utc>>,) =
            sqlx::query_as("SELECT ended_at FROM survey_session WHERE id = $1")
                .bind(second.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        (row.0, row2.0)
    };
    assert!(first_ended.is_some());
    assert!(second_ended.is_some());

    let count: (i64,) =
        sqlx::query_as("SELECT count(*) FROM survey_session WHERE ended_at IS NOT NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        count.0, 2,
        "both closed sessions must persist simultaneously"
    );
}
