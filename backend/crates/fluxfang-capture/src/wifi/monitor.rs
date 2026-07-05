//! Thin, hardware-touching wrapper around [`super::parse::parse_frame`].
//!
//! **This module is not exercised by the automated test suite** - it shells
//! out to `iw`/`ip` and opens a live pcap device, neither of which exist in
//! CI or on a dev machine without a monitor-mode-capable WiFi adapter. All
//! the actual parsing logic (the part that's worth testing) lives in
//! [`super::parse::parse_frame`] and is covered by `tests/wifi_parse.rs`
//! against a committed fixture. Verify this module manually on real
//! hardware.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use chrono::Utc;
use tokio::sync::mpsc;

use super::parse::parse_frame;
use crate::{Capturer, RawObservation};

/// Captures WiFi management frames (beacons, probe requests) from a
/// monitor-mode-capable interface via libpcap.
///
/// Lifecycle:
/// - `start` puts `interface` into monitor mode (`iw dev <if> set type
///   monitor` then `ip link set <if> up`, both via `std::process::Command`),
///   opens it with the `pcap` crate, and spawns a plain `std::thread` (not a
///   tokio task - `pcap`'s blocking read is not async-aware) that loops:
///   read a packet, run [`parse_frame`], and on a match, stamp
///   `observed_at = Utc::now()` and `tx.blocking_send(...)` the resulting
///   [`RawObservation`].
/// - The capture is opened with a short read timeout (see
///   [`READ_TIMEOUT_MS`]) purely so the blocking read periodically returns
///   control to the loop to recheck the stop flag; a `TimeoutExpired` result
///   is not an error, just a wakeup.
/// - `stop` flips a shared `Arc<AtomicBool>`, joins the capture thread (so
///   the caller knows the device has actually been released before
///   `stop()` returns), and restores managed mode (`iw dev <if> set type
///   managed`).
///
/// Same double-start guard as [`crate::mock::MockCapturer`]: `start` errors
/// if a capture is already active (`self.handle.is_some()`), and `stop`
/// clears the handle so a later restart is legitimate.
pub struct WifiMonitorCapturer {
    interface: String,
    running: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
    /// The channel-hopping thread's handle (see [`spawn_channel_hopper`]).
    hop_handle: Option<thread::JoinHandle<()>>,
}

/// How long a single blocking `next_packet()` read may block before
/// returning `Error::TimeoutExpired`, so the capture thread wakes up often
/// enough to notice `stop()` even with no traffic on the channel.
const READ_TIMEOUT_MS: i32 = 200;

/// How long to dwell on each channel before hopping to the next. ~250ms is
/// the airodump-ng-style default: long enough to catch a beacon (APs beacon
/// ~10x/sec) but short enough to sweep every channel within a few seconds.
const CHANNEL_DWELL_MS: u64 = 250;

/// The channels the monitor interface hops across so it hears APs on every
/// band, not just whatever it happened to be parked on. 2.4GHz 1-13 (the
/// non-overlapping 1/6/11 first so they get an early hit), then a broad set
/// of common 5GHz channels. Channels the adapter/regulatory domain doesn't
/// support just fail the `iw set channel` and are skipped — see
/// [`spawn_channel_hopper`].
pub(crate) fn channel_hop_list() -> Vec<u16> {
    let mut chans = vec![1u16, 6, 11, 2, 3, 4, 5, 7, 8, 9, 10, 12, 13];
    // Common 5GHz channels: UNII-1, UNII-2 (incl. DFS), UNII-3.
    chans.extend([
        36, 40, 44, 48, 52, 56, 60, 64, 100, 104, 108, 112, 116, 120, 124, 128, 132, 136, 140, 149,
        153, 157, 161, 165,
    ]);
    chans
}

/// Spawns a thread that retunes `interface` across [`channel_hop_list`] every
/// [`CHANNEL_DWELL_MS`] until `running` is cleared. Per-channel `iw set
/// channel` failures (unsupported channel / DFS / regulatory) are ignored so
/// one bad channel doesn't stop the sweep.
fn spawn_channel_hopper(interface: String, running: Arc<AtomicBool>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let channels = channel_hop_list();
        'outer: while running.load(Ordering::SeqCst) {
            for ch in &channels {
                if !running.load(Ordering::SeqCst) {
                    break 'outer;
                }
                let _ = run_cmd(
                    "iw",
                    &["dev", &interface, "set", "channel", &ch.to_string()],
                );
                thread::sleep(Duration::from_millis(CHANNEL_DWELL_MS));
            }
        }
    })
}

