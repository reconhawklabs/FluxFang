//! Managed-mode WiFi "SSID scan" capture: an alternative to
//! [`super::monitor::WifiMonitorCapturer`] that never touches the
//! interface's mode (no `iw ... set type monitor`/`ip link set up`, no
//! libpcap). Instead it periodically runs `iw dev <if> scan` — an *active*
//! scan the adapter's own firmware performs while still in ordinary
//! managed mode — and parses the resulting nearby-AP list.
//!
//! Trade-off vs. monitor mode: `scan` mode only sees APs that answer an
//! active probe (their SSID/BSSID/signal/channel as of the last scan), not
//! every beacon/probe-request frame in the air, and it can't see hidden
//! networks' true SSID or client probe requests at all. In exchange it
//! works on any managed-mode adapter (no monitor-mode-capable chipset/driver
//! required) and never takes the interface off the network.
//!
//! As with [`super::monitor`], the interesting, testable logic
//! ([`parse_iw_scan`]) is pure and hardware-free; [`WifiScanCapturer`] is a
//! thin wrapper that shells out to `iw` and feeds that function's output
//! into the same [`crate::Capturer`] pipeline. `WifiScanCapturer` itself is
//! **not exercised by the automated test suite** (needs a real wifi
//! interface, and `iw dev scan` typically needs elevated privileges) — see
//! `tests/wifi_scan_parse.rs` for `parse_iw_scan`'s coverage.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use chrono::Utc;
use tokio::sync::mpsc;

use super::parse::{freq_to_channel, WifiObservation};
use crate::{Capturer, RawObservation};

/// Default time between successive `iw dev <if> scan` runs. Active
/// scanning briefly disrupts the adapter's normal traffic and can annoy
/// nearby APs/clients if run too often, so this deliberately isn't
/// aggressive — 15s is frequent enough for "what's nearby" surveying
/// without hammering the radio.
pub const DEFAULT_SCAN_INTERVAL: Duration = Duration::from_secs(15);

/// How long a single sleep-between-scans step waits before rechecking the
/// `running` flag — keeps `stop()` responsive even mid-interval, same
/// rationale as [`super::monitor::READ_TIMEOUT_MS`].
const STOP_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Captures nearby access points by periodically running `iw dev <if>
/// scan` against a plain managed-mode interface — no monitor mode setup or
/// teardown, unlike [`super::monitor::WifiMonitorCapturer`].
///
/// Lifecycle:
/// - `start` spawns a plain `std::thread` (not a tokio task — the `iw`
///   subprocess call is blocking) that loops: run `iw dev <if> scan`,
///   [`parse_iw_scan`] its stdout, stamp each resulting observation's
///   `observed_at = Utc::now()` and `tx.blocking_send(...)` it, then sleep
///   for `interval` (checked in short slices against the stop flag so
///   `stop()` doesn't have to wait out a full interval).
/// - If the `iw` command itself fails (interface down, adapter busy with
///   another scan, insufficient permission, missing `iw` binary, ...) the
///   error is logged to stderr and the loop simply continues to the next
///   interval rather than crashing or exiting the loop — a transient
///   failure shouldn't take the whole data source down.
/// - `stop` flips a shared `Arc<AtomicBool>` and joins the thread. Same
///   double-start guard as [`super::monitor::WifiMonitorCapturer`]: `start`
///   errors if already running (`self.handle.is_some()`), and `stop`
///   clears the handle so a later restart is legitimate.
pub struct WifiScanCapturer {
    interface: String,
    interval: Duration,
    running: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl WifiScanCapturer {
    /// Create a capturer bound to `interface` (e.g. `"wlan0"`), scanning
    /// every [`DEFAULT_SCAN_INTERVAL`]. The interface is never put into
    /// monitor mode; it's used exactly as-is.
    pub fn new(interface: impl Into<String>) -> Self {
        Self {
            interface: interface.into(),
            interval: DEFAULT_SCAN_INTERVAL,
            running: Arc::new(AtomicBool::new(false)),
            handle: None,
        }
    }

    /// Chainable: override the default scan interval (e.g. a shorter one
    /// for a manual/interactive survey, or a longer one to be gentler on
    /// the radio).
    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }
}

