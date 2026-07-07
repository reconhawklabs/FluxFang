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
    /// When `Some`, ingest persists this rule verbatim as the auto-created
    /// emitter's `match_criteria` instead of the default single
    /// `identity_field == identity_value` condition. wifi classifiers set
    /// `None` (behavior unchanged); bluetooth uses it for the
    /// randomized-but-named OR rule. See the design doc.
    pub match_criteria: Option<crate::rule::Rule>,
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
        "bluetooth" => classify_bluetooth(payload),
        _ => None,
    }
}

fn classify_wifi(payload: &Value) -> Option<Classification> {
    match payload.get("frame_type").and_then(Value::as_str) {
        Some("beacon") => classify_wifi_beacon(payload),
        Some("probe_request") => classify_wifi_probe_request(payload),
        Some("association_request") | Some("reassociation_request") => {
            classify_wifi_association(payload)
        }
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
        match_criteria: None,
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
        match_criteria: None,
    })
}

/// Client association/reassociation request → `wifi_client`, identified by
/// the client's source MAC — the same identity as a probe request, so an
/// association collapses onto that client's existing emitter rather than
/// making a second one. Seeds the client's latest connected AP
/// (`connected_bssid`/`connected_ssid`) from the frame's `target_bssid`/
/// `target_ssid` for the case where a client's *first-ever* emission is an
/// association (ingest additionally merges these onto an already-existing
/// client — see the wifi-association design doc). No `src_mac` → no stable
/// identity → `None`.
fn classify_wifi_association(payload: &Value) -> Option<Classification> {
    let src_mac = non_empty_str(payload, "src_mac")?;
    let randomized = is_randomized_mac(&src_mac);
    let connected_bssid = non_empty_str(payload, "target_bssid");
    let connected_ssid = non_empty_str(payload, "target_ssid");
    Some(Classification {
        emitter_type: "wifi_client".to_string(),
        category: "wifi".to_string(),
        identity_field: "src_mac".to_string(),
        identity_value: src_mac.clone(),
        name: format!("WiFi Client {src_mac}"),
        attributes: serde_json::json!({
            "src_mac": src_mac,
            "randomized_mac": randomized,
            "connected_bssid": connected_bssid,
            "connected_ssid": connected_ssid,
        }),
        match_criteria: None,
    })
}

/// A bluetooth advertisement → `bluetooth_device`. Identity depends on
/// address type + name presence (see the design doc's identity table):
/// - public address → identity on `address`, default `address == …` rule;
/// - random address **with** a local name → identity on `name` (so a
///   rotating RPA re-advertising the same name collapses onto one emitter),
///   with an OR rule `ANY[name == …, address == …]`;
/// - random address without a name → identity on `address`, default rule.
///
/// No `address` → no stable identity → `None`.
fn classify_bluetooth(payload: &Value) -> Option<Classification> {
    if payload.get("frame_type").and_then(Value::as_str) != Some("advertisement") {
        return None;
    }
    let address = non_empty_str(payload, "address")?;
    // Whitespace-only names (e.g. "   ") are treated as absent: only the
    // shared `non_empty_str` empty-string filter is applied elsewhere, but
    // bluetooth additionally needs the whitespace-only case collapsed to
    // "no name" so it doesn't silently key an emitter's identity on
    // whitespace (see `classify_bluetooth_empty_name_treated_as_unnamed`).
    let name = non_empty_str(payload, "name").filter(|n| !n.trim().is_empty());
    let address_type = non_empty_str(payload, "address_type");
    let randomized = match address_type.as_deref() {
        Some("random") => true,
        Some(_) => false,
        None => is_randomized_mac(&address),
    };
    let is_public = matches!(address_type.as_deref(), Some("public"));

    let vendor = bluetooth_vendor(payload, &address, is_public);
    let device_type = bluetooth_device_type(payload);

    let mut attributes = serde_json::json!({
        "address": address,
        "randomized_mac": randomized,
    });
    if let Some(at) = &address_type {
        attributes["address_type"] = Value::String(at.clone());
    }
    if let Some(n) = &name {
        attributes["name"] = Value::String(n.clone());
    }
    if let Some(v) = vendor {
        attributes["vendor"] = Value::String(v.to_string());
    }
    if let Some(dt) = device_type {
        attributes["device_type"] = Value::String(dt.to_string());
    }
    if let Some(cid) = payload.get("company_id").and_then(Value::as_u64) {
        attributes["company_id"] = Value::from(cid);
    }

    let display_name = match &name {
        Some(n) => format!("BT Client \"{n}\" ({address})"),
        None => format!("BT Client ({address})"),
    };

    // Identity + optional rule override.
    let (identity_field, identity_value, match_criteria) = match (randomized, &name) {
        (true, Some(n)) => (
            "name".to_string(),
            n.clone(),
            Some(crate::rule::Rule {
                match_mode: crate::rule::MatchMode::Any,
                conditions: vec![
                    crate::rule::Condition {
                        field: "name".to_string(),
                        op: crate::rule::Op::Eq,
                        value: Value::String(n.clone()),
                    },
                    crate::rule::Condition {
                        field: "address".to_string(),
                        op: crate::rule::Op::Eq,
                        value: Value::String(address.clone()),
                    },
                ],
            }),
        ),
        _ => ("address".to_string(), address.clone(), None),
    };

    Some(Classification {
        emitter_type: "bluetooth_device".to_string(),
        category: "bluetooth".to_string(),
        identity_field,
        identity_value,
        name: display_name,
        attributes,
        match_criteria,
    })
}

