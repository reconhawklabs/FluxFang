//! `SessionManager`: opens/closes a `survey_session` — the bounded capture
//! period every emission and location fix is recorded under. It is now
//! **GPS-agnostic**: a session exists while *any* data source is running
//! (opened by the first start, closed by the last stop), so emissions always
//! have a session to persist under regardless of whether a location source
//! is present. Location production moved to [`crate::ingest::pump::LocationPump`];
//! the shared current fix lives in [`crate::ingest::location::LocationProvider`].

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

use fluxfang_capture::GpsFix;
use fluxfang_db::SessionRepo;
use sqlx::PgPool;
use uuid::Uuid;

/// Callback invoked once per `location_fix` row actually written, with the
/// fix that was just written (host-zone evaluation). Unchanged shape.
pub type HostZoneHook =
    Arc<dyn Fn(GpsFix) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// The default hook: does nothing.
pub fn no_op_hook() -> HostZoneHook {
    Arc::new(|_fix: GpsFix| Box::pin(async {}))
}

/// Opens/bounds a `survey_session`. Interior mutability so it is reachable
/// through a shared `Arc<SessionManager>` (ingest + pump both hold one).
pub struct SessionManager {
    pool: PgPool,
    session_id: Arc<RwLock<Option<Uuid>>>,
}

impl SessionManager {
    /// Self-heal any dangling active session, then open a fresh one.
    pub async fn open(pool: PgPool) -> Result<Self, sqlx::Error> {
        SessionRepo::close_active(&pool).await?;
        let session = SessionRepo::open(&pool).await?;
        Ok(Self {
            pool,
            session_id: Arc::new(RwLock::new(Some(session.id))),
        })
    }

    /// The session currently open, or `None` after [`close`].
    ///
    /// [`close`]: SessionManager::close
    pub fn current_session_id(&self) -> Option<Uuid> {
        *self.session_id.read().expect("session_id lock poisoned")
    }

    /// Close the active session. Idempotent. Takes `&self` (reached through a
    /// shared `Arc`).
    pub async fn close(&self) -> Result<(), sqlx::Error> {
        SessionRepo::close_active(&self.pool).await?;
        *self.session_id.write().expect("session_id lock poisoned") = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::fresh_pool;

    #[tokio::test]
    async fn open_creates_and_activates_a_session() {
        let pool = fresh_pool().await;
        let manager = SessionManager::open(pool.clone()).await.unwrap();
        let sid = manager.current_session_id().unwrap();
        let active = SessionRepo::active(&pool).await.unwrap().unwrap();
        assert_eq!(active.id, sid);
    }

    #[tokio::test]
    async fn open_self_heals_a_dangling_active_session() {
        let pool = fresh_pool().await;
        let dangling = SessionRepo::open(&pool).await.unwrap();
        let manager = SessionManager::open(pool.clone()).await.unwrap();
        let active = SessionRepo::active(&pool).await.unwrap().unwrap();
        assert_eq!(active.id, manager.current_session_id().unwrap());
        assert_ne!(active.id, dangling.id);
    }

    #[tokio::test]
    async fn close_ends_the_session() {
        let pool = fresh_pool().await;
        let manager = SessionManager::open(pool.clone()).await.unwrap();
        let sid = manager.current_session_id().unwrap();
        manager.close().await.unwrap();
        assert!(manager.current_session_id().is_none());
        let row = sqlx::query_as::<_, (Option<chrono::DateTime<chrono::Utc>>,)>(
            "SELECT ended_at FROM survey_session WHERE id = $1",
        )
        .bind(sid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(row.0.is_some());
    }
}
