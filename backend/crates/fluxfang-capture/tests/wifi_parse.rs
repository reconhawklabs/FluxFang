//! Tests `parse_frame` against the committed `tests/fixtures/beacons.pcap`
//! fixture - no monitor-mode hardware involved. See
//! `examples/gen_beacons_pcap.rs` for exactly how that fixture was built and
//! its documented known values (BSSID, SSID, signal, channel per packet).

use fluxfang_capture::wifi::parse_frame;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/beacons.pcap");

/// Loads every packet's raw bytes (radiotap header + 802.11 frame, exactly
/// what `pcap` yields for `DLT_IEEE802_11_RADIO`) from a `.pcap` file.
fn load_packets(path: &str) -> Vec<Vec<u8>> {
    let mut cap = pcap::Capture::from_file(path).expect("open fixture");
    let mut packets = Vec::new();
    while let Ok(pkt) = cap.next_packet() {
        packets.push(pkt.data.to_vec());
    }
    packets
}

#[test]
fn parses_bssid_ssid_rssi_from_fixture() {
    let packets = load_packets(FIXTURE);
    let pkt = &packets[0];

    let obs = parse_frame(pkt).expect("a management frame");
    assert_eq!(obs.frame_type, "beacon");
    assert_eq!(obs.bssid, "00:11:22:33:44:55");
    assert_eq!(obs.ssid, Some("FluxTest".to_string()));
    assert_eq!(obs.channel, Some(6));
    assert_eq!(obs.signal_strength, Some(-42));
}

#[test]
fn parses_probe_request_from_fixture() {
    let packets = load_packets(FIXTURE);
    let pkt = &packets[1];

    let obs = parse_frame(pkt).expect("a management frame");
    assert_eq!(obs.frame_type, "probe_request");
    // Probe requests have no BSSID of their own - `bssid` maps to the
    // transmitter address (802.11 address 2) instead. See the module docs
    // on `parse_frame` for why this differs from the beacon case.
    assert_eq!(obs.bssid, "aa:bb:cc:dd:ee:ff");
    assert_eq!(obs.ssid, Some("ProbeTest".to_string()));
    assert_eq!(obs.channel, Some(1));
    assert_eq!(obs.signal_strength, Some(-60));
}

#[test]
fn truncated_radiotap_header_returns_none_without_panic() {
    // Only 4 bytes: enough to start reading the radiotap header's `it_len`
    // field but not the present-bitmap word that follows it.
    let short = [0u8, 0, 13, 0];
    assert_eq!(parse_frame(&short), None);
}

#[test]
fn truncated_80211_header_returns_none_without_panic() {
    let packets = load_packets(FIXTURE);
    let full_beacon = &packets[0];
    // Keep the full (valid) radiotap header (13 bytes) but cut the 802.11
    // portion short of the 24-byte MAC header parse_frame requires.
    let truncated = &full_beacon[..13 + 10];
    assert_eq!(parse_frame(truncated), None);
}

#[test]
fn non_management_frame_returns_none() {
    let packets = load_packets(FIXTURE);
    let mut mutated = packets[0].clone();
    // Byte 13 is the first byte of the 802.11 frame control field (right
    // after the 13-byte radiotap header). Rewrite it from 0x80 (beacon) to
    // 0x08 (type=2 data, subtype=0) - not a management frame, so
    // `parse_frame` must return `None` rather than trying to interpret its
    // body as a beacon/probe request.
    mutated[13] = 0x08;
    assert_eq!(parse_frame(&mutated), None);
}

#[test]
fn empty_bytes_returns_none_without_panic() {
    assert_eq!(parse_frame(&[]), None);
}
