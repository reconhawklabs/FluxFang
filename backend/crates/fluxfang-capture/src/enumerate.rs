//! Hardware enumeration for the "pick a device from a dropdown" UX (Task
//! hardware-enumeration): lists wireless network interfaces and candidate
//! serial GPS device paths so the WebUI never has to ask a user to type an
//! interface/device name from memory.
//!
//! Both entry points ([`list_wifi_interfaces`], [`list_serial_devices`]) are
//! thin filesystem-walking wrappers around pure, unit-tested helpers
//! ([`parse_iw_dev`], [`filter_wireless`]) — the walking itself talks to
//! `/sys`/`/dev` and (for the `iw` fallback) spawns a subprocess, neither of
//! which is practical to unit test hermetically, but the parsing/filtering
//! logic they delegate to is fully covered without touching real hardware.
//!
//! Never panics: every I/O error (missing `/sys/class/net`, missing `iw`
//! binary, missing `/dev/serial/by-id`, ...) is swallowed and treated as "no
//! results from this source", falling through to the next detection method
//! or simply yielding an empty `Vec`.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Interface names known to have a `wireless` subdirectory under
/// `/sys/class/net/<name>/`, sorted and deduped. This is the canonical
/// kernel-exposed "is this a wireless interface" check (present for every
/// `cfg80211`-backed driver), so it's tried before the `iw dev`-parsing
/// fallback.
///
/// Never panics: if `/sys/class/net` doesn't exist or can't be read (e.g.
/// non-Linux dev environment, restricted container), this yields an empty
/// list and [`list_wifi_interfaces`] falls back to `iw dev`.
fn wireless_interfaces_from_sys(sys_class_net: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(sys_class_net) else {
        return Vec::new();
    };

    let candidates: Vec<(String, bool)> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            let is_wireless = entry.path().join("wireless").is_dir();
            Some((name, is_wireless))
        })
        .collect();

    filter_wireless(&candidates)
}

/// Pure filter/dedup/sort step, factored out of [`wireless_interfaces_from_sys`]
/// so it's unit-testable against an in-memory fixture rather than a real (or
/// temp-dir-simulated) `/sys/class/net`: given a list of `(interface name, has
/// a "wireless" subdirectory)` pairs, keeps only the wireless ones, dedupes,
/// and sorts the result for stable output.
fn filter_wireless(candidates: &[(String, bool)]) -> Vec<String> {
    let mut names: Vec<String> = candidates
        .iter()
        .filter(|(_, is_wireless)| *is_wireless)
        .map(|(name, _)| name.clone())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Parses `iw dev` output for `Interface <name>` lines. Pure and
/// panic-free: unrecognized lines are simply ignored, and malformed/empty
/// input yields an empty `Vec`.
///
/// Sample `iw dev` output this handles:
/// ```text
/// phy#0
///     Interface wlan0
///         ifindex 3
///         wdev 0x1
///         addr aa:bb:cc:dd:ee:ff
///         type managed
/// phy#1
///     Interface wlan1
///         ifindex 4
///         type monitor
/// ```
fn parse_iw_dev(output: &str) -> Vec<String> {
    let mut names: Vec<String> = output
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            trimmed
                .strip_prefix("Interface ")
                .map(|name| name.trim().to_string())
        })
        .filter(|name| !name.is_empty())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Runs `iw dev` and parses its stdout via [`parse_iw_dev`]. Never panics:
/// a missing `iw` binary, non-zero exit, or non-UTF-8 output all yield an
/// empty `Vec` rather than propagating an error.
fn wireless_interfaces_from_iw() -> Vec<String> {
    let Ok(output) = std::process::Command::new("iw").arg("dev").output() else {
        return Vec::new();
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_iw_dev(&stdout)
}

/// Lists wireless network interface names, sorted and deduped.
///
/// Primary detection: `/sys/class/net/*/wireless` (see
/// [`wireless_interfaces_from_sys`]). Falls back to parsing `iw dev` output
/// if the `/sys` walk yields nothing (e.g. `/sys` unavailable, or present
/// but `iw` still sees interfaces `/sys` doesn't — belt and suspenders).
/// Never panics; returns an empty `Vec` if neither source has anything.
pub fn list_wifi_interfaces() -> Vec<String> {
    let from_sys = wireless_interfaces_from_sys(Path::new("/sys/class/net"));
    if !from_sys.is_empty() {
        return from_sys;
    }
    wireless_interfaces_from_iw()
}

/// Lists entries of a directory as `dir/entry_name` path strings, sorted.
/// Never panics: a missing/unreadable directory yields an empty `Vec`.
fn list_dir_paths(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.path().to_str().map(|s| s.to_string()))
        .collect();
    paths.sort();
    paths
}

