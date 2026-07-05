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