/// Vendor attribution: prefer the SIG company id (survives RPA); else fall
/// back to the OUI vendor for **public** addresses only.
fn bluetooth_vendor(payload: &Value, address: &str, is_public: bool) -> Option<&'static str> {
    if let Some(id) = payload.get("company_id").and_then(Value::as_u64) {
        if let Ok(id) = u16::try_from(id) {
            if let Some(name) = crate::bluetooth::company_name(id) {
                return Some(name);
            }
        }
    }
    if is_public {
        return crate::bluetooth::oui_vendor(address);
    }
    None
}

/// Device-type attribution: prefer classic Class-of-Device, else BLE
/// Appearance.
fn bluetooth_device_type(payload: &Value) -> Option<&'static str> {
    if let Some(cod) = payload.get("class_of_device").and_then(Value::as_u64) {
        if let Ok(cod) = u32::try_from(cod) {
            if let Some(dt) = crate::bluetooth::cod_device_type(cod) {
                return Some(dt);
            }
        }
    }
    if let Some(appearance) = payload.get("appearance").and_then(Value::as_u64) {
        if let Ok(appearance) = u16::try_from(appearance) {
            return crate::bluetooth::appearance_device_type(appearance);
        }
    }
    None
}

/// Human-readable label for an emitter type key, for UI display (e.g. the
/// Emitters page's `type_label`). The return type is `&'static str`, so an
/// unrecognized key can't be passed through by reference — it maps to the
/// literal `"Unknown"` instead.
pub fn emitter_type_label(type_key: &str) -> &'static str {
    match type_key {
        "wifi_access_point" => "WiFi Access Point",
        "wifi_client" => "WiFi Client",
        "bluetooth_device" => "Bluetooth Device",
        _ => "Unknown",
    }
}

/// Category key for an emitter type, for map/UI grouping (e.g. the overview
/// map's toggleable heatmap layers). Unrecognized keys map to `"other"`.
pub fn emitter_category(type_key: &str) -> &'static str {
    match type_key {
        "wifi_access_point" | "wifi_client" => "wifi",
        "bluetooth_device" => "bluetooth",
        _ => "other",
    }
}

/// The data-source `kind` whose field catalog and emission set a rule for an
/// emitter of type `emitter_type` should be validated/backfilled against.
/// Bluetooth emitter types map to `"bluetooth"`; everything else (including
/// `None`/free-text emitters) defaults to `"wifi"`, preserving the original
/// wifi-only behavior of the emitter-rule endpoints.
pub fn catalog_kind_for(emitter_type: Option<&str>) -> &'static str {
    match emitter_type.map(emitter_category) {
        Some("bluetooth") => "bluetooth",
        _ => "wifi",
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
        "bluetooth" => vec![EmitterTypeInfo {
            key: "bluetooth_device",
            label: "Bluetooth Device",
        }],
        _ => Vec::new(),
    }
}