impl Capturer for WifiScanCapturer {
    fn start(&mut self, tx: mpsc::Sender<RawObservation>) -> anyhow::Result<()> {
        if self.handle.is_some() {
            anyhow::bail!("capturer already running");
        }

        self.running.store(true, Ordering::SeqCst);
        let running = self.running.clone();
        let interface = self.interface.clone();
        let interval = self.interval;

        let handle = thread::spawn(move || {
            while running.load(Ordering::SeqCst) {
                match run_iw_scan(&interface) {
                    Ok(stdout) => {
                        for obs in parse_iw_scan(&stdout) {
                            let raw = obs.into_raw_observation(Utc::now());
                            if tx.blocking_send(raw).is_err() {
                                // Receiver dropped; nothing more to do.
                                return;
                            }
                        }
                    }
                    Err(err) => {
                        // Interface down/busy/permission/missing `iw` --
                        // don't crash the loop over a transient failure,
                        // just try again next interval.
                        eprintln!("wifi scan: `iw dev {interface} scan` failed: {err:#}");
                    }
                }
                sleep_while_running(&running, interval);
            }
        });

        self.handle = Some(handle);
        Ok(())
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Sleeps up to `total`, but in short [`STOP_POLL_INTERVAL`] slices,
/// bailing out early the moment `running` goes false — so `stop()` doesn't
/// have to wait out a whole (possibly long) scan interval before the
/// capture thread notices and exits.
fn sleep_while_running(running: &AtomicBool, total: Duration) {
    let mut remaining = total;
    while !remaining.is_zero() && running.load(Ordering::SeqCst) {
        let step = remaining.min(STOP_POLL_INTERVAL);
        thread::sleep(step);
        remaining -= step;
    }
}

/// Runs `iw dev <interface> scan` and returns its stdout as a `String`.
/// Errors (missing `iw` binary, non-zero exit — e.g. interface down, scan
/// already in progress, insufficient permission) are returned as `Err`
/// rather than panicking; the caller logs and moves on.
fn run_iw_scan(interface: &str) -> anyhow::Result<String> {
    let output = std::process::Command::new("iw")
        .args(["dev", interface, "scan"])
        .output()?;
    if !output.status.success() {
        anyhow::bail!(
            "`iw dev {interface} scan` exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// One `BSS ...` block's fields accumulated while walking `iw scan` output,
/// before being converted into a [`WifiObservation`].
struct PartialBss {
    bssid: String,
    ssid: Option<String>,
    freq: Option<u16>,
    signal: Option<i32>,
}

impl PartialBss {
    fn into_observation(self) -> WifiObservation {
        WifiObservation {
            bssid: Some(self.bssid),
            // `iw scan` only ever surfaces APs (see the `frame_type` comment
            // below), so there's no client `src_mac` to report here.
            src_mac: None,
            ssid: self.ssid,
            // Scan results are built from what the adapter's firmware
            // heard from each AP (beacons and/or probe responses) while
            // scanning — there's no meaningful distinction to draw here
            // between the two the way `parse_frame` does for raw captured
            // frames, so this is always reported as `"beacon"` (an
            // existing, already-cataloged `frame_type` value — see
            // `fluxfang_core::catalog::wifi_catalog`) rather than
            // inventing a new enum value.
            frame_type: "beacon".to_string(),
            channel: self.freq.and_then(freq_to_channel),
            signal_strength: self.signal,
        }
    }
}

/// Parses `iw dev <if> scan` output into one [`WifiObservation`] per `BSS`
/// block.
///
/// Sample input this handles:
/// ```text
/// BSS aa:bb:cc:dd:ee:ff(on wlan0)
///     freq: 2437
///     signal: -42.00 dBm
///     SSID: FluxTest
///     DS Parameter set: channel 6
/// BSS 11:22:33:44:55:66(on wlan0) -- associated
///     freq: 5180
///     signal: -55.00 dBm
///     SSID: HomeNet5G
/// ```
///
/// Per block:
/// - `bssid` comes from the `BSS <mac>(on <if>)` header line (lowercased;
///   the optional `(on <if>)`/` -- associated` suffix is discarded). A
///   header line whose leading token isn't a well-formed
///   `aa:bb:cc:dd:ee:ff` MAC is treated as garbage: the block is skipped
///   entirely (its field lines, if any, are ignored) rather than
///   propagating a half-parsed observation.
/// - `signal: <float> dBm` -> `signal_strength`, rounded to the nearest
///   `i32` (e.g. `-42.00` -> `-42`). Missing or unparseable -> `None`.
/// - `freq: <MHz>` -> `channel` via [`freq_to_channel`] (the same
///   2.4GHz/5GHz mapping [`super::parse::parse_frame`] uses for radiotap's
///   Channel field). Missing or unparseable frequency, or one outside the
///   recognized 2.4/5GHz ranges -> `None`.
/// - `SSID: <name>` -> `ssid`. A present-but-blank SSID line (hidden
///   network) or an altogether absent SSID line both yield `None` — this
///   function doesn't distinguish "no tag" from "empty tag" the way
///   `parse_frame` does, since `iw`'s text output doesn't preserve that
///   distinction either.
///
/// Never panics: malformed, truncated, or entirely non-`iw`-scan input
/// (including empty input) simply yields fewer/no observations, never an
/// index-out-of-bounds or other panic.
pub fn parse_iw_scan(output: &str) -> Vec<WifiObservation> {
    let mut observations = Vec::new();
    let mut current: Option<PartialBss> = None;

    for line in output.lines() {
        let trimmed = line.trim();

        if let Some(rest) = trimmed.strip_prefix("BSS ") {
            if let Some(bss) = current.take() {
                observations.push(bss.into_observation());
            }
            let bssid = rest.split('(').next().unwrap_or("").trim().to_lowercase();
            current = if is_valid_mac(&bssid) {
                Some(PartialBss {
                    bssid,
                    ssid: None,
                    freq: None,
                    signal: None,
                })
            } else {
                // Malformed BSS header - skip this block's fields too
                // (they belong to a BSS we couldn't identify).
                None
            };
            continue;
        }

        let Some(bss) = current.as_mut() else {
            continue;
        };

        // `strip_prefix` on the bare `"freq:"`/`"signal:"`/`"SSID:"` tag
        // (no trailing space required) then `.trim()` the rest: real `iw`
        // output always has a space after the colon, but a hidden
        // network's blank `SSID:` line sometimes has nothing after the
        // colon at all (not even a space) - requiring the space would
        // silently fail to match that line's prefix.
        if let Some(rest) = trimmed.strip_prefix("freq:") {
            bss.freq = rest.trim().parse::<u16>().ok();
        } else if let Some(rest) = trimmed.strip_prefix("signal:") {
            bss.signal = rest
                .split_whitespace()
                .next()
                .and_then(|token| token.parse::<f64>().ok())
                .map(|dbm| dbm.round() as i32);
        } else if let Some(rest) = trimmed.strip_prefix("SSID:") {
            let ssid = rest.trim();
            bss.ssid = if ssid.is_empty() {
                None
            } else {
                Some(ssid.to_string())
            };
        }
    }

    if let Some(bss) = current.take() {
        observations.push(bss.into_observation());
    }

    observations
}

/// Checks `candidate` looks like `aa:bb:cc:dd:ee:ff`: six colon-separated
/// two-hex-digit groups. Doesn't allocate beyond the `split` iterator;
/// never panics on short/garbage input.
fn is_valid_mac(candidate: &str) -> bool {
    let parts: Vec<&str> = candidate.split(':').collect();
    parts.len() == 6
        && parts
            .iter()
            .all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_yields_no_observations() {
        assert_eq!(parse_iw_scan(""), Vec::new());
    }

    #[test]
    fn garbage_without_any_bss_block_yields_no_observations() {
        assert_eq!(
            parse_iw_scan("not an iw scan at all\nfreq: 2437\n"),
            Vec::new()
        );
    }
}
