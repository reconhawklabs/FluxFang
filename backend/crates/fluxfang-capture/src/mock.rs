//! Mock capture sources used for tests and for running the pipeline without
//! real hardware (CI, local dev).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::{Capturer, GpsFix, GpsSource, RawObservation};

/// Replays a fixed, caller-supplied set of [`RawObservation`]s to the
/// channel at a fixed cadence.
///
/// Time handling: this type never generates timestamps itself - the
/// `observed_at` on each emitted observation is exactly whatever the caller
/// put into the `Vec` passed to [`MockCapturer::new`]. The `interval` only
/// controls the (real, monotonic) pacing between sends on the channel; it
/// does not affect the data.
///
/// Behavior at end of the list: by default `MockCapturer` stops after
/// emitting the list once (the spawned task ends and drops its `Sender`, so
/// the receiver observes channel closure). Call [`MockCapturer::looping`]
/// with `true` to replay the list forever instead (useful for long-running
/// manual/dev sessions without real hardware).
///
/// Stop mechanism: `stop` flips a shared `Arc<AtomicBool>` that the spawned
/// task checks before every send, and aborts the stored `JoinHandle` as a
/// backstop so the task cannot outlive `stop()` even if it were blocked.
///
/// Double-start guard: dropping a tokio `JoinHandle` detaches rather than
/// aborts the task, so unconditionally overwriting `self.handle` on a second
/// `start()` call would orphan the first task (it keeps running, sharing the
/// same `running` flag, and for a looping capturer runs forever). `start()`
/// therefore returns an error if a capture is already active (`self.handle`
/// is `Some`). Calling `stop()` clears `self.handle`, so a subsequent
/// `start()` is a legitimate restart, not a rejected double-start.
pub struct MockCapturer {
    observations: Vec<RawObservation>,
    interval: Duration,
    loop_playback: bool,
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl MockCapturer {
    /// Create a capturer that replays `observations` once, pausing
    /// `interval` between each send.
    pub fn new(observations: Vec<RawObservation>, interval: Duration) -> Self {
        Self {
            observations,
            interval,
            loop_playback: false,
            running: Arc::new(AtomicBool::new(false)),
            handle: None,
        }
    }

    /// Set whether the observation list replays forever (`true`) or once
    /// (`false`, the default).
    pub fn looping(mut self, loop_playback: bool) -> Self {
        self.loop_playback = loop_playback;
        self
    }
}

impl Capturer for MockCapturer {
    fn start(&mut self, tx: mpsc::Sender<RawObservation>) -> anyhow::Result<()> {
        if self.handle.is_some() {
            anyhow::bail!("capturer already running");
        }
        self.running.store(true, Ordering::SeqCst);
        let running = self.running.clone();
        let observations = self.observations.clone();
        let interval = self.interval;
        let loop_playback = self.loop_playback;

        let handle = tokio::spawn(async move {
            'replay: loop {
                for obs in &observations {
                    if !running.load(Ordering::SeqCst) {
                        break 'replay;
                    }
                    if tx.send(obs.clone()).await.is_err() {
                        // Receiver dropped; nothing more to do.
                        break 'replay;
                    }
                    tokio::time::sleep(interval).await;
                }
                if !loop_playback {
                    break;
                }
            }
        });

        self.handle = Some(handle);
        Ok(())
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

/// Replays a fixed, caller-supplied synthetic track of [`GpsFix`]es.
///
/// Time handling: identical philosophy to [`MockCapturer`] - fixes carry
/// whatever `at` timestamp the caller gave them. [`MockGps::synthetic_track`]
/// is a convenience data generator that builds a `Vec<GpsFix>` from a base
/// time plus `index * step` (both supplied by the caller), never touching
/// the wall clock.
///
/// Behavior at end of track: by default `next_fix` returns `None` once the
/// track is exhausted. Call [`MockGps::looping`] with `true` to wrap back to
/// the first fix instead of ending.
pub struct MockGps {
    fixes: Vec<GpsFix>,
    index: usize,
    loop_playback: bool,
}

impl MockGps {
    /// Create a GPS source that yields `fixes` in order, then ends.
    pub fn new(fixes: Vec<GpsFix>) -> Self {
        Self {
            fixes,
            index: 0,
            loop_playback: false,
        }
    }

    /// Set whether the track wraps around (`true`) or ends (`false`, the
    /// default) once exhausted.
    pub fn looping(mut self, loop_playback: bool) -> Self {
        self.loop_playback = loop_playback;
        self
    }

