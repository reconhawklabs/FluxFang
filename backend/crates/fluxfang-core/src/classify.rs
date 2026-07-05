//! Classification registry: turns a raw emission payload into a candidate
//! auto-created-emitter [`Classification`]. Pure, data-driven, and
//! extensible per data-source `kind` — WiFi rules live here now; Bluetooth /
//! RTL-SDR / sensors add new registry entries later with no schema change.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A candidate emitter classification derived from a single emission
/// payload, produced by [`classify`]. Mirrors the shape ingest will use to
/// get-or-create an auto-created emitter (see the design doc's "Ingest
/// changes" section): `identity_field`/`identity_value` become the visible
/// match rule's single condition, `name` and `attributes` seed the emitter
/// row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Classification {
    pub emitter_type: String,
    pub category: String,
    pub identity_field: String,
    pub identity_value: String,
    pub name: String,
    pub attributes: Value,
}

impl Classification {
    /// Stable de-dup key for get-or-create under concurrent ingest:
    /// `"<emitter_type>:<identity_value>"` (e.g.
    /// `"wifi_access_point:aa:bb:cc:dd:ee:ff"`).
    pub fn identity_key(&self) -> String {
        format!("{}:{}", self.emitter_type, self.identity_value)
    }
}

/// True if `mac` is a locally-administered / randomized MAC address: the
/// 2nd-least-significant bit of the first octet is set
/// (`first_octet & 0x02 != 0`). `mac` is expected in `"aa:bb:cc:dd:ee:ff"`
/// form; only the first octet (before the first `:`) is parsed. Malformed,
/// empty, or non-hex input returns `false` rather than panicking — this is
/// a best-effort signal, not a validator.
pub fn is_randomized_mac(mac: &str) -> bool {
    let first_octet = mac.split(':').next().unwrap_or("");
    match u8::from_str_radix(first_octet, 16) {
        Ok(byte) => byte & 0x02 != 0,
        Err(_) => false,
    }
}

/// Classify a raw emission `payload` for a data-source `kind` into a
/// candidate emitter [`Classification`], or `None` if the `kind`/payload
/// shape isn't recognized, or is recognized but lacks the field needed to
/// build a stable identity (e.g. a beacon with no `bssid`). Never panics on
/// missing or malformed payload fields — this is read as advisory data, not
/// validated input.
///
/// Extending to a new `kind` (Bluetooth, RTL-SDR, sensors, …) means adding a
/// new match arm here; no other code in this crate needs to change.
pub fn classify(kind: &str, payload: &Value) -> Option<Classification> {
    match kind {
        "wifi" => classify_wifi(payload),
        _ => None,
    }
}

fn classify_wifi(payload: &Value) -> Option<Classification> {
    match payload.get("frame_type").and_then(Value::as_str) {
        Some("beacon") => classify_wifi_beacon(payload),
        Some("probe_request") => classify_wifi_probe_request(payload),
        _ => None,
    }
}

/// `payload[key]` as a string, treating an absent field or an empty string
/// both as "not present" — an emission with `"ssid": ""` is exactly as
/// hidden as one with no `ssid` field at all.
fn non_empty_str(payload: &Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// AP beacon → `wifi_access_point`, identified by BSSID. No `bssid` means
/// no stable identity to key an emitter on, so this returns `None` rather
/// than fabricating one.
fn classify_wifi_beacon(payload: &Value) -> Option<Classification> {
    let bssid = non_empty_str(payload, "bssid")?;
    let ssid = non_empty_str(payload, "ssid");
    let ssid_label = ssid.as_deref().unwrap_or("Hidden");
    Some(Classification {
        emitter_type: "wifi_access_point".to_string(),
        category: "wifi".to_string(),
        identity_field: "bssid".to_string(),
        identity_value: bssid.clone(),
        name: format!("WiFi AP \"{ssid_label}\" ({bssid})"),
        attributes: serde_json::json!({
            "bssid": bssid,
            "ssid": ssid,
        }),
    })
}

/// Client probe request → `wifi_client`, identified by the client's source
/// MAC. No `src_mac` means no stable identity, so this returns `None`.
fn classify_wifi_probe_request(payload: &Value) -> Option<Classification> {
    let src_mac = non_empty_str(payload, "src_mac")?;
    let randomized = is_randomized_mac(&src_mac);
    Some(Classification {
        emitter_type: "wifi_client".to_string(),
        category: "wifi".to_string(),
        identity_field: "src_mac".to_string(),
        identity_value: src_mac.clone(),
        name: format!("WiFi Client {src_mac}"),
        attributes: serde_json::json!({
            "src_mac": src_mac,
            "randomized_mac": randomized,
        }),
    })
}

/// Human-readable label for an emitter type key, for UI display (e.g. the
/// Emitters page's `type_label`). The return type is `&'static str`, so an
/// unrecognized key can't be passed through by reference — it maps to the
/// literal `"Unknown"` instead.
pub fn emitter_type_label(type_key: &str) -> &'static str {
    match type_key {
        "wifi_access_point" => "WiFi Access Point",
        "wifi_client" => "WiFi Client",
        _ => "Unknown",
    }
}

