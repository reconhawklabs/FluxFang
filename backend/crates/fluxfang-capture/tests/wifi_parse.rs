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

#[test]
fn mid_tag_truncation_returns_none_without_panic() {
    // Same minimal 13-byte radiotap header as the fixture generator
    // (see `examples/gen_beacons_pcap.rs`): version 0, it_len=13,
    // present bitmap = Channel | AntennaSignal, then those two fields.
    let mut pkt = Vec::new();
    pkt.push(0u8); // it_version
    pkt.push(0u8); // it_pad
    pkt.extend_from_slice(&13u16.to_le_bytes()); // it_len
    pkt.extend_from_slice(&0x0000_0028u32.to_le_bytes()); // present: Channel | AntennaSignal
    pkt.extend_from_slice(&2437u16.to_le_bytes()); // Channel: freq
    pkt.extend_from_slice(&0x00c0u16.to_le_bytes()); // Channel: flags
    pkt.push((-42i8) as u8); // AntennaSignal

    // A valid 24-byte 802.11 beacon MAC header.
    pkt.extend_from_slice(&[0x80, 0x00]); // frame control: beacon
    pkt.extend_from_slice(&[0x00, 0x00]); // duration
    pkt.extend_from_slice(&[0xff; 6]); // addr1: broadcast
    pkt.extend_from_slice(&[0x00, 0x11, 0x22, 0x33, 0x44, 0x55]); // addr2
    pkt.extend_from_slice(&[0x00, 0x11, 0x22, 0x33, 0x44, 0x55]); // addr3 / bssid
    pkt.extend_from_slice(&[0x00, 0x00]); // sequence control

    // Fixed body (12 bytes): timestamp(8) + interval(2) + capability(2).
    pkt.extend_from_slice(&[0u8; 8]);
    pkt.extend_from_slice(&100u16.to_le_bytes());
    pkt.extend_from_slice(&0x0001u16.to_le_bytes());

    // SSID tag (id 0) declaring 50 bytes of value but only 3 actually
    // follow - the tag-walk bounds check in `parse_ssid_tag` must catch
    // this and stop rather than reading past the end of the slice.
    pkt.push(0x00); // tag id: SSID
    pkt.push(50); // declared tag length (way more than remains)
    pkt.extend_from_slice(b"abc"); // only 3 bytes actually present

    // The frame itself is well-formed (valid radiotap + valid MAC header),
    // so `parse_frame` still recognizes it as a beacon; only the SSID is
    // unresolvable because its tag is truncated, so it must come back as
    // `None` rather than panicking or reading out of bounds.
    let obs = parse_frame(&pkt).expect("well-formed beacon frame, just truncated SSID tag");
    assert_eq!(obs.frame_type, "beacon");
    assert_eq!(obs.ssid, None);
}