    /// Build a synthetic straight-line track: `count` fixes starting at
    /// (`start_lon`, `start_lat`), each subsequent fix advancing both
    /// coordinates by `step` and the timestamp by `interval`, starting from
    /// `base`. Pure data generation - does not read the wall clock.
    pub fn synthetic_track(
        base: chrono::DateTime<chrono::Utc>,
        start_lon: f64,
        start_lat: f64,
        step: f64,
        interval: chrono::Duration,
        count: usize,
    ) -> Vec<GpsFix> {
        (0..count)
            .map(|i| GpsFix {
                at: base + interval * i as i32,
                lon: start_lon + step * i as f64,
                lat: start_lat + step * i as f64,
                altitude: Some(10.0),
                speed: Some(5.0),
                heading: Some(45.0),
                quality: 1,
            })
            .collect()
    }
}

#[async_trait]
impl GpsSource for MockGps {
    async fn next_fix(&mut self) -> Option<GpsFix> {
        if self.index >= self.fixes.len() {
            if self.loop_playback && !self.fixes.is_empty() {
                self.index = 0;
            } else {
                return None;
            }
        }
        let fix = self.fixes[self.index].clone();
        self.index += 1;
        Some(fix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    fn wifi_obs(bssid: &str, observed_at: chrono::DateTime<chrono::Utc>) -> RawObservation {
        RawObservation {
            kind: "wifi".to_string(),
            observed_at,
            signal_strength: Some(-55),
            payload: json!({ "bssid": bssid, "ssid": "test-network" }),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn mock_capturer_emits_fixed_observations_once() {
        let base = chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let observations = vec![
            wifi_obs("AA:AA:AA:AA:AA:01", base),
            wifi_obs("AA:AA:AA:AA:AA:02", base + chrono::Duration::seconds(1)),
            wifi_obs("AA:AA:AA:AA:AA:03", base + chrono::Duration::seconds(2)),
        ];

        let mut capturer = MockCapturer::new(observations.clone(), Duration::from_millis(10));
        let (tx, mut rx) = mpsc::channel(8);
        capturer.start(tx).unwrap();

        let mut received = Vec::new();
        while let Some(obs) = rx.recv().await {
            received.push(obs);
        }

        assert_eq!(received, observations);
        let bssids: Vec<_> = received
            .iter()
            .map(|o| o.payload["bssid"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            bssids,
            vec![
                "AA:AA:AA:AA:AA:01",
                "AA:AA:AA:AA:AA:02",
                "AA:AA:AA:AA:AA:03",
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn mock_capturer_stop_halts_emission() {
        let base = chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        // A long, looping list so that without an explicit stop() it would
        // never close the channel.
        let observations = vec![wifi_obs("AA:AA:AA:AA:AA:01", base)];

        let mut capturer = MockCapturer::new(observations, Duration::from_millis(10)).looping(true);
        let (tx, mut rx) = mpsc::channel(8);
        capturer.start(tx).unwrap();

        // Receive one observation, then stop.
        let first = rx.recv().await;
        assert!(first.is_some());
        capturer.stop();

        // After stop, the channel should close (sender dropped/aborted)
        // rather than yielding forever.
        while rx.recv().await.is_some() {}
    }

    #[tokio::test(start_paused = true)]
    async fn mock_capturer_start_twice_without_stop_errors_and_does_not_duplicate() {
        let base = chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let observations = vec![
            wifi_obs("AA:AA:AA:AA:AA:01", base),
            wifi_obs("AA:AA:AA:AA:AA:02", base + chrono::Duration::seconds(1)),
        ];

        let mut capturer = MockCapturer::new(observations.clone(), Duration::from_millis(10));
        let (tx, mut rx) = mpsc::channel(8);

        capturer.start(tx.clone()).unwrap();

        // A second start() while the first task is still active must be
        // rejected rather than spawning a competing task that shares the
        // same `running` flag.
        let err = capturer.start(tx).unwrap_err();
        assert_eq!(err.to_string(), "capturer already running");

        // Exactly one copy of the observation set should come through - if
        // the second start() had spawned another task, we'd see the set
        // twice (or interleaved).
        let mut received = Vec::new();
        while let Some(obs) = rx.recv().await {
            received.push(obs);
        }
        assert_eq!(received, observations);
    }

    #[tokio::test(start_paused = true)]
    async fn mock_capturer_restart_after_stop_emits_again() {
        let base = chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let observations = vec![wifi_obs("AA:AA:AA:AA:AA:01", base)];

        let mut capturer = MockCapturer::new(observations.clone(), Duration::from_millis(10));

        let (tx1, mut rx1) = mpsc::channel(8);
        capturer.start(tx1).unwrap();
        let mut first_run = Vec::new();
        while let Some(obs) = rx1.recv().await {
            first_run.push(obs);
        }
        assert_eq!(first_run, observations);

        // stop() clears self.handle, so the guard added for double-start
        // must not reject this legitimate restart.
        capturer.stop();

        let (tx2, mut rx2) = mpsc::channel(8);
        capturer.start(tx2).unwrap();
        let mut second_run = Vec::new();
        while let Some(obs) = rx2.recv().await {
            second_run.push(obs);
        }
        assert_eq!(second_run, observations);
    }

    #[tokio::test]
    async fn mock_gps_yields_synthetic_track_then_none() {
        let base = chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let fixes =
            MockGps::synthetic_track(base, -122.0, 37.0, 0.001, chrono::Duration::seconds(5), 3);
        let mut gps = MockGps::new(fixes.clone());

        let mut collected = Vec::new();
        while let Some(fix) = gps.next_fix().await {
            collected.push(fix);
        }

        assert_eq!(collected, fixes);
        assert_eq!(collected.len(), 3);
        assert_eq!(collected[0].at, base);
        assert_eq!(collected[1].at, base + chrono::Duration::seconds(5));
        assert_eq!(collected[2].at, base + chrono::Duration::seconds(10));
        // Positions advance monotonically.
        assert!(collected[1].lon > collected[0].lon);
        assert!(collected[2].lat > collected[1].lat);

        // Track ends.
        assert_eq!(gps.next_fix().await, None);
    }

    #[tokio::test]
    async fn mock_gps_loops_when_configured() {
        let base = chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let fixes = MockGps::synthetic_track(base, 0.0, 0.0, 1.0, chrono::Duration::seconds(1), 2);
        let mut gps = MockGps::new(fixes.clone()).looping(true);

        for _ in 0..2 {
            assert_eq!(gps.next_fix().await, Some(fixes[0].clone()));
            assert_eq!(gps.next_fix().await, Some(fixes[1].clone()));
        }
    }
}
