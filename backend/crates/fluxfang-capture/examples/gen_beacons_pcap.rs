//! Generates the committed test fixture `tests/fixtures/beacons.pcap`.
//!
//! Run with:
//!
//! ```sh
//! cargo run -p fluxfang-capture --example gen_beacons_pcap
//! ```
//!
//! This is a one-off/reproducibility tool, not part of the build or test
//! run - the fixture it produces is committed to the repo so
//! `tests/wifi_parse.rs` never has to regenerate it (and never needs a real
//! monitor-mode adapter). Re-run this if the fixture's known values below
//! ever need to change; keep `tests/wifi_parse.rs`'s assertions in sync.
//!
//! ## Known values baked into the fixture (documented here, asserted there)
//!
//! Packet 1 - a beacon frame:
//! - BSSID (802.11 address 2 and address 3): `00:11:22:33:44:55`
//! - SSID: `"FluxTest"`
//! - radiotap antenna signal: `-42` dBm
//! - radiotap channel: frequency `2437` MHz -> channel `6`
//!
//! Packet 2 - a probe request frame:
//! - transmitter (802.11 address 2, used as `bssid` for probe requests):
//!   `aa:bb:cc:dd:ee:ff`
//! - address 1 and address 3 (destination / wildcard BSSID): broadcast
//!   `ff:ff:ff:ff:ff:ff`
//! - SSID: `"ProbeTest"`
//! - radiotap antenna signal: `-60` dBm
//! - radiotap channel: frequency `2412` MHz -> channel `1`
//!
//! Both packets use the same minimal radiotap header layout: a present
//! bitmap of `Channel | AntennaSignal` (bits 3 and 5, `0x00000028`), fields
//! appearing in bit order (Channel's 4 bytes: freq u16 LE + flags u16 LE,
//! then AntennaSignal's 1 byte, an `i8`), for a total radiotap header length
//! of 13 bytes (8-byte fixed header + 4-byte Channel + 1-byte AntennaSignal).

use pcap::{Capture, Linktype, Packet, PacketHeader};

fn mac(bytes: [u8; 6]) -> [u8; 6] {
    bytes
}

/// Builds a minimal radiotap header (version 0) with just the Channel and
/// AntennaSignal fields present - the two fields `parse_frame` reads.
fn radiotap_header(signal_dbm: i8, freq_mhz: u16, channel_flags: u16) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.push(0u8); // it_version
    buf.push(0u8); // it_pad
    buf.extend_from_slice(&13u16.to_le_bytes()); // it_len: total header length
    buf.extend_from_slice(&0x0000_0028u32.to_le_bytes()); // it_present: bit3 (Channel) | bit5 (AntennaSignal)
    buf.extend_from_slice(&freq_mhz.to_le_bytes()); // Channel: frequency (MHz)
    buf.extend_from_slice(&channel_flags.to_le_bytes()); // Channel: flags
    buf.push(signal_dbm as u8); // AntennaSignal: dBm, i8
    assert_eq!(buf.len(), 13, "radiotap header length must match it_len");
    buf
}

/// Builds a beacon frame's 802.11 bytes: 24-byte MAC header + 12-byte fixed
/// body (timestamp/interval/capability) + SSID tagged parameter.
fn beacon_80211(bssid: [u8; 6], ssid: &str) -> Vec<u8> {
    let mut buf = Vec::new();
    // Frame control: type=0 (management), subtype=8 (beacon) -> 0x80.
    buf.extend_from_slice(&[0x80, 0x00]);
    buf.extend_from_slice(&[0x00, 0x00]); // duration
    buf.extend_from_slice(&[0xff; 6]); // addr1: destination (broadcast)
    buf.extend_from_slice(&bssid); // addr2: transmitter (the AP)
    buf.extend_from_slice(&bssid); // addr3: BSSID
    buf.extend_from_slice(&[0x00, 0x00]); // sequence control
                                          // Fixed body (12 bytes): timestamp(8) + beacon interval(2) + capability(2).
    buf.extend_from_slice(&[0u8; 8]); // timestamp
    buf.extend_from_slice(&100u16.to_le_bytes()); // beacon interval (100 TU)
    buf.extend_from_slice(&0x0001u16.to_le_bytes()); // capability info
                                                     // Tagged parameters: SSID (tag id 0).
    buf.push(0x00);
    buf.push(ssid.len() as u8);
    buf.extend_from_slice(ssid.as_bytes());
    buf
}

/// Builds a probe request frame's 802.11 bytes: 24-byte MAC header + SSID
/// tagged parameter directly (no fixed body for probe requests).
fn probe_request_80211(transmitter: [u8; 6], ssid: &str) -> Vec<u8> {
    let mut buf = Vec::new();
    // Frame control: type=0 (management), subtype=4 (probe request) -> 0x40.
    buf.extend_from_slice(&[0x40, 0x00]);
    buf.extend_from_slice(&[0x00, 0x00]); // duration
    buf.extend_from_slice(&[0xff; 6]); // addr1: destination (broadcast)
    buf.extend_from_slice(&transmitter); // addr2: transmitter (the client)
    buf.extend_from_slice(&[0xff; 6]); // addr3: BSSID (wildcard - probing for any AP)
    buf.extend_from_slice(&[0x00, 0x00]); // sequence control
                                          // Tagged parameters only - probe requests have no fixed body.
    buf.push(0x00);
    buf.push(ssid.len() as u8);
    buf.extend_from_slice(ssid.as_bytes());
    buf
}

fn write_packet(savefile: &mut pcap::Savefile, ts_sec: i64, data: &[u8]) {
    let header = PacketHeader {
        ts: libc::timeval {
            tv_sec: ts_sec as _,
            tv_usec: 0,
        },
        caplen: data.len() as u32,
        len: data.len() as u32,
    };
    savefile.write(&Packet::new(&header, data));
}

fn main() -> anyhow::Result<()> {
    let out_path = format!("{}/tests/fixtures/beacons.pcap", env!("CARGO_MANIFEST_DIR"));

    let cap = Capture::dead(Linktype::IEEE802_11_RADIOTAP)?;
    let mut savefile = cap.savefile(&out_path)?;

    // Packet 1: beacon, BSSID 00:11:22:33:44:55, SSID "FluxTest",
    // -42 dBm, channel 6 (2437 MHz).
    let beacon_bssid = mac([0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
    let mut beacon_pkt = radiotap_header(-42, 2437, 0x00c0);
    beacon_pkt.extend_from_slice(&beacon_80211(beacon_bssid, "FluxTest"));
    write_packet(&mut savefile, 1_700_000_000, &beacon_pkt);

    // Packet 2: probe request, transmitter aa:bb:cc:dd:ee:ff, SSID
    // "ProbeTest", -60 dBm, channel 1 (2412 MHz).
    let probe_transmitter = mac([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
    let mut probe_pkt = radiotap_header(-60, 2412, 0x00c0);
    probe_pkt.extend_from_slice(&probe_request_80211(probe_transmitter, "ProbeTest"));
    write_packet(&mut savefile, 1_700_000_001, &probe_pkt);

    savefile.flush()?;
    println!("wrote {out_path}");
    Ok(())
}
