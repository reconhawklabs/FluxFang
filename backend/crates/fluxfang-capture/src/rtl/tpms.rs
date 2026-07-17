//! `rtl_433` subprocess capturer for TPMS. Spawns `rtl_433 -F json`, reads
//! its stdout line-by-line on a `std::thread`, and forwards each parsed
//! [`RawObservation`] via `blocking_send` — modeled on `gps::serial` (blocking
//! `BufReader::read_line` loop) and `wifi::scan` (subprocess).
//!
//! **Not unit-tested beyond [`rtl_433_args`]** — it spawns a real `rtl_433`
//! process against real SDR hardware, present in neither CI nor a dev box.
//! The pure line parsing it delegates to (`super::parse::parse_tpms_line`)
//! and the arg construction ([`rtl_433_args`]) are fully covered.

use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, bail};
use chrono::Utc;
use tokio::sync::mpsc;

use super::parse::parse_tpms_line;
use crate::{Capturer, RawObservation};

/// How long to wait after spawning `rtl_433` before checking whether it
/// already exited — long enough to catch an immediate failure (device busy,
/// no such device, bad args), short enough not to noticeably delay start.
const STARTUP_GRACE: Duration = Duration::from_millis(600);

/// Extract the most useful failure reason from a failed `rtl_433`'s captured
/// output (stderr, plus stdout appended) for surfacing as a data source's
/// `last_error`.
///
/// rtl_433 prints a noisy startup banner — a version line and a long
/// `Registered N out of M device decoding protocols [ ... ]` line — *before*
/// it tries to open the SDR, so naively taking the last output line often
/// shows that banner instead of the actual device-open failure (the confusing
/// "…exited immediately: Registered 191 out of 223 device decoding protocols"
/// message operators saw). This keeps only the lines that look like an error
/// (open failure, no/absent device, busy, permission) and joins them; if none
/// match it falls back to the last non-empty line, and to `"no output"` when
/// there was nothing at all.
fn summarize_rtl_error(captured: &str) -> String {
    // Substrings (matched case-insensitively) that mark a line as the real
    // failure reason rather than banner/progress noise.
    const ERROR_MARKERS: [&str; 9] = [
        "error",
        "fail",
        "no supported",
        "no matching",
        "no device",
        "busy",
        "permission",
        "not found",
        "no such",
    ];
    let lines: Vec<&str> = captured
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let error_lines: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            ERROR_MARKERS.iter().any(|marker| lower.contains(marker))
        })
        .collect();
    if !error_lines.is_empty() {
        return error_lines.join("; ");
    }
    lines.last().copied().unwrap_or("no output").to_string()
}

/// Build the `rtl_433` argument vector (everything after the binary name).
/// `frequency` is a literal rtl_433 frequency string (`"315M"` / `"433.92M"`).
/// A non-blank `device_serial` selects the dongle by stable serial via
/// `-d :SERIAL`; `None`/blank omits `-d` so rtl_433 uses device 0.
pub fn rtl_433_args(frequency: &str, device_serial: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "-f".to_string(),
        frequency.to_string(),
        "-M".to_string(),
        "level".to_string(),
        "-F".to_string(),
        "json".to_string(),
    ];
    if let Some(serial) = device_serial {
        if !serial.trim().is_empty() {
            args.push("-d".to_string());
            args.push(format!(":{}", serial.trim()));
        }
    }
    args
}

/// Spawns and streams `rtl_433` for TPMS. See module docs.
pub struct TpmsCapturer {
    frequency: String,
    device_serial: Option<String>,
    running: Arc<AtomicBool>,
    child: Option<Child>,
    handle: Option<thread::JoinHandle<()>>,
    stderr_handle: Option<thread::JoinHandle<()>>,
}

impl TpmsCapturer {
    pub fn new(frequency: String, device_serial: Option<String>) -> Self {
        Self {
            frequency,
            device_serial,
            running: Arc::new(AtomicBool::new(false)),
            child: None,
            handle: None,
            stderr_handle: None,
        }
    }
}

impl Capturer for TpmsCapturer {
    fn start(&mut self, tx: mpsc::Sender<RawObservation>) -> anyhow::Result<()> {
        if self.handle.is_some() {
            bail!("capturer already running");
        }
        let args = rtl_433_args(&self.frequency, self.device_serial.as_deref());
        let mut child = Command::new("rtl_433")
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| {
                anyhow!("failed to spawn rtl_433 (is it installed and on PATH?): {err}")
            })?;

