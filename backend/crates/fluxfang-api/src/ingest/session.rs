//! `SessionManager`: bounds a continuous capture period into a
//! `survey_session` and logs the host's own GPS trajectory into
//! `location_fix` rows — the foundation for follow/stalker detection
//! later (a subject that stays suspiciously close to the host's own
//! logged track).
//!
//! ## Session bounding / self-heal
//!
//! [`SessionManager::open`] always closes whatever session is currently
//! active (`SessionRepo::close_active`, which itself now closes *every*
//! dangling-open row, see Task 5.1's fix there) before opening a fresh
//! one. This mirrors the DB-level backstop added in
//! `0002_single_active_session.sql` (a partial unique index making it
//! impossible to *persist* two active sessions) — the self-heal here is
//! what keeps the *application* from ever attempting to violate that
//! index in the first place.
//!
//! ## `latest_fix`/`current_session_id` concurrency
//!
//! Both are `Arc<std::sync::RwLock<..>>`, not `tokio::sync::RwLock`: every
//! critical section that touches them is a single, synchronous
//! read-or-assign statement with no `.await` inside the guard's lifetime,
//! so a blocking `std::sync::RwLock` is safe to use from async code here
//! (same idiom as `AppState::LoginLimiter`'s `std::sync::Mutex`) and lets
//! `current_session_id()`/`latest_fix()` stay plain synchronous methods —
//! matching the brief's own (non-`async`) signatures — rather than forcing
//! every caller to `.await` a trivial read.
//!
//! ## Cadence
//!
//! The background loop writes one `location_fix` row **per fix received**
//! by default (`SessionManagerConfig::write_interval = Duration::ZERO`).
//! GPS sources already emit at their own natural cadence (~1Hz for NMEA/
//! gpsd — see `fluxfang-capture`'s `gps` module), so writing on every fix
//! is the simplest correct behavior; there's no buffering/interpolation
//! need at this slice's scale. Setting `write_interval` to a positive
//! `Duration` throttles *persisted rows and host-zone hook invocations* to
//! at most one per interval (fixes arriving faster than that still update
//! `latest_fix()` immediately, so in-memory freshness is never throttled —
//! only what's durably logged and what triggers zone evaluation is).
//!
//! ## Host-zone hook
//!
//! Every time (and only when) a `location_fix` row is actually written,
//! [`HostZoneHook`] is invoked with the fix that was just written. This
//! task (5.1) only defines the hook's shape and wires the call site; Task
//! 5.4 supplies a real implementation that evaluates host zone enter/leave
//! transitions. Passing a callback instead of importing the zones module
//! directly keeps this task's ingest layer with zero dependency on 5.4's
//! (not-yet-written) code. [`no_op_hook`] is the default for callers that
//! don't care yet (e.g. this task's own tests).
//!
//! ## Inactivity gap
//!
//! If no fix arrives within `SessionManagerConfig::inactivity_gap`
//! (default 5 minutes) the session is treated as over and closed
//! automatically. GPS source *exhaustion* (`next_fix` returning `None` —
//! hardware disconnected, or a finite mock track ending) closes the
//! session immediately instead of waiting out the gap, since no more
//! fixes are possible either way; the gap only covers "source still
//! present but temporarily silent" (e.g. lost satellite lock).
//!
//! ## Time injection for tests
//!
//! `SessionManagerConfig::inactivity_gap` is the injection point: tests
//! never wait on the production 5-minute default, they construct a
//! `SessionManager` with a millisecond-scale gap instead and await the
//! loop's `JoinHandle` (via [`SessionManager::join`]) — deterministic
//! (the gap timer *always* fires once the source goes quiet, there's no
//! flaky race to get right) and fast, without depending on the wall-clock
//! minutes the real default uses.
//!
//! This deliberately does **not** use `tokio::time::pause()`/
//! `#[tokio::test(start_paused = true)]`, even though `tokio::time::sleep`
//! would otherwise respect it: pausing the whole runtime's clock races
//! against the real Postgres socket I/O these tests also perform (pool
//! connect/acquire has its own internal `tokio::time` deadline), and in
//! practice that combination reliably produced spurious `PoolTimedOut`
//! errors — the paused clock auto-advances past that deadline before the
//! real TCP handshake with Postgres completes. Short *real* durations for
//! a config field that's designed to be injectable sidesteps that
//! conflict entirely while still never touching the production value.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use fluxfang_capture::{GpsFix, GpsSource};
use fluxfang_db::models::NewLocationFix;
use fluxfang_db::{LocationRepo, SessionRepo};
use sqlx::PgPool;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use uuid::Uuid;

