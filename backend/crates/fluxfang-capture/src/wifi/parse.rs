//! Pure parsing of a radiotap-prefixed 802.11 frame into a [`WifiObservation`].
//!
//! [`parse_frame`] is deliberately hardware-free: it takes the exact byte
//! slice `pcap`/libpcap yields for `DLT_IEEE802_11_RADIO` (link type 127) and
//! returns `Option<WifiObservation>`, `None` on anything it doesn't
//! understand or that looks truncated. This lets it be unit-tested against a
//! committed `.pcap` fixture with no monitor-mode adapter involved -
//! [`crate::wifi::monitor::WifiMonitorCapturer`] is a thin wrapper that only
//! adds hardware I/O around this function.
//!
//! ## Frame types handled
//!
//! Four 802.11 management-frame subtypes are recognized (YAGNI - this slice
//! of FluxFang only cares about AP/client discovery and association, not
//! data frames, deauth, etc.):
//!
//! - **Beacon** (type=0 management, subtype=8 -> first frame-control byte
//!   `0x80`): sent periodically by an AP. `bssid` comes from **address 3**
//!   (bytes 16..22 of the 802.11 header), which for an AP-originated beacon
//!   is the BSSID (address 2, the transmitter, is normally identical for a
//!   beacon but address 3 is the canonical BSSID field per 802.11). `src_mac`
//!   is `None` for a beacon.
//! - **Probe request** (type=0 management, subtype=4 -> first frame-control
//!   byte `0x40`): sent by a client scanning for APs; it has no BSSID of its
//!   own (address 3 is typically the wildcard/broadcast address). Its actual
//!   identity is **address 2** (the transmitter - the probing client's MAC),
//!   which is stored in the distinct **`src_mac`** field rather than
//!   overloaded onto `bssid` - a probe request has no BSSID at all, so
//!   `bssid` is `None` for it. This is documented as a frame-type-dependent
//!   mapping, not a general "address 3 is always bssid" rule.
//! - **Association request** (type=0 management, subtype=0 -> first
//!   frame-control byte `0x00`) and **reassociation request** (subtype=2 ->
//!   `0x20`): sent by a client joining (or roaming to) an AP. **Address 2**
//!   (the transmitter) is the associating client, stored in `src_mac`.
//!   **Address 3** is the *target* AP's BSSID - not this frame's own
//!   identity, so it's stored in the distinct `target_bssid` field, never
//!   `bssid`. The SSID tag names the target network and is likewise stored
//!   in `target_ssid`, never `ssid`, after a fixed body of 4 bytes
//!   (association: capability info + listen interval) or 10 bytes
//!   (reassociation: capability info + listen interval + current AP
//!   address).
//!
//! Anything else (data frames, control frames, other management subtypes)
//! returns `None`.
//!
//! ## SSID
//!
//! The management-frame body starts with a fixed part (12 bytes for a
//! beacon: 8-byte timestamp + 2-byte beacon interval + 2-byte capability
//! info; 4 bytes for an association request; 10 bytes for a reassociation
//! request; 0 bytes for a probe request, which has no fixed part) followed by
//! tagged parameters (`tag id`, `tag len`, `tag value`, repeated). Tag id 0
//! is SSID. Its value is decoded as UTF-8 (lossy, since SSIDs are not
//! guaranteed valid UTF-8). A *present* SSID tag with zero length (a hidden
//! network deliberately broadcasting an empty SSID) yields `Some("")`; a
//! frame with *no* SSID tag at all (e.g. truncated before reaching one)
//! yields `None`.

use crate::RawObservation;
use chrono::{DateTime, Utc};
use serde_json::json;

/// A single WiFi management-frame observation extracted by [`parse_frame`].
#[derive(Debug, Clone, PartialEq)]
pub struct WifiObservation {
    /// The AP's BSSID (802.11 address 3, lowercase colon-separated MAC),
    /// present only for a beacon. `None` for a probe request, which has no
    /// BSSID of its own - see the module docs.
    pub bssid: Option<String>,
    /// The probing client's transmitter address (802.11 address 2,
    /// lowercase colon-separated MAC), present only for a probe request.
    /// `None` for a beacon.
    pub src_mac: Option<String>,
    /// Decoded SSID tag, if present. `Some("")` means a present-but-empty
    /// (hidden) SSID; `None` means no SSID tag was found at all. For a
    /// probe request this is the SSID the client is probing *for*
    /// (informational), not its own identity - the client's identity is
    /// `src_mac`.
    pub ssid: Option<String>,
    /// The AP a client is joining, from an association/reassociation request
    /// (802.11 address 3). `None` for beacon/probe frames. Kept distinct from
    /// `bssid` so an association emission identifies the *client* (`src_mac`)
    /// and never matches an AP emitter's `bssid` rule — see the module docs.
    pub target_bssid: Option<String>,
    /// The SSID a client is (re)associating to, from an association/
    /// reassociation request. `None` for beacon/probe frames.
    pub target_ssid: Option<String>,
    /// `"beacon"` or `"probe_request"`.
    pub frame_type: String,
    /// Channel number derived from the radiotap Channel field's frequency
    /// (MHz), if that field was present.
    pub channel: Option<u16>,
    /// Antenna signal in dBm from the radiotap header, if present.
    pub signal_strength: Option<i32>,
}

