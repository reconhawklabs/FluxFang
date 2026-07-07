//! Byte-built association/reassociation-request frame tests for
//! `parse_frame` — no pcap fixture needed (mirrors the hand-built frame in
//! `tests/wifi_parse.rs::mid_tag_truncation_returns_none_without_panic`).

use fluxfang_capture::wifi::parse_frame;

/// Minimal 13-byte radiotap header (version 0, it_len 13, present bitmap
/// Channel|AntennaSignal, then those two fields) — same shape the fixture
/// generator uses.
fn radiotap_header(channel_freq: u16, signal_dbm: i8) -> Vec<u8> {
    let mut h = Vec::new();
    h.push(0u8); // it_version
    h.push(0u8); // it_pad
    h.extend_from_slice(&13u16.to_le_bytes()); // it_len
    h.extend_from_slice(&0x0000_0028u32.to_le_bytes()); // present: Channel | AntennaSignal
    h.extend_from_slice(&channel_freq.to_le_bytes()); // Channel: freq
    h.extend_from_slice(&0x00c0u16.to_le_bytes()); // Channel: flags
    h.push(signal_dbm as u8); // AntennaSignal
    h
}

/// Build a radiotap-prefixed (re)association-request frame:
/// - `fc0` = frame-control byte 0 (`0x00` assoc req, `0x20` reassoc req)
/// - addr1 = AP (RA), addr2 = client (TA), addr3 = BSSID (AP)
/// - `fixed_body` = the fixed params before tagged params (4 bytes for
///   assoc, 10 for reassoc)
/// - one SSID tag (id 0) carrying `ssid`
fn assoc_frame(fc0: u8, fixed_body: &[u8], ssid: &str) -> Vec<u8> {
    let mut pkt = radiotap_header(2437, -50);
    pkt.extend_from_slice(&[fc0, 0x00]); // frame control
    pkt.extend_from_slice(&[0x00, 0x00]); // duration
    pkt.extend_from_slice(&[0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]); // addr1: AP
    pkt.extend_from_slice(&[0x3a, 0xde, 0xad, 0xbe, 0xef, 0x00]); // addr2: client
    pkt.extend_from_slice(&[0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]); // addr3: BSSID
    pkt.extend_from_slice(&[0x00, 0x00]); // sequence control
    pkt.extend_from_slice(fixed_body);
    pkt.push(0x00); // tag id: SSID
    pkt.push(ssid.len() as u8);
    pkt.extend_from_slice(ssid.as_bytes());
    pkt
}

#[test]
fn parses_association_request_client_and_target_ap() {
    // Assoc fixed body: capability(2) + listen interval(2).
    let fixed = [0x01, 0x00, 0x0a, 0x00];
    let pkt = assoc_frame(0x00, &fixed, "HomeNet");

    let obs = parse_frame(&pkt).expect("association request parses");
    assert_eq!(obs.frame_type, "association_request");
    assert_eq!(obs.src_mac, Some("3a:de:ad:be:ef:00".to_string()));
    assert_eq!(obs.target_bssid, Some("aa:bb:cc:dd:ee:ff".to_string()));
    assert_eq!(obs.target_ssid, Some("HomeNet".to_string()));
    assert_eq!(obs.bssid, None);
    assert_eq!(obs.ssid, None);

    let raw = obs.into_raw_observation(chrono::Utc::now());
    assert_eq!(raw.payload["frame_type"], "association_request");
    assert_eq!(raw.payload["src_mac"], "3a:de:ad:be:ef:00");
    assert_eq!(raw.payload["target_bssid"], "aa:bb:cc:dd:ee:ff");
    assert_eq!(raw.payload["target_ssid"], "HomeNet");
    // Crucial: no plain `bssid`/`ssid` keys, or they would match AP emitter
    // rules and misattribute the association to the AP.
    assert!(raw.payload.get("bssid").is_none());
    assert!(raw.payload.get("ssid").is_none());
}

#[test]
fn parses_reassociation_request_reading_ssid_past_current_ap_field() {
    // Reassoc fixed body: capability(2) + listen interval(2) + current AP(6).
    // The SSID tag is 6 bytes further along than in an assoc frame; a wrong
    // offset would misread it.
    let fixed = [
        0x01, 0x00, // capability
        0x0a, 0x00, // listen interval
        0x99, 0x88, 0x77, 0x66, 0x55, 0x44, // current AP address
    ];
    let pkt = assoc_frame(0x20, &fixed, "CoffeeShop");

    let obs = parse_frame(&pkt).expect("reassociation request parses");
    assert_eq!(obs.frame_type, "reassociation_request");
    assert_eq!(obs.src_mac, Some("3a:de:ad:be:ef:00".to_string()));
    assert_eq!(obs.target_bssid, Some("aa:bb:cc:dd:ee:ff".to_string()));
    assert_eq!(obs.target_ssid, Some("CoffeeShop".to_string()));
}