/// True if `type_key` is a recognized emitter type (i.e. appears in some
/// [`emitter_types_for_kind`] listing) — used to validate a caller-supplied
/// `emitter_type` on emitter creation before it's stored. Currently just
/// the wifi pair; grows alongside `emitter_types_for_kind`.
pub fn is_known_emitter_type(type_key: &str) -> bool {
    matches!(
        type_key,
        "wifi_access_point" | "wifi_client" | "bluetooth_device"
    )
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

    // -- classify: association / reassociation ------------------------------

    #[test]
    fn classify_wifi_association_request_is_client_with_connected_ap() {
        let payload = serde_json::json!({
            "frame_type": "association_request",
            "src_mac": "3a:de:ad:be:ef:00",
            "target_bssid": "aa:bb:cc:dd:ee:ff",
            "target_ssid": "HomeNet"
        });
        let c = classify("wifi", &payload).expect("association classifies");
        assert_eq!(c.emitter_type, "wifi_client");
        assert_eq!(c.category, "wifi");
        assert_eq!(c.identity_field, "src_mac");
        assert_eq!(c.identity_value, "3a:de:ad:be:ef:00");
        assert_eq!(c.name, "WiFi Client 3a:de:ad:be:ef:00");
        assert_eq!(c.attributes["src_mac"], "3a:de:ad:be:ef:00");
        assert_eq!(c.attributes["randomized_mac"], serde_json::json!(true));
        assert_eq!(c.attributes["connected_bssid"], "aa:bb:cc:dd:ee:ff");
        assert_eq!(c.attributes["connected_ssid"], "HomeNet");
        assert_eq!(c.identity_key(), "wifi_client:3a:de:ad:be:ef:00");
    }

    #[test]
    fn classify_wifi_reassociation_request_also_client_null_ssid_when_absent() {
        let payload = serde_json::json!({
            "frame_type": "reassociation_request",
            "src_mac": "00:11:22:33:44:55",
            "target_bssid": "aa:bb:cc:dd:ee:ff"
        });
        let c = classify("wifi", &payload).unwrap();
        assert_eq!(c.emitter_type, "wifi_client");
        assert_eq!(c.attributes["connected_bssid"], "aa:bb:cc:dd:ee:ff");
        assert_eq!(c.attributes["connected_ssid"], serde_json::Value::Null);
    }

    #[test]
    fn classify_wifi_association_missing_src_mac_is_none() {
        let payload = serde_json::json!({
            "frame_type": "association_request",
            "target_bssid": "aa:bb:cc:dd:ee:ff"
        });
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
        assert_eq!(emitter_type_label("bluetooth_device"), "Bluetooth Device");
    }

    #[test]
    fn emitter_category_known_and_unknown() {
        assert_eq!(emitter_category("wifi_access_point"), "wifi");
        assert_eq!(emitter_category("wifi_client"), "wifi");
        assert_eq!(emitter_category("bluetooth_device"), "bluetooth");
    }

    #[test]
    fn catalog_kind_for_bluetooth_types_maps_to_bluetooth() {
        assert_eq!(catalog_kind_for(Some("bluetooth_device")), "bluetooth");
    }

    #[test]
    fn catalog_kind_for_wifi_types_none_and_unknown_default_to_wifi() {
        assert_eq!(catalog_kind_for(Some("wifi_access_point")), "wifi");
        assert_eq!(catalog_kind_for(Some("wifi_client")), "wifi");
        assert_eq!(catalog_kind_for(None), "wifi");
        assert_eq!(catalog_kind_for(Some("something_unrecognized")), "wifi");
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
        assert_eq!(emitter_types_for_kind(""), Vec::new());
    }

    #[test]
    fn emitter_types_for_kind_bluetooth_lists_device() {
        let types = emitter_types_for_kind("bluetooth");
        assert_eq!(
            types,
            vec![EmitterTypeInfo {
                key: "bluetooth_device",
                label: "Bluetooth Device",
            }]
        );
    }

    #[test]
    fn is_known_emitter_type_true_for_wifi_types_false_otherwise() {
        assert!(is_known_emitter_type("wifi_access_point"));
        assert!(is_known_emitter_type("wifi_client"));
        assert!(is_known_emitter_type("bluetooth_device"));
        assert!(!is_known_emitter_type(""));
    }

    // -- classify: bluetooth ------------------------------------------------

    #[test]
    fn classify_bluetooth_public_named_uses_address_identity_and_vendor() {
        let payload = serde_json::json!({
            "frame_type": "advertisement",
            "address": "3c:15:c2:aa:bb:cc",
            "address_type": "public",
            "name": "Study Speaker",
            "company_id": 76
        });
        let c = classify("bluetooth", &payload).expect("advertisement classifies");
        assert_eq!(c.emitter_type, "bluetooth_device");
        assert_eq!(c.category, "bluetooth");
        assert_eq!(c.identity_field, "address");
        assert_eq!(c.identity_value, "3c:15:c2:aa:bb:cc");
        assert_eq!(c.name, "BT Client \"Study Speaker\" (3c:15:c2:aa:bb:cc)");
        assert_eq!(c.identity_key(), "bluetooth_device:3c:15:c2:aa:bb:cc");
        assert_eq!(c.attributes["randomized_mac"], serde_json::json!(false));
        assert_eq!(c.attributes["vendor"], "Apple, Inc.");
        // Public + named → default single-condition rule (None override).
        assert!(c.match_criteria.is_none());
    }

    #[test]
    fn classify_bluetooth_random_named_uses_name_identity_and_or_rule() {
        use crate::rule::{MatchMode, Op};
        let payload = serde_json::json!({
            "frame_type": "advertisement",
            "address": "7a:11:22:33:44:55",
            "address_type": "random",
            "name": "Johns iPhone",
            "company_id": 76
        });
        let c = classify("bluetooth", &payload).unwrap();
        assert_eq!(c.identity_field, "name");
        assert_eq!(c.identity_value, "Johns iPhone");
        assert_eq!(c.identity_key(), "bluetooth_device:Johns iPhone");
        assert_eq!(c.name, "BT Client \"Johns iPhone\" (7a:11:22:33:44:55)");
        assert_eq!(c.attributes["randomized_mac"], serde_json::json!(true));
        let rule = c
            .match_criteria
            .expect("random+named yields an override rule");
        assert_eq!(rule.match_mode, MatchMode::Any);
        assert_eq!(rule.conditions.len(), 2);
        assert_eq!(rule.conditions[0].field, "name");
        assert_eq!(rule.conditions[0].op, Op::Eq);
        assert_eq!(rule.conditions[0].value, serde_json::json!("Johns iPhone"));
        assert_eq!(rule.conditions[1].field, "address");
        assert_eq!(
            rule.conditions[1].value,
            serde_json::json!("7a:11:22:33:44:55")
        );
    }

    #[test]
    fn classify_bluetooth_random_unnamed_uses_address_identity() {
        let payload = serde_json::json!({
            "frame_type": "advertisement",
            "address": "7a:11:22:33:44:55",
            "address_type": "random"
        });
        let c = classify("bluetooth", &payload).unwrap();
        assert_eq!(c.identity_field, "address");
        assert_eq!(c.identity_value, "7a:11:22:33:44:55");
        assert_eq!(c.name, "BT Client (7a:11:22:33:44:55)");
        assert!(c.match_criteria.is_none());
    }

    #[test]
    fn classify_bluetooth_empty_name_treated_as_unnamed() {
        let payload = serde_json::json!({
            "frame_type": "advertisement",
            "address": "7a:11:22:33:44:55",
            "address_type": "random",
            "name": ""
        });
        let c = classify("bluetooth", &payload).unwrap();
        assert_eq!(c.identity_field, "address");
    }

    #[test]
    fn classify_bluetooth_whitespace_only_name_treated_as_unnamed() {
        let payload = serde_json::json!({
            "frame_type": "advertisement",
            "address": "7a:11:22:33:44:55",
            "address_type": "random",
            "name": "   "
        });
        let c = classify("bluetooth", &payload).unwrap();
        assert_eq!(c.identity_field, "address");
        assert_eq!(c.identity_value, "7a:11:22:33:44:55");
        assert_eq!(c.name, "BT Client (7a:11:22:33:44:55)");
        assert!(c.attributes.get("name").is_none());
        assert!(c.match_criteria.is_none());
    }

    #[test]
    fn classify_bluetooth_missing_address_is_none() {
        let payload = serde_json::json!({"frame_type": "advertisement", "name": "x"});
        assert!(classify("bluetooth", &payload).is_none());
    }

    #[test]
    fn classify_bluetooth_device_type_from_class_of_device() {
        let payload = serde_json::json!({
            "frame_type": "advertisement",
            "address": "3c:15:c2:aa:bb:cc",
            "address_type": "public",
            "class_of_device": 0x5A020C
        });
        let c = classify("bluetooth", &payload).unwrap();
        assert_eq!(c.attributes["device_type"], "Phone");
    }
}