        // Catch an immediate exit (e.g. device busy / not found): rtl_433
        // prints the reason to stderr and exits within a moment, so surface
        // it as this data source's last_error rather than a silent no-op.
        thread::sleep(STARTUP_GRACE);
        if let Ok(Some(status)) = child.try_wait() {
            // Read stderr AND stdout: the process has exited, so both pipes
            // are at EOF and fully readable, and rtl_433 doesn't consistently
            // put its device-open failure on one stream. `summarize_rtl_error`
            // then picks the real error line out of the startup banner noise.
            let mut captured = String::new();
            if let Some(mut stderr) = child.stderr.take() {
                let _ = stderr.read_to_string(&mut captured);
            }
            if let Some(mut stdout) = child.stdout.take() {
                let _ = stdout.read_to_string(&mut captured);
            }
            bail!(
                "rtl_433 exited immediately ({status}): {}",
                summarize_rtl_error(&captured)
            );
        }

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("rtl_433 stdout was not captured"))?;

        // Drain stderr on its own thread for the life of the process.
        // rtl_433 writes periodic diagnostics to stderr; if nobody reads them
        // the OS pipe buffer (~64KB on Linux) fills and rtl_433's write()
        // blocks, stalling its main loop and silently starving stdout. The
        // drain thread just discards the bytes and exits on EOF, which
        // arrives when stop() kills the child.
        let stderr_handle = child.stderr.take().map(|stderr| {
            thread::spawn(move || {
                let mut reader = BufReader::new(stderr);
                let mut scratch = String::new();
                loop {
                    scratch.clear();
                    match reader.read_line(&mut scratch) {
                        Ok(0) | Err(_) => break, // EOF or read error: child gone
                        Ok(_) => {}              // discard
                    }
                }
            })
        });

        self.running.store(true, Ordering::SeqCst);
        let running = self.running.clone();
        let handle = thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            while running.load(Ordering::SeqCst) {
                line.clear();
                match reader.read_line(&mut line) {
                    // EOF: rtl_433 exited (or was killed by stop()).
                    Ok(0) => break,
                    Ok(_) => {
                        if let Some(obs) = parse_tpms_line(&line, Utc::now()) {
                            if tx.blocking_send(obs).is_err() {
                                break; // receiver dropped
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        self.child = Some(child);
        self.handle = Some(handle);
        self.stderr_handle = stderr_handle;
        Ok(())
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        // Killing the child closes its stdout pipe, which makes the reader
        // thread's blocking read_line return Ok(0) so it exits promptly.
        // The same kill closes stderr, which lets the drain thread's read
        // loop hit EOF and return.
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.stderr_handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for TpmsCapturer {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_with_serial_include_device_selector() {
        let args = rtl_433_args("315M", Some("67475624"));
        assert_eq!(
            args,
            vec!["-f", "315M", "-M", "level", "-F", "json", "-d", ":67475624"]
        );
    }

    #[test]
    fn args_without_serial_omit_device_flag() {
        let args = rtl_433_args("433.92M", None);
        assert_eq!(args, vec!["-f", "433.92M", "-M", "level", "-F", "json"]);
    }

    #[test]
    fn blank_serial_is_treated_as_absent() {
        let args = rtl_433_args("315M", Some("   "));
        assert!(!args.iter().any(|a| a == "-d"));
    }

    #[test]
    fn summarize_error_skips_banner_and_returns_real_failure() {
        // Realistic rtl_433 startup + device-open failure (the case an
        // unplug/replug produces): the operative error is NOT the last line's
        // neighbour but is buried after the long protocols banner.
        let output = "\
rtl_433 version 21.12 branch  at ...
Registered 191 out of 223 device decoding protocols [ 1-4 8 11-12 ... 217-223 ]
Using device 0: Generic RTL2832U OEM
usb_open error -4
Failed to open rtlsdr device #0.
";
        let summary = summarize_rtl_error(output);
        assert!(
            summary.contains("usb_open error -4"),
            "should surface the usb_open error: {summary}"
        );
        assert!(
            summary.contains("Failed to open rtlsdr device"),
            "should surface the open-failure line: {summary}"
        );
        assert!(
            !summary.contains("Registered 191"),
            "must not surface the protocols banner: {summary}"
        );
    }

    #[test]
    fn summarize_error_catches_no_supported_devices() {
        let output = "\
Registered 191 out of 223 device decoding protocols [ ... ]
No supported devices found.
";
        assert_eq!(summarize_rtl_error(output), "No supported devices found.");
    }

    #[test]
    fn summarize_error_falls_back_to_last_line_when_no_marker() {
        // No error-marker line present: fall back to the last non-empty line
        // rather than dropping the (only) diagnostic we have.
        let output = "Registered 191 out of 223 device decoding protocols [ ... ]\n";
        assert_eq!(
            summarize_rtl_error(output),
            "Registered 191 out of 223 device decoding protocols [ ... ]"
        );
    }

    #[test]
    fn summarize_error_empty_output_is_no_output() {
        assert_eq!(summarize_rtl_error(""), "no output");
        assert_eq!(summarize_rtl_error("   \n  \n"), "no output");
    }
}
