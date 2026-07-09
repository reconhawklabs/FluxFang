//! Serial NMEA GPS reader: thin, hardware-touching wrapper around
//! [`super::nmea::parse_nmea`].
//!
//! **This module is not exercised by the automated test suite** (beyond the
//! pure baud-rate validation) - it opens a real serial device via the
//! `serialport` crate, which doesn't exist in CI or on a dev machine without
//! GPS hardware attached. All the actual parsing logic (the part that's
//! worth testing) lives in [`super::nmea::parse_nmea`] and is covered by
//! `tests/nmea.rs`. Verify [`SerialGpsSource`] manually on real hardware.

use std::io::BufRead;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::mpsc;

use super::nmea::parse_nmea;
use crate::{GpsFix, LocationSource};

/// Baud rates this source accepts. Anything else is rejected by
/// [`SerialGpsSource::open`] before touching the device, rather than
/// silently passing an unsupported rate down to the OS driver.
///
/// `pub` (re-exported at `gps::ALLOWED_BAUD_RATES`) so Task 6.2's
/// `fluxfang-api` data-source config validation can check a proposed serial
/// baud rate against this exact list instead of hand-duplicating it and
/// risking the two lists drifting apart.
pub const ALLOWED_BAUD_RATES: [u32; 6] = [4800, 9600, 19200, 38400, 57600, 115200];

/// How long a single blocking `read_line` may block before returning a
/// timeout error, so the reader thread wakes up often enough to notice
/// [`SerialGpsSource`] being dropped even with no data arriving on the wire.
const READ_TIMEOUT: Duration = Duration::from_millis(500);

/// Reads NMEA sentences from a serial GPS device and yields parsed
/// [`GpsFix`]es.
///
/// Lifecycle: [`SerialGpsSource::open`] validates `baud`, opens the device
/// via the `serialport` crate, and spawns a plain `std::thread` (not a
/// tokio task - `serialport`'s blocking read is not async-aware) that loops:
/// blocking-read a line, run [`parse_nmea`] against it (stamped with
/// `Utc::now().date_naive()` - see below), and on a successful parse, push
/// the resulting [`GpsFix`] onto an internal channel. [`LocationSource::next_fix`]
/// just awaits that channel.
///
/// ## Where the clock is read
///
/// [`parse_nmea`] itself never touches the clock (see its module docs) - it
/// takes a `date` parameter. This reader thread is the one place that
/// samples `Utc::now().date_naive()` (once per line) and passes it in as
/// that `date`, so a GGA sentence (which carries no date of its own) still
/// gets today's date. That's the intentional, documented boundary: the pure
/// parser stays deterministic and unit-testable; the impurity lives here, in
/// the thin I/O wrapper that isn't unit-tested anyway.
///
/// Dropping a `SerialGpsSource` stops the reader thread (flips the shared
/// stop flag and joins it) so the device is released promptly.
pub struct SerialGpsSource {
    rx: mpsc::Receiver<GpsFix>,
    running: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl SerialGpsSource {
    /// Open `device` (e.g. `"/dev/ttyUSB0"`) at `baud`. `baud` must be one of
    /// [`ALLOWED_BAUD_RATES`]; anything else is rejected without attempting
    /// to open the device.
    pub fn open(device: &str, baud: u32) -> anyhow::Result<Self> {
        if !ALLOWED_BAUD_RATES.contains(&baud) {
            anyhow::bail!("unsupported baud rate {baud}; must be one of {ALLOWED_BAUD_RATES:?}");
        }

        let port = serialport::new(device, baud)
            .timeout(READ_TIMEOUT)
            .open()
            .map_err(|err| anyhow::anyhow!("opening serial GPS device {device}: {err}"))?;

        let (tx, rx) = mpsc::channel(16);
        let running = Arc::new(AtomicBool::new(true));
        let running_thread = running.clone();

        let handle = thread::spawn(move || {
            let mut reader = std::io::BufReader::new(port);
            let mut line = String::new();
            while running_thread.load(Ordering::SeqCst) {
                line.clear();
                match reader.read_line(&mut line) {
                    // EOF: device gone.
                    Ok(0) => break,
                    Ok(_) => {
                        let today = Utc::now().date_naive();
                        if let Some(fix) = parse_nmea(line.trim(), today) {
                            if tx.blocking_send(fix).is_err() {
                                // Receiver dropped; nothing more to do.
                                break;
                            }
                        }
                        // Sentence didn't parse to a fix (e.g. unsupported
                        // type, or a no-fix GGA/RMC); keep reading.
                    }
                    // Just a wakeup to recheck `running`; not an error.
                    Err(ref err) if err.kind() == std::io::ErrorKind::TimedOut => continue,
                    // Device unplugged, permission revoked, etc.
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            rx,
            running,
            handle: Some(handle),
        })
    }
}

#[async_trait]
impl LocationSource for SerialGpsSource {
    async fn next_fix(&mut self) -> Option<GpsFix> {
        self.rx.recv().await
    }
}

impl Drop for SerialGpsSource {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_rejects_unsupported_baud_rate() {
        // `SerialGpsSource` doesn't implement `Debug` (it holds a live
        // thread/channel), so use `.err()` rather than `.unwrap_err()`
        // (which requires `T: Debug`).
        let err = SerialGpsSource::open("/dev/ttyUSB0", 1_200)
            .err()
            .expect("unsupported baud rate must be rejected");
        assert!(err.to_string().contains("unsupported baud rate"));
    }

    #[test]
    fn open_accepts_all_documented_baud_rates_before_the_device_check() {
        // We can't open a real device in CI, but every allowed baud rate
        // must get past the baud-validation guard and fail only once it
        // tries to actually open the (nonexistent) device - i.e. the error
        // must NOT be "unsupported baud rate".
        for baud in ALLOWED_BAUD_RATES {
            let err = SerialGpsSource::open("/dev/nonexistent-fluxfang-gps", baud)
                .err()
                .expect("opening a nonexistent device must fail");
            assert!(
                !err.to_string().contains("unsupported baud rate"),
                "baud {baud} should have passed validation, got: {err}"
            );
        }
    }
}