/// Callback invoked once per `location_fix` row actually written, with the
/// fix that was just written. Task 5.4 wires a real implementation
/// (`update_host_zones`) that evaluates host zone enter/leave transitions;
/// this crate/task has no dependency on that module, only on this shape.
pub type HostZoneHook =
    Arc<dyn Fn(GpsFix) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// The default hook: does nothing. Used whenever a caller (or a test) has
/// no interest in host-zone evaluation.
pub fn no_op_hook() -> HostZoneHook {
    Arc::new(|_fix: GpsFix| Box::pin(async {}))
}

/// Tunables for [`SessionManager`]. See module docs for the cadence/gap
/// semantics.
#[derive(Debug, Clone, Copy)]
pub struct SessionManagerConfig {
    /// Close the session if no fix arrives within this long. Default 5
    /// minutes.
    pub inactivity_gap: Duration,
    /// Minimum spacing between persisted `location_fix` rows (and hook
    /// invocations). `Duration::ZERO` (the default) means "write every
    /// fix".
    pub write_interval: Duration,
}

impl Default for SessionManagerConfig {
    fn default() -> Self {
        Self {
            inactivity_gap: Duration::from_secs(5 * 60),
            write_interval: Duration::ZERO,
        }
    }
}

/// Opens/bounds a `survey_session` and logs a `GpsSource`'s fixes into
/// `location_fix` for as long as it stays active. See module docs for the
/// full design.
pub struct SessionManager {
    pool: PgPool,
    session_id: Arc<RwLock<Option<Uuid>>>,
    latest_fix: Arc<RwLock<Option<GpsFix>>>,
    handle: Option<JoinHandle<()>>,
}

impl SessionManager {
    /// Self-heal (close any dangling active session), open a fresh
    /// `survey_session`, and spawn the background ingest loop pulling
    /// fixes from `gps` until it closes itself (inactivity gap / source
    /// exhaustion) or [`SessionManager::close`] is called.
    pub async fn open<G>(
        pool: PgPool,
        gps: G,
        config: SessionManagerConfig,
        hook: HostZoneHook,
    ) -> Result<Self, sqlx::Error>
    where
        G: GpsSource + Send + 'static,
    {
        SessionRepo::close_active(&pool).await?;
        let session = SessionRepo::open(&pool).await?;

        let session_id = Arc::new(RwLock::new(Some(session.id)));
        let latest_fix = Arc::new(RwLock::new(None));

        let loop_pool = pool.clone();
        let loop_session_id = session_id.clone();
        let loop_latest_fix = latest_fix.clone();
        let sid = session.id;
        let gap = config.inactivity_gap;
        let write_interval = config.write_interval;

        let handle = tokio::spawn(async move {
            run_ingest_loop(
                loop_pool,
                sid,
                gps,
                gap,
                write_interval,
                &loop_latest_fix,
                hook,
            )
            .await;
            *loop_session_id.write().expect("session_id lock poisoned") = None;
        });

        Ok(Self {
            pool,
            session_id,
            latest_fix,
            handle: Some(handle),
        })
    }

    /// The session currently being logged to, or `None` once the loop has
    /// ended (gap timeout, source exhaustion, or an explicit [`close`]).
    ///
    /// [`close`]: SessionManager::close
    pub fn current_session_id(&self) -> Option<Uuid> {
        *self.session_id.read().expect("session_id lock poisoned")
    }

    /// The most recent fix seen, kept in memory (updated on every fix,
    /// independent of `write_interval` throttling — see module docs).
    pub fn latest_fix(&self) -> Option<GpsFix> {
        self.latest_fix
            .read()
            .expect("latest_fix lock poisoned")
            .clone()
    }

    /// Wait for the background loop to end on its own (inactivity gap or
    /// GPS source exhaustion). Mainly for tests and graceful-shutdown
    /// draining — ordinary operation just runs until [`close`] is called.
    ///
    /// [`close`]: SessionManager::close
    pub async fn join(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.await;
        }
    }

    /// Stop capture: abort the background loop (if still running, a
    /// no-op otherwise) and close the active session.
    pub async fn close(&mut self) -> Result<(), sqlx::Error> {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
        SessionRepo::close_active(&self.pool).await?;
        *self.session_id.write().expect("session_id lock poisoned") = None;
        Ok(())
    }
}