impl WifiObservation {
    /// Convert into the hardware-agnostic [`RawObservation`] the rest of the
    /// pipeline consumes. `observed_at` is supplied by the caller (the live
    /// capturer stamps `Utc::now()` at capture time; tests supply a fixed
    /// timestamp) - this function never reads the wall clock itself.
    pub fn into_raw_observation(self, observed_at: DateTime<Utc>) -> RawObservation {
        // The frame's own identity fields are frame-type-dependent (see the
        // module docs). Beacon/probe carry a plain `ssid` (possibly null for a
        // hidden network). A (re)association request must NOT carry a plain
        // `ssid`/`bssid`: the AP it names is a *target*, kept under
        // `target_ssid`/`target_bssid` so it can't match an AP emitter's
        // `ssid`/`bssid` rule and misattribute the association to the AP.
        let mut payload = match self.frame_type.as_str() {
            "association_request" | "reassociation_request" => json!({
                "frame_type": self.frame_type,
                "channel": self.channel,
            }),
            _ => json!({
                "ssid": self.ssid,
                "frame_type": self.frame_type,
                "channel": self.channel,
            }),
        };
        let obj = payload
            .as_object_mut()
            .expect("payload is always a JSON object");
        if let Some(bssid) = self.bssid {
            obj.insert("bssid".to_string(), json!(bssid));
        }
        if let Some(src_mac) = self.src_mac {
            obj.insert("src_mac".to_string(), json!(src_mac));
        }
        if let Some(target_bssid) = self.target_bssid {
            obj.insert("target_bssid".to_string(), json!(target_bssid));
        }
        if let Some(target_ssid) = self.target_ssid {
            obj.insert("target_ssid".to_string(), json!(target_ssid));
        }
        RawObservation {
            kind: "wifi".to_string(),
            observed_at,
            signal_strength: self.signal_strength,
            payload,
        }
    }
}

/// Frame-control byte 0 for a beacon: type=0 (management), subtype=8.
const FC_BEACON: u8 = 0x80;
/// Frame-control byte 0 for a probe request: type=0 (management), subtype=4.
const FC_PROBE_REQUEST: u8 = 0x40;
/// Frame-control byte 0 for an association request: type=0 (management),
/// subtype=0.
const FC_ASSOC_REQUEST: u8 = 0x00;
/// Frame-control byte 0 for a reassociation request: type=0 (management),
/// subtype=2.
const FC_REASSOC_REQUEST: u8 = 0x20;
/// Mask isolating type (bits 2-3) + subtype (bits 4-7) of frame control byte
/// 0 - i.e. everything except the protocol-version bits (0-1), which is
/// always `00` for the frames we care about.
const FC_TYPE_SUBTYPE_MASK: u8 = 0xfc;

/// Length of the fixed 802.11 MAC header common to all frame types: frame
/// control(2) + duration(2) + addr1(6) + addr2(6) + addr3(6) + seq
/// control(2).
const MAC_HEADER_LEN: usize = 24;
/// Length of a beacon's fixed body before tagged parameters begin:
/// timestamp(8) + beacon interval(2) + capability info(2).
const BEACON_FIXED_BODY_LEN: usize = 12;
/// Fixed body before an association request's tagged parameters: capability
/// info(2) + listen interval(2).
const ASSOC_REQ_FIXED_BODY_LEN: usize = 4;
/// Fixed body before a reassociation request's tagged parameters: capability
/// info(2) + listen interval(2) + current AP address(6).
const REASSOC_REQ_FIXED_BODY_LEN: usize = 10;

/// SSID information element tag id.
const TAG_SSID: u8 = 0;

