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
}

/// How long a single blocking `next_packet()` read may block before
/// returning `Error::TimeoutExpired`, so the capture thread wakes up often
/// enough to notice `stop()` even with no traffic on the channel.
const READ_TIMEOUT_MS: i32 = 200;

impl WifiMonitorCapturer {
    /// Create a capturer bound to `interface` (e.g. `"wlan0"`). Monitor mode
    /// is not set until [`Capturer::start`] is called.
    pub fn new(interface: impl Into<String>) -> Self {
        Self {
            interface: interface.into(),
            running: Arc::new(AtomicBool::new(false)),
            handle: None,
        }
    }

    fn run_iw(&self, args: &[&str]) -> anyhow::Result<()> {
        let status = std::process::Command::new("iw").args(args).status()?;
        if !status.success() {
            anyhow::bail!("`iw {}` exited with {status}", args.join(" "));
        }
        Ok(())
    }

    fn run_ip(&self, args: &[&str]) -> anyhow::Result<()> {
        let status = std::process::Command::new("ip").args(args).status()?;
        if !status.success() {
            anyhow::bail!("`ip {}` exited with {status}", args.join(" "));
        }
        Ok(())
    }

    fn set_monitor_mode(&self) -> anyhow::Result<()> {
        self.run_iw(&["dev", &self.interface, "set", "type", "monitor"])?;
        self.run_ip(&["link", "set", &self.interface, "up"])?;
        Ok(())
    }

    fn restore_managed_mode(&self) -> anyhow::Result<()> {
        self.run_iw(&["dev", &self.interface, "set", "type", "managed"])
    }
}

impl Capturer for WifiMonitorCapturer {
    fn start(&mut self, tx: mpsc::Sender<RawObservation>) -> anyhow::Result<()> {
        if self.handle.is_some() {
            anyhow::bail!("capturer already running");
        }

        self.set_monitor_mode()?;

        let mut cap = pcap::Capture::from_device(self.interface.as_str())?
            .promisc(true)
            .timeout(READ_TIMEOUT_MS)
            .open()?;

        self.running.store(true, Ordering::SeqCst);
        let running = self.running.clone();

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
        if let Err(err) = self.restore_managed_mode() {
            eprintln!(
                "wifi monitor: failed to restore managed mode on {}: {err:#}",
                self.interface
            );
        }
    }
}