/// Category key for an emitter type, for map/UI grouping (e.g. the overview
/// map's toggleable heatmap layers). Unrecognized keys map to `"other"`.
pub fn emitter_category(type_key: &str) -> &'static str {
    match type_key {
        "wifi_access_point" | "wifi_client" => "wifi",
        _ => "other",
    }
}

/// One emitter type as exposed to a caller building a "pick a type"
/// dropdown (see [`emitter_types_for_kind`]): a machine `key` (what gets
/// stored as `Emitter::emitter_type`/sent as `POST /api/emitters`'
/// `emitter_type`) paired with the same human-readable `label`
/// [`emitter_type_label`] would derive from that key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmitterTypeInfo {
    pub key: &'static str,
    pub label: &'static str,
}

/// The known emitter types for a data-source `kind`, for a frontend type
/// dropdown (replacing a free-text field). Data-driven and additive, same
/// spirit as [`classify`]: adding Bluetooth later means adding a new match
/// arm here, no other code changes. An unrecognized `kind` returns an empty
/// vec rather than erroring — mirrors `catalog_for`'s "unknown kind has no
/// fields" convention.
///
/// Labels here are kept in lockstep with [`emitter_type_label`] by
/// construction (both list the same key/label pairs) rather than by
/// deriving one from the other, since `emitter_type_label`'s signature
/// (`&str -> &'static str`) has no "which keys exist" direction to walk.
pub fn emitter_types_for_kind(kind: &str) -> Vec<EmitterTypeInfo> {
    match kind {
        "wifi" => vec![
            EmitterTypeInfo {
                key: "wifi_access_point",
                label: "WiFi Access Point",
            },
            EmitterTypeInfo {
                key: "wifi_client",
                label: "WiFi Client",
            },
        ],
        _ => Vec::new(),
    }
}