/// Parse a radiotap-prefixed 802.11 frame (exactly what pcap yields for
/// `DLT_IEEE802_11_RADIO`) into a [`WifiObservation`].
///
/// Returns `None` for anything that isn't a beacon or probe request, or that
/// looks truncated/malformed at any parsing step - this function never
/// panics or indexes out of bounds on attacker/hardware-controlled input.
pub fn parse_frame(bytes: &[u8]) -> Option<WifiObservation> {
    // `radiotap::Radiotap::parse` reads the radiotap header (little-endian
    // it_version/it_pad/it_len/it_present, chaining further present-bitmap
    // words as needed) and hands back both the parsed fields and `rest`:
    // everything after the header's `it_len` bytes, i.e. exactly the 802.11
    // frame. It already guards truncated/malformed input by returning `Err`
    // rather than panicking, which we turn into `None` via `.ok()?`.
    let (radiotap, dot11) = radiotap::Radiotap::parse(bytes).ok()?;

    let signal_strength = radiotap.antenna_signal.map(|s| s.value as i32);
    let channel = radiotap.channel.and_then(|c| freq_to_channel(c.freq));

    if dot11.len() < MAC_HEADER_LEN {
        return None;
    }

    let fc_type_subtype = dot11[0] & FC_TYPE_SUBTYPE_MASK;
    let frame_type = match fc_type_subtype {
        FC_BEACON => "beacon",
        FC_PROBE_REQUEST => "probe_request",
        FC_ASSOC_REQUEST => "association_request",
        FC_REASSOC_REQUEST => "reassociation_request",
        _ => return None,
    };

    // addr2 = transmitter (bytes 10..16), addr3 (bytes 16..22).
    // - beacon: addr3 is the BSSID; no client.
    // - probe request: addr2 is the probing client; no BSSID.
    // - (re)association request: addr2 is the associating client (its
    //   identity), addr3 is the *target* AP's BSSID — a target, not this
    //   emission's own identity, so it goes in `target_bssid`, never `bssid`.
    let addr2 = &dot11[10..16];
    let addr3 = &dot11[16..22];
    let (bssid, src_mac, target_bssid) = match frame_type {
        "beacon" => (Some(format_mac(addr3)), None, None),
        "probe_request" => (None, Some(format_mac(addr2)), None),
        _ => (None, Some(format_mac(addr2)), Some(format_mac(addr3))),
    };

    let fixed_body_len = match frame_type {
        "beacon" => BEACON_FIXED_BODY_LEN,
        "association_request" => ASSOC_REQ_FIXED_BODY_LEN,
        "reassociation_request" => REASSOC_REQ_FIXED_BODY_LEN,
        _ => 0, // probe_request has no fixed body before its tags
    };
    let parsed_ssid = parse_ssid_tag(dot11, MAC_HEADER_LEN + fixed_body_len);
    // For a (re)association request the SSID tag names the *target* network,
    // so it's `target_ssid`, not the frame's own `ssid`.
    let (ssid, target_ssid) = match frame_type {
        "association_request" | "reassociation_request" => (None, parsed_ssid),
        _ => (parsed_ssid, None),
    };

    Some(WifiObservation {
        bssid,
        src_mac,
        ssid,
        target_bssid,
        target_ssid,
        frame_type: frame_type.to_string(),
        channel,
        signal_strength,
    })
}

/// Formats a 6-byte MAC address as lowercase colon-separated hex
/// (`aa:bb:cc:dd:ee:ff`). Caller guarantees `bytes.len() == 6`.
fn format_mac(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Maps a radiotap Channel field's frequency (MHz) to a WiFi channel number.
/// Covers 2.4GHz (channels 1-13, plus the special-cased channel 14) and
/// 5GHz; returns `None` for anything outside those ranges rather than
/// guessing.
///
/// `pub(crate)` (not private) so [`super::scan::parse_iw_scan`] can reuse
/// the exact same freq->channel mapping for `iw scan`'s `freq: <MHz>`
/// lines, rather than duplicating this table and risking the two capture
/// paths (monitor-mode radiotap vs. managed-mode `iw scan`) disagreeing on
/// what channel a given frequency means.
pub(crate) fn freq_to_channel(freq_mhz: u16) -> Option<u16> {
    match freq_mhz {
        2412..=2472 => Some((freq_mhz - 2407) / 5),
        2484 => Some(14),
        5000..=5900 => Some((freq_mhz - 5000) / 5),
        _ => None,
    }
}

/// Walks tagged parameters starting at `start` looking for the SSID tag
/// (id 0). Returns `Some(ssid)` (possibly `Some("")` for a present-but-empty
/// hidden SSID) if found, `None` if absent or if `start` is past the end of
/// `dot11` (e.g. a beacon with no tagged parameters at all - truncated or
/// minimal, either way not an error). Never indexes out of bounds: every
/// tag's declared length is checked against the remaining slice before use.
fn parse_ssid_tag(dot11: &[u8], start: usize) -> Option<String> {
    let mut idx = start;
    while idx + 2 <= dot11.len() {
        let tag_id = dot11[idx];
        let tag_len = dot11[idx + 1] as usize;
        let value_start = idx + 2;
        let value_end = value_start + tag_len;
        if value_end > dot11.len() {
            // Tag claims more data than is actually present - stop rather
            // than read past the end.
            break;
        }
        if tag_id == TAG_SSID {
            return Some(String::from_utf8_lossy(&dot11[value_start..value_end]).into_owned());
        }
        idx = value_end;
    }
    None
}
