//! Tests `parse_iw_scan` against a realistic, hand-written multi-BSS `iw
//! dev <if> scan` output sample - no wifi hardware or `iw` binary
//! involved. Companion to `tests/wifi_parse.rs` (monitor-mode
//! `parse_frame`); see `src/wifi/scan.rs` for the parser itself and
//! `WifiScanCapturer`, the thin live wrapper around it.

use fluxfang_capture::wifi::parse_iw_scan;

/// A realistic multi-BSS `iw dev wlan0 scan` sample, covering:
/// - a normal 2.4GHz AP with a visible SSID (`aa:bb:cc:dd:ee:ff`, channel 6)
/// - a 5GHz AP the scanning host is already associated with, which `iw`
///   annotates with `-- associated` on the `BSS` line
///   (`11:22:33:44:55:66`, channel 36)
/// - a hidden network reporting a blank `SSID:` line
///   (`22:33:44:55:66:77`, channel 11)
/// - a block with unparseable `freq`/`signal` values, to confirm those
///   fields degrade to `None` rather than panicking while the SSID still
///   parses (`ff:ff:ff:ff:ff:ff`)
const SAMPLE: &str = r#"
BSS aa:bb:cc:dd:ee:ff(on wlan0)
	TSF: 1234567890123 usec (14288d, 23:17:03)
	freq: 2437
	beacon interval: 100 TUs
	capability: ESS Privacy ShortSlotTime (0x0411)
	signal: -42.00 dBm
	last seen: 120 ms ago
	SSID: FluxTest
	Supported rates: 1.0* 2.0* 5.5* 11.0* 6.0 9.0 12.0 18.0
	DS Parameter set: channel 6
	ERP: Barker_Preamble_Mode
BSS 11:22:33:44:55:66(on wlan0) -- associated
	TSF: 987654321 usec (11431d, 13:25:21)
	freq: 5180
	beacon interval: 100 TUs
	capability: ESS Privacy (0x0011)
	signal: -55.00 dBm
	last seen: 40 ms ago
	SSID: HomeNet5G
	DS Parameter set: channel 36
BSS 22:33:44:55:66:77(on wlan0)
	TSF: 555555 usec (6d, 10:00:00)
	freq: 2462
	signal: -71.00 dBm
	last seen: 500 ms ago
	SSID:
BSS ff:ff:ff:ff:ff:ff(on wlan0)
	TSF: 1 usec (0d, 00:00:00)
	freq: garbage-not-a-number
	signal: not-a-number dBm
	SSID: NoFreqNet
"#;

#[test]
fn parses_all_bss_blocks_in_order() {
    let obs = parse_iw_scan(SAMPLE);
    assert_eq!(obs.len(), 4, "expected 4 BSS blocks, got {obs:#?}");
}

#[test]
fn parses_normal_24ghz_ap_with_visible_ssid() {
    let obs = parse_iw_scan(SAMPLE);
    let ap = &obs[0];
    assert_eq!(ap.bssid, "aa:bb:cc:dd:ee:ff");
    assert_eq!(ap.ssid, Some("FluxTest".to_string()));
    assert_eq!(ap.signal_strength, Some(-42));
    assert_eq!(ap.channel, Some(6));
    assert_eq!(ap.frame_type, "beacon");
}

#[test]
fn parses_associated_5ghz_ap_ignoring_the_associated_suffix() {
    let obs = parse_iw_scan(SAMPLE);
    let ap = &obs[1];
    assert_eq!(ap.bssid, "11:22:33:44:55:66");
    assert_eq!(ap.ssid, Some("HomeNet5G".to_string()));
    assert_eq!(ap.signal_strength, Some(-55));
    assert_eq!(ap.channel, Some(36));
}

#[test]
fn hidden_network_with_blank_ssid_line_yields_none_ssid() {
    let obs = parse_iw_scan(SAMPLE);
    let ap = &obs[2];
    assert_eq!(ap.bssid, "22:33:44:55:66:77");
    assert_eq!(ap.ssid, None);
    assert_eq!(ap.signal_strength, Some(-71));
    assert_eq!(ap.channel, Some(11));
}

#[test]
fn unparseable_freq_and_signal_degrade_to_none_without_panicking() {
    let obs = parse_iw_scan(SAMPLE);
    let ap = &obs[3];
    assert_eq!(ap.bssid, "ff:ff:ff:ff:ff:ff");
    assert_eq!(ap.ssid, Some("NoFreqNet".to_string()));
    assert_eq!(ap.signal_strength, None);
    assert_eq!(ap.channel, None);
}

#[test]
fn empty_input_yields_no_observations() {
    assert_eq!(parse_iw_scan(""), Vec::new());
}

#[test]
fn garbage_input_without_a_bss_line_yields_no_observations() {
    assert_eq!(
        parse_iw_scan("this is not iw scan output\nfreq: 2437\nsignal: -40.00 dBm\n"),
        Vec::new()
    );
}

#[test]
fn malformed_bss_header_is_skipped_without_panicking() {
    // A `BSS` line whose leading token isn't a well-formed MAC address --
    // its fields must be skipped, not attributed to a bogus observation,
    // and parsing must continue with the next well-formed block.
    let input = r#"
BSS not-a-mac-address(on wlan0)
	freq: 2437
	signal: -42.00 dBm
	SSID: Bogus
BSS aa:bb:cc:dd:ee:ff(on wlan0)
	freq: 2412
	signal: -30.00 dBm
	SSID: Real
"#;
    let obs = parse_iw_scan(input);
    assert_eq!(
        obs.len(),
        1,
        "expected only the well-formed block: {obs:#?}"
    );
    assert_eq!(obs[0].bssid, "aa:bb:cc:dd:ee:ff");
    assert_eq!(obs[0].ssid, Some("Real".to_string()));
}

#[test]
fn truncated_output_ending_mid_block_still_yields_the_partial_block() {
    // No trailing newline / block cut off after the SSID line -- the last
    // in-progress block must still be flushed rather than silently
    // dropped.
    let input = "BSS aa:bb:cc:dd:ee:ff(on wlan0)\n\tfreq: 2412\n\tsignal: -50.00 dBm\n\tSSID: Cut";
    let obs = parse_iw_scan(input);
    assert_eq!(obs.len(), 1);
    assert_eq!(obs[0].ssid, Some("Cut".to_string()));
    assert_eq!(obs[0].channel, Some(1));
    assert_eq!(obs[0].signal_strength, Some(-50));
}