/// Lists entries of `dir` whose file name matches `prefix<digits>` (e.g.
/// `ttyUSB0`, `ttyACM12`) as full path strings, sorted. Never panics: a
/// missing/unreadable directory yields an empty `Vec`.
fn list_matching_dev_nodes(dir: &Path, prefix: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            let suffix = name.strip_prefix(prefix)?;
            if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
                entry.path().to_str().map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect();
    paths.sort();
    paths
}

/// Lists candidate serial GPS device paths, sorted and deduped.
///
/// Prefers stable `/dev/serial/by-id/*` symlinks (survive USB re-enumeration
/// across reboots/replugs, unlike `/dev/ttyUSB0`-style names) when that
/// directory exists; also always includes plain `/dev/ttyUSB*`/`/dev/ttyACM*`
/// device nodes, since not every GPS dongle's driver populates
/// `/dev/serial/by-id`. Never panics; returns an empty `Vec` on a host with
/// no serial devices (or no `/dev` at all, e.g. a restricted dev sandbox).
pub fn list_serial_devices() -> Vec<String> {
    let mut paths = list_dir_paths(Path::new("/dev/serial/by-id"));
    paths.extend(list_matching_dev_nodes(Path::new("/dev"), "ttyUSB"));
    paths.extend(list_matching_dev_nodes(Path::new("/dev"), "ttyACM"));
    paths.sort();
    paths.dedup();
    paths
}

/// Filter `/sys/class/bluetooth` entry names to HCI adapter names
/// (`hci0`, `hci1`, …), sorted and deduped. Pure; unit-testable against a
/// fixture list.
fn filter_hci_adapters(names: &[String]) -> Vec<String> {
    let mut out: Vec<String> = names
        .iter()
        .filter(|n| {
            n.strip_prefix("hci")
                .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
        })
        .cloned()
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Lists Bluetooth HCI adapter names by walking `/sys/class/bluetooth`.
/// Never panics: a missing/unreadable directory (no adapter, restricted
/// container) yields an empty `Vec`.
pub fn list_bluetooth_adapters() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(Path::new("/sys/class/bluetooth")) else {
        return Vec::new();
    };
    let names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
        .collect();
    filter_hci_adapters(&names)
}

/// One RTL-SDR dongle as reported by `rtl_test` — its enumeration `index`
/// (what rtl_433's `-d N` uses), a human `name` (vendor + product), and its
/// stable USB `serial` (what we persist and pass as `-d :SERIAL`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RtlSdrDevice {
    pub index: u32,
    pub name: String,
    pub serial: String,
}

/// Parse `rtl_test`'s device-list lines, e.g.
/// `  0:  Nooelec, NESDR SMArTee v5, SN: 67475624`. Pure and panic-free:
/// non-device lines ("Found N device(s):", "Using device 0: …", blanks) are
/// ignored. A line with no `SN:` or an unparseable index is skipped.
fn parse_rtl_test(output: &str) -> Vec<RtlSdrDevice> {
    output.lines().filter_map(parse_rtl_test_line).collect()
}

fn parse_rtl_test_line(line: &str) -> Option<RtlSdrDevice> {
    let line = line.trim();
    let (idx_str, rest) = line.split_once(':')?;
    let index: u32 = idx_str.trim().parse().ok()?;
    let (name_part, sn_part) = rest.rsplit_once("SN:")?;
    let serial = sn_part.trim().to_string();
    if serial.is_empty() {
        return None;
    }
    let name = name_part.trim().trim_end_matches(',').trim().to_string();
    Some(RtlSdrDevice {
        index,
        name,
        serial,
    })
}

