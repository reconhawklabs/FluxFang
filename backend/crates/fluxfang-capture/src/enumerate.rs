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
    fn list_wifi_interfaces_never_panics() {
        let _ = list_wifi_interfaces();
    }

    #[test]
    fn list_serial_devices_never_panics() {
        let _ = list_serial_devices();
    }
}
