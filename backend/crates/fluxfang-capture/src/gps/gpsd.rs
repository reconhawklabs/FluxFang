//! `gpsd` client: pure JSON `TPV`-report parsing ([`parse_tpv`]) plus a thin
//! live-network wrapper ([`GpsdSource`]).
//!
//! **[`GpsdSource`] is not exercised by the automated test suite** - it
//! opens a real TCP connection to a running `gpsd` daemon, which doesn't
//! exist in CI or on a dev machine without one. All the actual parsing
//! logic (the part that's worth testing) lives in [`parse_tpv`], which is a
//! pure function over an already-decoded [`serde_json::Value`] and is
//! covered by the unit tests at the bottom of this file. Verify
//! [`GpsdSource`] manually against a real (or `gpsd`'s bundled test/replay)
//! daemon.

use anyhow::Context;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::OwnedReadHalf;
use tokio::net::TcpStream;

use crate::{GpsFix, GpsSource};

/// gpsd's `?WATCH=...;` command, sent once on connect to ask the daemon to
/// start streaming JSON reports (rather than requiring a client to poll).
const WATCH_ENABLE_JSON: &str = "?WATCH={\"enable\":true,\"json\":true};\n";

/// A live connection to a `gpsd` daemon, yielding [`GpsFix`]es parsed from
/// its `TPV` ("Time-Position-Velocity") reports.
///
/// gpsd speaks newline-delimited JSON over a plain TCP socket. On connect we
/// send [`WATCH_ENABLE_JSON`] to start the stream, then just read lines:
/// each is decoded as JSON and handed to [`parse_tpv`], which returns `None`
/// for anything that isn't a `TPV` report (`VERSION`, `DEVICES`, `SKY`,
/// a `WATCH` ack, ...) or that lacks a fix - [`GpsdSource::next_fix`] simply
/// keeps reading until it finds one, or the connection ends.
///
/// The write half is kept alive (`_write_half`) purely so the socket isn't
/// half-closed by dropping it; nothing is written after the initial `WATCH`
/// command (YAGNI - no other gpsd control commands are needed for this
/// task).
pub struct GpsdSource {
    reader: BufReader<OwnedReadHalf>,
    _write_half: tokio::net::tcp::OwnedWriteHalf,
}

impl GpsdSource {
    /// Connect to a `gpsd` daemon at `host:port` (typically `127.0.0.1:2947`)
    /// and enable JSON TPV streaming.
    pub async fn connect(host: &str, port: u16) -> anyhow::Result<Self> {
        let stream = TcpStream::connect((host, port))
            .await
            .with_context(|| format!("connecting to gpsd at {host}:{port}"))?;
        let (read_half, mut write_half) = stream.into_split();
        write_half
            .write_all(WATCH_ENABLE_JSON.as_bytes())
            .await
            .context("sending ?WATCH to gpsd")?;
        Ok(Self {
            reader: BufReader::new(read_half),
            _write_half: write_half,
        })
    }
}

#[async_trait]
impl GpsSource for GpsdSource {
    async fn next_fix(&mut self) -> Option<GpsFix> {
        loop {
            let mut line = String::new();
            match self.reader.read_line(&mut line).await {
                // EOF: gpsd closed the connection.
                Ok(0) => return None,
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let value: serde_json::Value = match serde_json::from_str(trimmed) {
                        Ok(v) => v,
                        // Not valid JSON; skip and keep reading.
                        Err(_) => continue,
                    };
                    if let Some(fix) = parse_tpv(&value) {
                        return Some(fix);
                    }
                    // A non-TPV report (VERSION, DEVICES, SKY, WATCH ack,
                    // ...) or a TPV with no usable fix - keep reading.
                }
                // Socket error; nothing more to do.
                Err(_) => return None,
            }
        }
    }
}