/// Lists connected RTL-SDR dongles by running `rtl_test` briefly and parsing
/// its device listing (printed to stderr before it opens device 0), then
/// killing it so it never runs its benchmark loop. Never panics: a missing
/// `rtl_test` binary, or a device already in use, yields whatever the listing
/// contained (possibly empty). The listing is printed even when opening the
/// device subsequently fails, so an in-use dongle still enumerates.
pub fn list_rtl_sdr_devices() -> Vec<RtlSdrDevice> {
    use std::io::BufRead;
    use std::process::{Command, Stdio};

    let Ok(mut child) = Command::new("rtl_test")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    else {
        return Vec::new();
    };

    let mut text = String::new();
    if let Some(stderr) = child.stderr.take() {
        let mut reader = std::io::BufReader::new(stderr);
        let mut line = String::new();
        // Read until rtl_test starts opening a device (right after the list)
        // or the stream ends; guard with a line cap so a chatty build can't
        // spin here.
        for _ in 0..64 {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    text.push_str(&line);
                    if line.trim_start().starts_with("Using device") {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    parse_rtl_test(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A realistic multi-interface `iw dev` sample (two phys, one managed
    /// wifi interface each) — including nested/indented detail lines that
    /// must NOT be mistaken for interface names.
    const IW_DEV_SAMPLE: &str = "\
phy#0
\tInterface wlan0
\t\tifindex 3
\t\twdev 0x1
\t\taddr aa:bb:cc:dd:ee:ff
\t\tssid MyNetwork
\t\ttype managed
\t\tchannel 6 (2437 MHz), width: 20 MHz, center1: 2437 MHz
phy#1
\tInterface wlan1
\t\tifindex 4
\t\taddr 11:22:33:44:55:66
\t\ttype monitor
";

    #[test]
    fn parse_iw_dev_extracts_interface_names() {
        let names = parse_iw_dev(IW_DEV_SAMPLE);
        assert_eq!(names, vec!["wlan0".to_string(), "wlan1".to_string()]);
    }

    #[test]
    fn parse_iw_dev_empty_output_yields_empty() {
        assert_eq!(parse_iw_dev(""), Vec::<String>::new());
    }

    #[test]
    fn parse_iw_dev_dedupes_and_sorts() {
        let output =
            "phy#0\n\tInterface wlan1\nphy#1\n\tInterface wlan0\nphy#2\n\tInterface wlan1\n";
        assert_eq!(
            parse_iw_dev(output),
            vec!["wlan0".to_string(), "wlan1".to_string()]
        );
    }

    #[test]
    fn parse_iw_dev_ignores_lines_without_interface_prefix() {
        let output = "phy#0\n\tsome other line\n\tInterface wlan0\n";
        assert_eq!(parse_iw_dev(output), vec!["wlan0".to_string()]);
    }

    #[test]
    fn filter_wireless_keeps_only_wireless_interfaces_sorted_deduped() {
        let candidates = vec![
            ("eth0".to_string(), false),
            ("wlan1".to_string(), true),
            ("lo".to_string(), false),
            ("wlan0".to_string(), true),
            ("wlan0".to_string(), true), // duplicate entry
        ];
        assert_eq!(
            filter_wireless(&candidates),
            vec!["wlan0".to_string(), "wlan1".to_string()]
        );
    }

    #[test]
    fn filter_wireless_empty_input_yields_empty() {
        assert_eq!(filter_wireless(&[]), Vec::<String>::new());
    }

    #[test]
    fn filter_wireless_no_wireless_interfaces_yields_empty() {
        let candidates = vec![("eth0".to_string(), false), ("lo".to_string(), false)];
        assert_eq!(filter_wireless(&candidates), Vec::<String>::new());
    }

    #[test]
    fn wireless_interfaces_from_sys_missing_dir_yields_empty() {
        // A path that (almost certainly) doesn't exist rather than a real
        // `/sys/class/net` — exercises the "never panics on missing /sys"
        // path directly, deterministically, regardless of the host running
        // this test suite.
        let missing = Path::new("/nonexistent/fluxfang-test-fixture/class/net");
        assert_eq!(wireless_interfaces_from_sys(missing), Vec::<String>::new());
    }

    #[test]
    fn wireless_interfaces_from_sys_detects_wireless_subdir() {
        let tmp = std::env::temp_dir().join(format!(
            "fluxfang-enumerate-test-{}-{}",
            std::process::id(),
            "sys_wireless_detect"
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("wlan0/wireless")).unwrap();
        std::fs::create_dir_all(tmp.join("eth0")).unwrap();
        std::fs::create_dir_all(tmp.join("lo")).unwrap();

        let mut result = wireless_interfaces_from_sys(&tmp);
        result.sort();
        assert_eq!(result, vec!["wlan0".to_string()]);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn list_matching_dev_nodes_filters_prefix_and_digits() {
        let tmp = std::env::temp_dir().join(format!(
            "fluxfang-enumerate-test-{}-{}",
            std::process::id(),
            "dev_nodes"
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("ttyUSB0"), b"").unwrap();
        std::fs::write(tmp.join("ttyUSB1"), b"").unwrap();
        std::fs::write(tmp.join("ttyS0"), b"").unwrap();
        std::fs::write(tmp.join("ttyUSBnotanumber"), b"").unwrap();

        let result = list_matching_dev_nodes(&tmp, "ttyUSB");
        let expected: Vec<String> = vec![
            tmp.join("ttyUSB0").to_str().unwrap().to_string(),
            tmp.join("ttyUSB1").to_str().unwrap().to_string(),
        ];
        assert_eq!(result, expected);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn list_dir_paths_missing_dir_yields_empty() {
        let missing = Path::new("/nonexistent/fluxfang-test-fixture/serial/by-id");
        assert_eq!(list_dir_paths(missing), Vec::<String>::new());
    }

    /// End-to-end smoke tests for the two public entry points: they must
    /// never panic regardless of what hardware (or lack thereof) the test
    /// runner has. Contents aren't asserted since they're host-dependent;
    /// only that the call returns at all.
    #[test]
    fn filter_hci_adapters_keeps_only_hci_names_sorted_deduped() {
        let names = vec![
            "hci1".to_string(),
            "rfkill".to_string(),
            "hci0".to_string(),
            "hci0".to_string(),
            "hcixyz".to_string(),
        ];
        assert_eq!(
            filter_hci_adapters(&names),
            vec!["hci0".to_string(), "hci1".to_string()]
        );
    }

    #[test]
    fn list_bluetooth_adapters_never_panics() {
        let _ = list_bluetooth_adapters();
    }

    #[test]
    fn list_wifi_interfaces_never_panics() {
        let _ = list_wifi_interfaces();
    }

    #[test]
    fn list_serial_devices_never_panics() {
        let _ = list_serial_devices();
    }

    const RTL_TEST_SAMPLE: &str = "\
Found 2 device(s):
  0:  Nooelec, NESDR SMArTee v5, SN: 67475624
  1:  Realtek, RTL2838UHIDIR, SN: 00000001
Using device 0: Generic RTL2832U OEM
";

    #[test]
    fn parse_rtl_test_extracts_index_name_serial() {
        let devices = parse_rtl_test(RTL_TEST_SAMPLE);
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].index, 0);
        assert_eq!(devices[0].name, "Nooelec, NESDR SMArTee v5");
        assert_eq!(devices[0].serial, "67475624");
        assert_eq!(devices[1].index, 1);
        assert_eq!(devices[1].serial, "00000001");
    }

    #[test]
    fn parse_rtl_test_ignores_non_device_lines() {
        assert!(parse_rtl_test("Found 0 device(s):\n").is_empty());
        assert!(parse_rtl_test("").is_empty());
        assert!(parse_rtl_test("No supported devices found.\n").is_empty());
    }

    #[test]
    fn list_rtl_sdr_devices_never_panics() {
        let _ = list_rtl_sdr_devices();
    }
}