/// Run an external command (`iw`/`ip`), surfacing its stderr in the error so
/// the UI shows *why* it failed (e.g. "Device or resource busy") instead of a
/// bare exit code like "exit status: 240".
fn run_cmd(bin: &str, args: &[&str]) -> anyhow::Result<()> {
    let output = std::process::Command::new(bin)
        .args(args)
        .output()
        .map_err(|e| {
            anyhow::anyhow!("failed to run `{bin}` (is it installed on the host image?): {e}")
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        if stderr.is_empty() {
            anyhow::bail!("`{bin} {}` exited with {}", args.join(" "), output.status);
        }
        anyhow::bail!("`{bin} {}` failed: {stderr}", args.join(" "));
    }
    Ok(())
}

impl WifiMonitorCapturer {
    /// Create a capturer bound to `interface` (e.g. `"wlan0"`). Monitor mode
    /// is not set until [`Capturer::start`] is called.
    pub fn new(interface: impl Into<String>) -> Self {
        Self {
            interface: interface.into(),
            running: Arc::new(AtomicBool::new(false)),
            handle: None,
            hop_handle: None,
        }
    }

    fn run_iw(&self, args: &[&str]) -> anyhow::Result<()> {
        run_cmd("iw", args)
    }

    fn run_ip(&self, args: &[&str]) -> anyhow::Result<()> {
        run_cmd("ip", args)
    }

    fn set_monitor_mode(&self) -> anyhow::Result<()> {
        // The interface must be DOWN before its type can change: most drivers
        // reject `iw ... set type monitor` with EBUSY ("Device or resource
        // busy", `iw` exit status 240) while the interface is up. So bring it
        // down, switch type, then bring it back up.
        self.run_ip(&["link", "set", &self.interface, "down"])?;
        self.run_iw(&["dev", &self.interface, "set", "type", "monitor"])?;
        self.run_ip(&["link", "set", &self.interface, "up"])?;
        Ok(())
    }

    fn restore_managed_mode(&self) -> anyhow::Result<()> {
        // Same down-before-type-change requirement as monitor mode.
        self.run_ip(&["link", "set", &self.interface, "down"])?;
        self.run_iw(&["dev", &self.interface, "set", "type", "managed"])?;
        self.run_ip(&["link", "set", &self.interface, "up"])?;
        Ok(())
    }

    /// Opens the (already monitor-mode) interface with `pcap`. Split out of
    /// `start` purely so the error path there can roll back monitor mode
    /// before propagating whatever this returns.
    fn open_capture(&self) -> anyhow::Result<pcap::Capture<pcap::Active>> {
        let cap = pcap::Capture::from_device(self.interface.as_str())?
            .promisc(true)
            .timeout(READ_TIMEOUT_MS)
            .open()?;
        Ok(cap)
    }
}

impl Capturer for WifiMonitorCapturer {
    fn start(&mut self, tx: mpsc::Sender<RawObservation>) -> anyhow::Result<()> {
        if self.handle.is_some() {
            anyhow::bail!("capturer already running");
        }

        self.set_monitor_mode()?;

        // If opening the device fails after we've already flipped the
        // interface into monitor mode, roll that back (best-effort - we
        // still want to surface the real `open` error, not a rollback
        // failure) rather than leaving the adapter stuck in monitor mode
        // and unable to rejoin a normal network.
        let mut cap = match self.open_capture() {
            Ok(cap) => cap,
            Err(err) => {
                let _ = self.restore_managed_mode();
                return Err(err);
            }
        };

        self.running.store(true, Ordering::SeqCst);
        let running = self.running.clone();

        // Sweep channels so we hear APs on every band, not just the one the
        // radio powered up on (otherwise only whatever channel we're parked
        // on is captured — typically ch1 — and off-channel APs are invisible).
        self.hop_handle = Some(spawn_channel_hopper(
            self.interface.clone(),
            self.running.clone(),
        ));

        let handle = thread::spawn(move || {
            while running.load(Ordering::SeqCst) {
                match cap.next_packet() {
                    Ok(packet) => {
                        if let Some(obs) = parse_frame(packet.data) {
                            let raw = obs.into_raw_observation(Utc::now());
                            if tx.blocking_send(raw).is_err() {
                                // Receiver dropped; nothing more to do.
                                break;
                            }
                        }
                    }
                    // Just a wakeup to recheck `running`; not an error.
                    Err(pcap::Error::TimeoutExpired) => continue,
                    // Device gone, permission revoked, etc. - stop rather
                    // than spin.
                    Err(_) => break,
                }
            }
        });

        self.handle = Some(handle);
        Ok(())
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            // Block until the capture thread actually exits so the device
            // is released before we try to flip it back to managed mode.
            let _ = handle.join();
        }
        if let Some(hop) = self.hop_handle.take() {
            // Stop retuning the radio before we restore managed mode.
            let _ = hop.join();
        }
        if let Err(err) = self.restore_managed_mode() {
            eprintln!(
                "wifi monitor: failed to restore managed mode on {}: {err:#}",
                self.interface
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::channel_hop_list;

    #[test]
    fn channel_hop_list_covers_both_bands_without_duplicates() {
        let chans = channel_hop_list();
        // 2.4GHz non-overlapping channels come first for an early hit.
        assert_eq!(&chans[..3], &[1, 6, 11]);
        // Covers 2.4GHz (<=14) and 5GHz (>=36).
        assert!(chans.iter().any(|&c| (1..=13).contains(&c)));
        assert!(chans.iter().any(|&c| c >= 36));
        // No duplicates.
        let mut sorted = chans.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), chans.len(), "channel list has duplicates");
    }
}