/// True if `type_key` is a recognized emitter type (i.e. appears in some
/// [`emitter_types_for_kind`] listing) — used to validate a caller-supplied
/// `emitter_type` on emitter creation before it's stored. Currently just
/// the wifi pair; grows alongside `emitter_types_for_kind`.
pub fn is_known_emitter_type(type_key: &str) -> bool {
    matches!(type_key, "wifi_access_point" | "wifi_client")
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- is_randomized_mac -------------------------------------------------

    #[test]
    fn randomized_mac_true_for_known_locally_administered() {
        assert!(is_randomized_mac("02:00:00:00:00:00"));
        // 0x3a = 0011_1010: bit1 (0x02) set.
        assert!(is_randomized_mac("3a:de:ad:be:ef:00"));
    }

    #[test]
    fn randomized_mac_false_for_globally_administered_vendor_mac() {
        assert!(!is_randomized_mac("00:11:22:33:44:55"));
        // 0x3c = 0011_1100: bit1 (0x02) clear.
        assert!(!is_randomized_mac("3c:15:c2:00:00:00"));
    }

    #[test]
    fn randomized_mac_false_for_malformed_input_never_panics() {
        assert!(!is_randomized_mac(""));
        assert!(!is_randomized_mac("not-a-mac"));
        assert!(!is_randomized_mac("zz:bb:cc:dd:ee:ff"));
        assert!(!is_randomized_mac(":bb:cc:dd:ee:ff"));
    }

    // -- classify: beacon ---------------------------------------------------

    #[test]
    fn classify_wifi_beacon_with_ssid() {
        let payload = serde_json::json!({
            "frame_type": "beacon",
            "bssid": "aa:bb:cc:dd:ee:ff",
            "ssid": "HomeNet",
            "channel": 6
        });
        let c = classify("wifi", &payload).expect("beacon with bssid classifies");
        assert_eq!(c.emitter_type, "wifi_access_point");
        assert_eq!(c.category, "wifi");
        assert_eq!(c.identity_field, "bssid");
        assert_eq!(c.identity_value, "aa:bb:cc:dd:ee:ff");
        assert_eq!(c.name, "WiFi AP \"HomeNet\" (aa:bb:cc:dd:ee:ff)");
        assert_eq!(
            c.attributes,
            serde_json::json!({"bssid": "aa:bb:cc:dd:ee:ff", "ssid": "HomeNet"})
        );
        assert_eq!(c.identity_key(), "wifi_access_point:aa:bb:cc:dd:ee:ff");
    }

    #[test]
    fn classify_wifi_beacon_hidden_ssid_when_ssid_empty_or_absent() {
        let empty = serde_json::json!({
            "frame_type": "beacon",
            "bssid": "aa:bb:cc:dd:ee:ff",
            "ssid": ""
        });
        let c = classify("wifi", &empty).unwrap();
        assert_eq!(c.name, "WiFi AP \"Hidden\" (aa:bb:cc:dd:ee:ff)");
        assert_eq!(c.attributes["ssid"], serde_json::Value::Null);

        let absent = serde_json::json!({
            "frame_type": "beacon",
            "bssid": "aa:bb:cc:dd:ee:ff"
        });
        let c2 = classify("wifi", &absent).unwrap();
        assert_eq!(c2.name, "WiFi AP \"Hidden\" (aa:bb:cc:dd:ee:ff)");
        assert_eq!(c2.attributes["ssid"], serde_json::Value::Null);
    }

    #[test]
    fn classify_wifi_beacon_missing_bssid_is_none() {
        let payload = serde_json::json!({"frame_type": "beacon", "ssid": "HomeNet"});
        assert!(classify("wifi", &payload).is_none());

        let empty_bssid = serde_json::json!({"frame_type": "beacon", "bssid": ""});
        assert!(classify("wifi", &empty_bssid).is_none());
    }

    // -- classify: probe_request --------------------------------------------

    #[test]
    fn classify_wifi_probe_request() {
        let payload = serde_json::json!({
            "frame_type": "probe_request",
            "src_mac": "3a:de:ad:be:ef:00"
        });
        let c = classify("wifi", &payload).expect("probe_request with src_mac classifies");
        assert_eq!(c.emitter_type, "wifi_client");
        assert_eq!(c.category, "wifi");
        assert_eq!(c.identity_field, "src_mac");
        assert_eq!(c.identity_value, "3a:de:ad:be:ef:00");
        assert_eq!(c.name, "WiFi Client 3a:de:ad:be:ef:00");
        assert_eq!(
            c.attributes,
            serde_json::json!({"src_mac": "3a:de:ad:be:ef:00", "randomized_mac": true})
        );
        assert_eq!(c.identity_key(), "wifi_client:3a:de:ad:be:ef:00");
    }

    #[test]
    fn classify_wifi_probe_request_non_randomized_mac_flag_false() {
        let payload = serde_json::json!({
            "frame_type": "probe_request",
            "src_mac": "00:11:22:33:44:55"
        });
        let c = classify("wifi", &payload).unwrap();
        assert_eq!(c.attributes["randomized_mac"], serde_json::json!(false));
    }

    #[test]
    fn classify_wifi_probe_request_missing_src_mac_is_none() {
        let payload = serde_json::json!({"frame_type": "probe_request"});
        assert!(classify("wifi", &payload).is_none());
    }

    // -- classify: unrecognized shapes ---------------------------------------

    #[test]
    fn classify_unknown_frame_type_and_kind_is_none() {
        assert!(classify("wifi", &serde_json::json!({"frame_type": "data"})).is_none());
        assert!(classify("wifi", &serde_json::json!({})).is_none());
        assert!(classify(
            "bluetooth",
            &serde_json::json!({"frame_type": "beacon", "bssid": "aa:bb:cc:dd:ee:ff"})
        )
        .is_none());
    }

    // -- emitter_type_label / emitter_category -------------------------------

    #[test]
    fn emitter_type_label_known_and_unknown() {
        assert_eq!(emitter_type_label("wifi_access_point"), "WiFi Access Point");
        assert_eq!(emitter_type_label("wifi_client"), "WiFi Client");
        assert_eq!(emitter_type_label("bluetooth_device"), "Unknown");
    }

    #[test]
    fn emitter_category_known_and_unknown() {
        assert_eq!(emitter_category("wifi_access_point"), "wifi");
        assert_eq!(emitter_category("wifi_client"), "wifi");
        assert_eq!(emitter_category("bluetooth_device"), "other");
    }

    // -- emitter_types_for_kind / is_known_emitter_type ----------------------

    #[test]
    fn emitter_types_for_kind_wifi_lists_both_types_with_matching_labels() {
        let types = emitter_types_for_kind("wifi");
        assert_eq!(
            types,
            vec![
                EmitterTypeInfo {
                    key: "wifi_access_point",
                    label: "WiFi Access Point",
                },
                EmitterTypeInfo {
                    key: "wifi_client",
                    label: "WiFi Client",
                },
            ]
        );
        // Consistent with emitter_type_label for every listed key.
        for t in &types {
            assert_eq!(emitter_type_label(t.key), t.label);
        }
    }

    #[test]
    fn emitter_types_for_kind_unknown_kind_is_empty() {
        assert_eq!(emitter_types_for_kind("bluetooth"), Vec::new());
        assert_eq!(emitter_types_for_kind(""), Vec::new());
    }

    #[test]
    fn is_known_emitter_type_true_for_wifi_types_false_otherwise() {
        assert!(is_known_emitter_type("wifi_access_point"));
        assert!(is_known_emitter_type("wifi_client"));
        assert!(!is_known_emitter_type("bluetooth_device"));
        assert!(!is_known_emitter_type(""));
    }
}
