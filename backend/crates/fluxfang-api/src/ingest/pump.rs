//! `LocationPump`: pulls fixes from one [`LocationSource`], updates the
//! shared [`LocationProvider`], logs the host track into `location_fix`
//! (throttled by `write_interval`), and fires the host-zone hook — for as
//! long as the source yields fixes. On exhaustion / device disconnect
//! (`next_fix()` -> `None`) it runs `on_exhausted` (which reports the source
//! as failed) and stops. It never touches the session and never clears the
//! provider — a lost signal ages out to NULL via the freshness gate; only a
//! user-initiated stop clears the provider (see `CaptureSupervisor`).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use fluxfang_capture::LocationSource;
use fluxfang_db::models::NewLocationFix;
use fluxfang_db::LocationRepo;
use sqlx::PgPool;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::location::LocationProvider;
use super::session::{HostZoneHook, SessionManager};

/// Ran once when the pump's source exhausts/disconnects. Reports failure.
pub type OnExhausted = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// A running location pump's teardown handle.
pub struct LocationPump {
    handle: JoinHandle<()>,
}

impl LocationPump {
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        pool: PgPool,
        mut source: Box<dyn LocationSource>,
        provider: Arc<LocationProvider>,
        session: Arc<SessionManager>,
        write_interval: Duration,
        hook: HostZoneHook,
        on_exhausted: OnExhausted,
    ) -> Self {
        let handle = tokio::spawn(async move {
            let mut last_write: Option<Instant> = None;
            while let Some(fix) = source.next_fix().await {
                provider.update(fix.clone());

                let should_write = write_interval.is_zero()
                    || last_write.is_none_or(|t| t.elapsed() >= write_interval);
                if !should_write {
                    continue;
                }
                let Some(session_id) = session.current_session_id() else {
                    // Session closed out from under us — skip the track write
                    // (FK would fail) but keep feeding the provider.
                    continue;
                };
                let new_fix = NewLocationFix {
                    session_id,
                    observed_at: fix.at,
                    location: (fix.lon, fix.lat),
                    altitude: fix.altitude,
                    speed: fix.speed,
                    heading: fix.heading,
                    fix_quality: Some(fix.quality.to_string()),
                };
                match LocationRepo::insert_fix(&pool, new_fix).await {
                    Ok(_) => {
                        last_write = Some(Instant::now());
                        hook(fix).await;
                    }
                    Err(e) => {
                        eprintln!("LocationPump: failed to write location_fix: {e}");
                    }
                }
            }
            // Source exhausted / device disconnected.
            on_exhausted().await;
        });
        Self { handle }
    }

    /// Stop the pump task. Used on a user-initiated stop (which also aborts
    /// before `on_exhausted` can fire, so a deliberate stop is not reported
    /// as a failure).
    pub fn abort(self) {
        self.handle.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::session::no_op_hook;
    use crate::test_support::fresh_pool;
    use chrono::TimeZone;
    use fluxfang_capture::mock::MockGps;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn pump_feeds_provider_and_reports_exhaustion() {
        let pool = fresh_pool().await;
        let session = Arc::new(SessionManager::open(pool.clone()).await.unwrap());
        let provider = Arc::new(LocationProvider::new());

        let base = base_time();
        let fixes =
            MockGps::synthetic_track(base, -122.0, 37.0, 0.001, chrono::Duration::seconds(1), 3);
        let source = Box::new(MockGps::new(fixes.clone()));

        let fired = Arc::new(AtomicUsize::new(0));
        let fired_cb = fired.clone();
        let on_exhausted: OnExhausted = Arc::new(move || {
            let fired = fired_cb.clone();
            Box::pin(async move {
                fired.fetch_add(1, Ordering::SeqCst);
            })
        });

        let pump = LocationPump::start(
            pool.clone(),
            source,
            provider.clone(),
            session.clone(),
            Duration::ZERO,
            no_op_hook(),
            on_exhausted,
        );

        // MockGps track is finite: the pump drains it, then fires on_exhausted.
        tokio::time::timeout(Duration::from_secs(5), async {
            while fired.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("on_exhausted must fire once the finite source drains");

        // Provider holds the last fix; session was NOT closed.
        assert_eq!(provider.latest_raw().unwrap(), *fixes.last().unwrap());
        assert!(session.current_session_id().is_some());

        pump.abort();
    }

    fn base_time() -> chrono::DateTime<chrono::Utc> {
        chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
    }
}
