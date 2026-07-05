//! `fluxfang-capture`: hardware-agnostic capture abstractions.
//!
//! This crate defines the boundary between real capture hardware (WiFi/GPS,
//! see Tasks 4.2/4.3) and the rest of the FluxFang pipeline. It intentionally
//! knows nothing about the database or API layers - it only produces
//! [`RawObservation`]s and [`GpsFix`]es that a higher layer (ingest) consumes.
//!
//! ## Determinism
//!
//! Nothing in this crate calls `Utc::now()` or `Instant::now()` to *generate*
//! data. [`mock::MockCapturer`] replays whatever [`RawObservation`]s it is
//! constructed with verbatim (including their `observed_at` timestamps), and
//! [`mock::MockGps`] replays whatever [`GpsFix`]es it is constructed with. Any
//! timestamps in test data are supplied by the caller (e.g. a fixed base time
//! plus `index * interval`), so tests never depend on wall-clock time.

pub mod gps;
pub mod mock;
pub mod wifi;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// A hardware-agnostic captured datum handed off to ingest.
///
/// `kind` identifies the observation's source/type (e.g. `"wifi"`, `"ble"`);
/// `payload` carries kind-specific fields (e.g. BSSID, SSID) as loosely typed
/// JSON so this crate doesn't need to know about every hardware type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawObservation {
    pub kind: String,
    pub observed_at: DateTime<Utc>,
    pub signal_strength: Option<i32>,
    pub payload: serde_json::Value,
}

/// A background capture source that pushes [`RawObservation`]s to a channel.
///
/// Implementors run their capture loop on a tokio task spawned from
/// [`Capturer::start`] and must stop that task (or otherwise cease sending)
/// promptly after [`Capturer::stop`] is called. The exact stop mechanism is
/// up to the implementor - [`mock::MockCapturer`] uses a shared
/// `Arc<AtomicBool>` flag checked between sends, plus aborting the stored
/// `JoinHandle` as a backstop.
pub trait Capturer: Send {
    /// Begin capturing. Typically spawns a tokio task that sends
    /// [`RawObservation`]s to `tx` until [`Capturer::stop`] is called or the
    /// receiver is dropped.
    fn start(&mut self, tx: mpsc::Sender<RawObservation>) -> anyhow::Result<()>;

    /// Signal the capture task to end. Idempotent; safe to call even if
    /// `start` was never called or already stopped.
    fn stop(&mut self);
}

/// A single GPS fix.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GpsFix {
    pub at: DateTime<Utc>,
    pub lon: f64,
    pub lat: f64,
    pub altitude: Option<f64>,
    pub speed: Option<f64>,
    pub heading: Option<f64>,
    pub quality: i32,
}

/// A source of GPS fixes, yielded one at a time.
///
/// Returns `None` once the source is exhausted (e.g. hardware disconnected,
/// or a mock track has finished and isn't set to loop).
#[async_trait]
pub trait GpsSource: Send {
    async fn next_fix(&mut self) -> Option<GpsFix>;
}