/// The background loop body: pulls fixes from `gps` one at a time,
/// racing each pull against an inactivity-gap timer, writing/hooking on
/// every fix (subject to `write_interval` throttling) and updating
/// `latest_fix`, until the source is exhausted or the gap elapses — then
/// closes the session and returns.
async fn run_ingest_loop<G: GpsSource>(
    pool: PgPool,
    session_id: Uuid,
    mut gps: G,
    gap: Duration,
    write_interval: Duration,
    latest_fix: &RwLock<Option<GpsFix>>,
    hook: HostZoneHook,
) {
    let mut last_write: Option<Instant> = None;

    loop {
        let gap_sleep = tokio::time::sleep(gap);
        tokio::select! {
            fix_opt = gps.next_fix() => {
                match fix_opt {
                    Some(fix) => {
                        *latest_fix.write().expect("latest_fix lock poisoned") = Some(fix.clone());

                        let should_write = write_interval.is_zero()
                            || last_write.is_none_or(|t| t.elapsed() >= write_interval);

                        if should_write {
                            let new_fix = NewLocationFix {
                                session_id,
                                observed_at: fix.at,
                                location: (fix.lon, fix.lat),
                                altitude: fix.altitude,
                                speed: fix.speed,
                                heading: fix.heading,
                                fix_quality: Some(fix.quality.to_string()),
                            };
                            // A single failed write shouldn't kill the whole
                            // session's ingest loop -- no tracing/log crate
                            // is wired into this workspace yet, so this is a
                            // deliberately visible stderr fallback rather
                            // than silently swallowing the error. Critically,
                            // the throttle only advances and the hook only
                            // fires in the `Ok` arm: the documented contract
                            // is "once per row actually WRITTEN", so a failed
                            // insert must neither trigger 5.4's zone
                            // evaluation on data that was never persisted nor
                            // eat the next real write's throttle window.
                            match LocationRepo::insert_fix(&pool, new_fix).await {
                                Ok(_) => {
                                    last_write = Some(Instant::now());
                                    hook(fix).await;
                                }
                                Err(e) => {
                                    eprintln!(
                                        "SessionManager: failed to write location_fix: {e}"
                                    );
                                }
                            }
                        }
                    }
                    None => break,
                }
            }
            _ = gap_sleep => break,
        }
    }

    // Same rationale as the insert-failure path above: don't silently
    // swallow this. An error here means the DB now disagrees with this
    // process's view that the session ended (an in-memory/DB divergence)
    // and that should be visible, not eaten by `let _ = ...`.
    if let Err(e) = SessionRepo::close_active(&pool).await {
        eprintln!("SessionManager: failed to close session on loop exit: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::fresh_pool;
    use async_trait::async_trait;
    use chrono::TimeZone;
    use fluxfang_capture::mock::MockGps;
    use sqlx::Executor;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::mpsc;

    /// A `GpsSource` backed by a channel, letting tests control exactly
    /// when (and whether) fixes arrive -- unlike `MockGps`, which yields
    /// its whole track instantly with no real await/timing behavior.
    /// Needed to exercise the inactivity-gap path in isolation from
    /// source-exhaustion (an open, un-dropped sender means `next_fix`
    /// genuinely awaits with no fix ready, so only the gap timer can win
    /// the loop's `select!`).
    struct ChannelGps(mpsc::UnboundedReceiver<GpsFix>);

    #[async_trait]
    impl GpsSource for ChannelGps {
        async fn next_fix(&mut self) -> Option<GpsFix> {
            self.0.recv().await
        }
    }

    fn counting_hook() -> (HostZoneHook, Arc<AtomicUsize>) {
        let count = Arc::new(AtomicUsize::new(0));
        let count_for_hook = count.clone();
        let hook: HostZoneHook = Arc::new(move |_fix: GpsFix| {
            let count = count_for_hook.clone();
            Box::pin(async move {
                count.fetch_add(1, Ordering::SeqCst);
            })
        });
        (hook, count)
    }

    #[tokio::test]
    async fn open_creates_and_activates_a_session() {
        let pool = fresh_pool().await;
        let gps = MockGps::new(vec![]);

        let manager = SessionManager::open(
            pool.clone(),
            gps,
            SessionManagerConfig::default(),
            no_op_hook(),
        )
        .await
        .unwrap();

        let session_id = manager.current_session_id();
        assert!(session_id.is_some());
        let active = SessionRepo::active(&pool).await.unwrap();
        assert_eq!(active.unwrap().id, session_id.unwrap());
    }

    #[tokio::test]
    async fn n_fixes_are_written_and_latest_fix_tracks_the_last_one() {
        let pool = fresh_pool().await;
        let base = chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let fixes =
            MockGps::synthetic_track(base, -122.0, 37.0, 0.001, chrono::Duration::seconds(1), 3);
        let gps = MockGps::new(fixes.clone());
        let (hook, count) = counting_hook();

        let mut manager =
            SessionManager::open(pool.clone(), gps, SessionManagerConfig::default(), hook)
                .await
                .unwrap();
        let session_id = manager.current_session_id().unwrap();

        // MockGps's track is finite and non-looping: the loop drains it,
        // sees `None`, and closes -- `join` waits for exactly that.
        manager.join().await;

        let rows = LocationRepo::list_for_session(&pool, session_id)
            .await
            .unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(
            count.load(Ordering::SeqCst),
            3,
            "hook fires once per written fix"
        );

        // The manager's session ended when the loop closed itself.
        assert!(SessionRepo::active(&pool).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn closes_after_the_inactivity_gap_with_no_fixes() {
        let pool = fresh_pool().await;
        let (tx, rx) = mpsc::unbounded_channel();
        let gps = ChannelGps(rx);
        // A short, injected gap -- not the production 5-minute default --
        // is exactly the "make the gap injectable" design point: this test
        // exercises the real inactivity-timeout code path deterministically
        // (it always fires once the channel goes quiet) without waiting on
        // wall-clock minutes. See the module docs' "Time injection for
        // tests" section for why this uses real (not `start_paused`) tokio
        // time.
        let gap = Duration::from_millis(30);

        let mut manager = SessionManager::open(
            pool.clone(),
            gps,
            SessionManagerConfig {
                inactivity_gap: gap,
                write_interval: Duration::ZERO,
            },
            no_op_hook(),
        )
        .await
        .unwrap();
        let session_id = manager.current_session_id().unwrap();

        // One fix arrives, proving the loop is alive and logging.
        let base = chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let fix =
            MockGps::synthetic_track(base, -122.0, 37.0, 0.0, chrono::Duration::seconds(1), 1)
                .remove(0);
        tx.send(fix.clone()).unwrap();

        // Then silence: no more sends, and the sender is deliberately kept
        // alive (not dropped) so the channel never closes on its own --
        // only the gap timer can end the loop.
        manager.join().await;

        assert!(SessionRepo::active(&pool).await.unwrap().is_none());
        let rows = LocationRepo::list_for_session(&pool, session_id)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(manager.latest_fix(), Some(fix));

        drop(tx);
    }

    #[tokio::test]
    async fn open_self_heals_a_dangling_active_session() {
        let pool = fresh_pool().await;
        let dangling = SessionRepo::open(&pool).await.unwrap();
        assert!(dangling.ended_at.is_none());

        let manager = SessionManager::open(
            pool.clone(),
            MockGps::new(vec![]),
            SessionManagerConfig::default(),
            no_op_hook(),
        )
        .await
        .unwrap();

        // Exactly one active session -- the new one, not the dangling one.
        let active = SessionRepo::active(&pool).await.unwrap().unwrap();
        assert_eq!(active.id, manager.current_session_id().unwrap());
        assert_ne!(active.id, dangling.id);
    }

    /// Regression test for the "hook fires / throttle advances on FAILED
    /// writes" bug: after one fix writes successfully, this poisons
    /// `location_fix` so every *subsequent* insert fails, then sends a
    /// second fix and asserts the hook did NOT fire for it and no second
    /// row was written.
    ///
    /// Failure injection: `ALTER TABLE ... ADD CONSTRAINT ... CHECK (false)
    /// NOT VALID`. `NOT VALID` skips validating rows that already exist
    /// (so the first, already-written fix's row is untouched) but is still
    /// enforced against every subsequent `INSERT` -- exactly "make the next
    /// write fail" without touching `session_id`/FK identity (which would
    /// risk cascading deletes of the row we're trying to keep) and without
    /// relying on connection/pool-level failures that Postgres's
    /// superuser-by-default local role wouldn't even honor (e.g.
    /// `REVOKE INSERT` is a no-op against a superuser connection).
    #[tokio::test]
    async fn hook_and_throttle_do_not_advance_on_a_failed_write() {
        let pool = fresh_pool().await;
        let (tx, rx) = mpsc::unbounded_channel();
        let gps = ChannelGps(rx);
        let (hook, count) = counting_hook();

        let mut manager =
            SessionManager::open(pool.clone(), gps, SessionManagerConfig::default(), hook)
                .await
                .unwrap();
        let session_id = manager.current_session_id().unwrap();

        let base = chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let mut fixes =
            MockGps::synthetic_track(base, -122.0, 37.0, 0.001, chrono::Duration::seconds(1), 2)
                .into_iter();
        let fix1 = fixes.next().unwrap();
        let fix2 = fixes.next().unwrap();

        // First fix: writes and hooks normally. Poll (bounded, so a
        // regression that never fires the hook fails loudly instead of
        // hanging the suite) until the async write/hook have landed.
        tx.send(fix1).unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while count.load(Ordering::SeqCst) < 1 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("hook should fire for the first, successfully-written fix");
        assert_eq!(
            LocationRepo::list_for_session(&pool, session_id)
                .await
                .unwrap()
                .len(),
            1
        );

        // Poison every subsequent insert into `location_fix`.
        pool.execute(
            "ALTER TABLE location_fix ADD CONSTRAINT block_further_writes CHECK (false) NOT VALID",
        )
        .await
        .unwrap();

        // Second fix: insert_fix must now fail. Drop the sender right
        // after so the loop exhausts the source and closes on its own --
        // `join` then gives a deterministic point after which no more
        // hook/write activity is possible for this test to observe.
        tx.send(fix2).unwrap();
        drop(tx);
        manager.join().await;

        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "hook must not fire for a fix whose write failed"
        );
        let rows = LocationRepo::list_for_session(&pool, session_id)
            .await
            .unwrap();
        assert_eq!(
            rows.len(),
            1,
            "the failed write must not have persisted a second row"
        );
    }

    /// `write_interval > 0` throttles both persisted rows and hook
    /// invocations to at most one per interval: fixes arriving faster than
    /// that must be dropped from the write path (though `latest_fix`
    /// itself is never throttled, per the module docs).
    #[tokio::test]
    async fn write_interval_throttles_writes_and_hook_calls() {
        let pool = fresh_pool().await;
        let (tx, rx) = mpsc::unbounded_channel();
        let gps = ChannelGps(rx);
        let (hook, count) = counting_hook();

        let mut manager = SessionManager::open(
            pool.clone(),
            gps,
            SessionManagerConfig {
                inactivity_gap: Duration::from_secs(5 * 60),
                write_interval: Duration::from_millis(200),
            },
            hook,
        )
        .await
        .unwrap();
        let session_id = manager.current_session_id().unwrap();

        let base = chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let fixes =
            MockGps::synthetic_track(base, -122.0, 37.0, 0.001, chrono::Duration::seconds(1), 5);
        let sent = fixes.len();

        // All five fixes arrive back-to-back, far faster than the
        // 200ms throttle -- only the first should pass `should_write`.
        for fix in fixes {
            tx.send(fix).unwrap();
        }
        // Source exhaustion (not the inactivity gap) ends the loop as soon
        // as the channel drains, giving a deterministic stopping point.
        drop(tx);
        manager.join().await;

        let rows = LocationRepo::list_for_session(&pool, session_id)
            .await
            .unwrap();
        assert!(
            rows.len() < sent,
            "throttle must drop some of {sent} fast-arriving fixes, wrote {}",
            rows.len()
        );
        assert_eq!(
            count.load(Ordering::SeqCst),
            rows.len(),
            "hook count must match the number of rows actually written"
        );
    }

    #[tokio::test]
    async fn close_ends_the_session_and_stops_the_loop() {
        let pool = fresh_pool().await;
        let (_tx, rx) = mpsc::unbounded_channel();
        let gps = ChannelGps(rx);

        let mut manager = SessionManager::open(
            pool.clone(),
            gps,
            SessionManagerConfig::default(),
            no_op_hook(),
        )
        .await
        .unwrap();
        let session_id = manager.current_session_id().unwrap();

        manager.close().await.unwrap();

        assert!(manager.current_session_id().is_none());
        let closed = sqlx::query_as::<_, (Option<chrono::DateTime<chrono::Utc>>,)>(
            "SELECT ended_at FROM survey_session WHERE id = $1",
        )
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(closed.0.is_some());
    }
}