/// Parse a single decoded gpsd JSON report into a [`GpsFix`], iff it's a
/// `TPV` ("Time-Position-Velocity") report carrying a usable position.
///
/// Pure - no I/O, no clock reads. Returns `None` when:
/// - `class` isn't `"TPV"` (e.g. `VERSION`, `DEVICES`, `SKY`, a `WATCH`
///   acknowledgement),
/// - `lat`/`lon` are missing (gpsd omits them on a `TPV` with no fix yet),
/// - `time` is missing - `GpsFix::at` is required and this function never
///   substitutes `Utc::now()` for a missing value (that would make it
///   impure); a `TPV` with no fix and no time is simply not a usable fix.
///
/// Field mapping:
/// - `lat`/`lon` -> `GpsFix::lat`/`lon` directly (already decimal degrees).
/// - `altHAE` (ellipsoidal altitude) preferred, falling back to `alt`
///   (MSL altitude) if `altHAE` is absent - both are meters.
/// - `speed` (m/s, already gpsd's unit - no conversion, unlike NMEA's
///   knots) -> `GpsFix::speed`.
/// - `track` (degrees true) -> `GpsFix::heading`.
/// - `mode` (gpsd fix mode: 0/1 = no fix, 2 = 2D, 3 = 3D) -> `GpsFix::quality`
///   directly, defaulting to `0` if absent.
pub fn parse_tpv(value: &serde_json::Value) -> Option<GpsFix> {
    if value.get("class").and_then(|c| c.as_str()) != Some("TPV") {
        return None;
    }
    let lat = value.get("lat")?.as_f64()?;
    let lon = value.get("lon")?.as_f64()?;
    let time_str = value.get("time")?.as_str()?;
    let at: DateTime<Utc> = DateTime::parse_from_rfc3339(time_str)
        .ok()?
        .with_timezone(&Utc);

    let altitude = value
        .get("altHAE")
        .and_then(|v| v.as_f64())
        .or_else(|| value.get("alt").and_then(|v| v.as_f64()));
    let speed = value.get("speed").and_then(|v| v.as_f64());
    let heading = value.get("track").and_then(|v| v.as_f64());
    let quality = value.get("mode").and_then(|v| v.as_i64()).unwrap_or(0) as i32;

    Some(GpsFix {
        at,
        lon,
        lat,
        altitude,
        speed,
        heading,
        quality,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_tpv_report_into_gps_fix() {
        let value = json!({
            "class": "TPV",
            "device": "/dev/ttyUSB0",
            "mode": 3,
            "time": "2026-07-05T12:35:19.000Z",
            "lat": 48.1173,
            "lon": 11.5167,
            "altHAE": 545.4,
            "speed": 5.14,
            "track": 84.4,
        });

        let fix = parse_tpv(&value).unwrap();
        assert_eq!(fix.lat, 48.1173);
        assert_eq!(fix.lon, 11.5167);
        assert_eq!(fix.altitude, Some(545.4));
        assert_eq!(fix.speed, Some(5.14));
        assert_eq!(fix.heading, Some(84.4));
        assert_eq!(fix.quality, 3);
        assert_eq!(
            fix.at,
            DateTime::parse_from_rfc3339("2026-07-05T12:35:19.000Z")
                .unwrap()
                .with_timezone(&Utc)
        );
    }

    #[test]
    fn falls_back_to_alt_when_alt_hae_absent() {
        let value = json!({
            "class": "TPV",
            "mode": 2,
            "time": "2026-07-05T12:35:19.000Z",
            "lat": 1.0,
            "lon": 2.0,
            "alt": 12.3,
        });
        let fix = parse_tpv(&value).unwrap();
        assert_eq!(fix.altitude, Some(12.3));
    }

    #[test]
    fn non_tpv_report_returns_none() {
        let value = json!({"class": "VERSION", "release": "3.25"});
        assert_eq!(parse_tpv(&value), None);
    }

    #[test]
    fn tpv_without_fix_returns_none() {
        // gpsd reports a TPV with mode 1 ("no fix") and omits lat/lon
        // entirely rather than sending stale/zero coordinates.
        let value = json!({"class": "TPV", "mode": 1, "time": "2026-07-05T12:35:19.000Z"});
        assert_eq!(parse_tpv(&value), None);
    }

    #[test]
    fn malformed_json_value_returns_none_without_panic() {
        assert_eq!(parse_tpv(&json!({})), None);
        assert_eq!(parse_tpv(&json!("not an object")), None);
        assert_eq!(parse_tpv(&json!(null)), None);
    }
}
