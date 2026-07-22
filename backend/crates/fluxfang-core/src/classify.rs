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
    /// How long this emitter's identifying address is expected to persist,
    /// for emitters identified by a MAC/BLE address. `None` for emitter
    /// types with no address at all (TPMS sensor ids), which the
    /// per-data-source retention gate treats as always-retained — there is
    /// no randomization to filter.
    pub persistence: Option<MacPersistence>,
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

/// How long the address that identifies an emitter can be expected to stay
/// the same. A single `randomized_mac: bool` collapses five very different
/// lifetimes into one flag — a BLE static-random address (stable until the
/// device reboots, i.e. weeks) and a resolvable private address (rotates
/// every ~15 minutes) are both "randomized", but only one of them is worth
/// tracking. This enum is that distinction, and it drives three things: the
/// UI badge ([`MacPersistence::badge`]), the emission/emitter filters, and
/// the per-data-source retention gate ([`MacPersistence::retained_at`]).
///
/// Ordered most- to least-persistent; [`MacPersistence::rank`] is that
/// order as a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MacPersistence {
    /// Not randomized at all: a globally-administered vendor MAC or a BLE
    /// public address. Persists for the life of the hardware.
    Stable,
    /// Randomized but derived per network and reused on every reconnect —
    /// a Wi-Fi (re)association MAC (iOS "Private Wi-Fi Address", Android
    /// per-network randomization) or a virtual-AP/hotspot BSSID. Persists
    /// for months, until the network is forgotten.
    PerNetwork,
    /// Randomized and stable for a power cycle: a BLE static-random
    /// address. Persists days to weeks — most peripherals rarely reboot.
    Session,
    /// Randomized and short-lived: a Wi-Fi probe-request MAC or a BLE
    /// resolvable private address. Rotates on the order of minutes.
    Ephemeral,
    /// Randomized and deliberately un-linkable: a BLE non-resolvable
    /// private address. Cannot be correlated across rotations at all.
    Unlinkable,
}

impl MacPersistence {
    /// Every class, most- to least-persistent. The canonical ordering for
    /// building a retention-level dropdown.
    pub const ALL: [MacPersistence; 5] = [
        MacPersistence::Stable,
        MacPersistence::PerNetwork,
        MacPersistence::Session,
        MacPersistence::Ephemeral,
        MacPersistence::Unlinkable,
    ];

    /// Wire/storage token — what lands in `attributes.mac_persistence` and
    /// what the `mac_persistence=` filter accepts.
    pub fn as_str(&self) -> &'static str {
        match self {
            MacPersistence::Stable => "stable",
            MacPersistence::PerNetwork => "per_network",
            MacPersistence::Session => "session",
            MacPersistence::Ephemeral => "ephemeral",
            MacPersistence::Unlinkable => "unlinkable",
        }
    }

    /// Parse a wire token back into a class. Unrecognized input is `None`
    /// rather than a default, so a bad filter/config value can be rejected
    /// at the boundary instead of silently meaning something.
    pub fn parse(s: &str) -> Option<Self> {
        MacPersistence::ALL.into_iter().find(|c| c.as_str() == s)
    }

    /// Persistence rank: higher is longer-lived. Only the ordering is
    /// meaningful — see [`MacPersistence::retained_at`].
    pub fn rank(&self) -> u8 {
        match self {
            MacPersistence::Stable => 4,
            MacPersistence::PerNetwork => 3,
            MacPersistence::Session => 2,
            MacPersistence::Ephemeral => 1,
            MacPersistence::Unlinkable => 0,
        }
    }

    /// The badge this class shows in the UI, and the value the badge filter
    /// matches on. `None` means no badge — the address isn't randomized.
    ///
    /// Both `PerNetwork` and `Session` are `"randomized-longterm"`: they're
    /// randomized, but persist long enough to be worth tracking, which is
    /// exactly the distinction the plain `"randomized"` badge was hiding.
    pub fn badge(&self) -> Option<&'static str> {
        match self {
            MacPersistence::Stable => None,
            MacPersistence::PerNetwork | MacPersistence::Session => Some("randomized-longterm"),
            MacPersistence::Ephemeral | MacPersistence::Unlinkable => Some("randomized"),
        }
    }

    /// Whether this class is randomized at all — the back-compatible
    /// `attributes.randomized_mac` boolean, kept so existing rules, saved
    /// filters, and the operator's manual override keep working unchanged.
    pub fn is_randomized(&self) -> bool {
        !matches!(self, MacPersistence::Stable)
    }

    /// Whether an emission of this class is kept by a data source whose
    /// retention level is `level`: a level keeps its own class and every
    /// more-persistent one. Selecting `Session` stores `Session`,
    /// `PerNetwork`, and `Stable`, and drops `Ephemeral`/`Unlinkable`.
    pub fn retained_at(&self, level: MacPersistence) -> bool {
        self.rank() >= level.rank()
    }
}

/// Every value the `mac_persistence=` filter accepts, in menu order: the
/// two badges first, then each exact class. Shared by the REST filters, the
/// MCP tool schemas, and the frontend dropdowns so all three stay in step.
pub const PERSISTENCE_FILTER_TOKENS: [&str; 7] = [
    "randomized",
    "randomized-longterm",
    "stable",
    "per_network",
    "session",
    "ephemeral",
    "unlinkable",
];

/// Expand a `mac_persistence=` filter token into the classes it selects, or
/// `None` if the token isn't recognized (which callers surface as a 400
/// rather than silently matching nothing).
///
/// Two kinds of token are accepted. The **badges** are the coarse buckets
/// the UI shows — `"randomized"` selects the short-lived classes only
/// (`ephemeral`, `unlinkable`), and `"randomized-longterm"` selects the
/// classes that persist long enough to track (`per_network`, `session`).
/// The **exact class names** (`"session"`, `"ephemeral"`, …) select just
/// that one class, for when the badge granularity isn't enough.
///
/// Note that `"randomized"` deliberately does *not* mean "any randomized
/// address": it means the badge of that name. Selecting every randomized
/// class means selecting both badges.
pub fn persistence_filter_classes(token: &str) -> Option<Vec<MacPersistence>> {
    if let Some(class) = MacPersistence::parse(token) {
        return Some(vec![class]);
    }
    let classes: Vec<MacPersistence> = MacPersistence::ALL
        .into_iter()
        .filter(|c| c.badge() == Some(token))
        .collect();
    (!classes.is_empty()).then_some(classes)
}

/// The persistence class of a Wi-Fi address, from the frame it was seen in.
///
/// The frame type is what separates the two randomized Wi-Fi lifetimes, and
/// it's the only thing that can: the same phone emits a throwaway MAC in a
/// probe request and its stable per-SSID MAC in an association request, and
/// the address alone looks identical in both cases (locally-administered
/// bit set). A non-randomized address is [`MacPersistence::Stable`]
/// regardless of frame type.
pub fn wifi_persistence(frame_type: &str, mac: &str) -> MacPersistence {
    // An AP's BSSID is always `stable`, locally-administered or not. The LA
    // bit on a BSSID means "secondary virtual interface" -- one physical
    // radio broadcasting several SSIDs -- not privacy randomization, and
    // such a BSSID is fixed for the life of the AP's config. Treating it as
    // randomized would badge a large fraction of ordinary infrastructure
    // (40% of the APs in a dense urban capture) as something it isn't.
    if frame_type == "beacon" {
        return MacPersistence::Stable;
    }
    if !is_randomized_mac(mac) {
        return MacPersistence::Stable;
    }
    match frame_type {
        // The MAC a client actually connects with: derived per network and
        // reused on every reconnect.
        "association_request" | "reassociation_request" => MacPersistence::PerNetwork,
        // Probe requests, and anything else unattributed: throwaway.
        _ => MacPersistence::Ephemeral,
    }
}

/// The persistence class of a Bluetooth address.
///
/// For a random address the class lives in the **top two bits of the first
/// octet** (Core spec's address-type encoding), not in the
/// locally-administered bit: a static-random address like `db:…` has the LA
/// bit *clear*, so the 802.11 test would call it a stable vendor MAC. The
/// top-two-bits test is only applied when the controller told us the
/// address is random (`address_type == "random"`) — a public address may
/// coincidentally start with those bits (`c8:9f:bb…` is a real OUI) and
/// must not be reinterpreted.
///
/// When `address_type` is absent the subtype is unknowable, so this falls
/// back to the LA-bit test and reports the conservative
/// [`MacPersistence::Ephemeral`] for a randomized address.
pub fn bluetooth_persistence(address_type: Option<&str>, address: &str) -> MacPersistence {
    match address_type {
        Some("random") => ble_random_subtype(address),
        Some(_) => MacPersistence::Stable,
        None if is_randomized_mac(address) => MacPersistence::Ephemeral,
        None => MacPersistence::Stable,
    }
}

/// Split a BLE *random* address by its top two bits: `11` static random,
/// `01` resolvable private, `00` non-resolvable private. `10` is reserved
/// by the spec and shouldn't appear; it and malformed input report
/// [`MacPersistence::Ephemeral`], the class that claims the least.
fn ble_random_subtype(address: &str) -> MacPersistence {
    let first_octet = address.split(':').next().unwrap_or("");
    match u8::from_str_radix(first_octet, 16) {
        Ok(byte) => match byte >> 6 {
            0b11 => MacPersistence::Session,
            0b01 => MacPersistence::Ephemeral,
            0b00 => MacPersistence::Unlinkable,
            _ => MacPersistence::Ephemeral,
        },
        Err(_) => MacPersistence::Ephemeral,
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
        "tpms" => classify_tpms(payload),
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
    let persistence = wifi_persistence("beacon", &bssid);
    let mut attributes = serde_json::json!({
        "bssid": bssid,
        "ssid": ssid,
        "mac_persistence": persistence.as_str(),
    });
    let attr_obj = attributes.as_object_mut().expect("object");
    for key in [
        "security",
        "auth",
        "cipher",
        "transition_mode",
        "security_label",
    ] {
        if let Some(v) = payload.get(key) {
            attr_obj.insert(key.to_string(), v.clone());
        }
    }
    Some(Classification {
        emitter_type: "wifi_access_point".to_string(),
        category: "wifi".to_string(),
        identity_field: "bssid".to_string(),
        identity_value: bssid.clone(),
        name: format!("WiFi AP \"{ssid_label}\" ({bssid})"),
        attributes,
        match_criteria: None,
        persistence: Some(persistence),
    })
}

/// Client probe request → `wifi_client`, identified by the client's source
/// MAC. No `src_mac` means no stable identity, so this returns `None`.
fn classify_wifi_probe_request(payload: &Value) -> Option<Classification> {
    let src_mac = non_empty_str(payload, "src_mac")?;
    let persistence = wifi_persistence("probe_request", &src_mac);
    Some(Classification {
        emitter_type: "wifi_client".to_string(),
        category: "wifi".to_string(),
        identity_field: "src_mac".to_string(),
        identity_value: src_mac.clone(),
        name: format!("WiFi Client {src_mac}"),
        attributes: serde_json::json!({
            "src_mac": src_mac,
            "randomized_mac": persistence.is_randomized(),
            "mac_persistence": persistence.as_str(),
        }),
        match_criteria: None,
        persistence: Some(persistence),
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
    // An association carries the MAC the client actually connects with,
    // which is derived per network and reused on every reconnect — a much
    // longer-lived address than the same device's probe-request MAC, even
    // though both have the locally-administered bit set.
    let persistence = wifi_persistence("association_request", &src_mac);
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
            "randomized_mac": persistence.is_randomized(),
            "mac_persistence": persistence.as_str(),
            "connected_bssid": connected_bssid,
            "connected_ssid": connected_ssid,
        }),
        match_criteria: None,
        persistence: Some(persistence),
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
    // The subtype of a *random* address (static / resolvable /
    // non-resolvable) is what separates a weeks-long identifier from a
    // 15-minute one; `bluetooth_persistence` reads it from the top two bits
    // rather than the locally-administered bit, which is wrong for BLE.
    let persistence = bluetooth_persistence(address_type.as_deref(), &address);
    let randomized = persistence.is_randomized();
    let is_public = matches!(address_type.as_deref(), Some("public"));

    let vendor = bluetooth_vendor(payload, &address, is_public);
    let device_type = bluetooth_device_type(payload);

    let mut attributes = serde_json::json!({
        "address": address,
        "randomized_mac": randomized,
        "mac_persistence": persistence.as_str(),
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
        persistence: Some(persistence),
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

/// A TPMS sensor report → `tpms_sensor`, identified by the sensor `id`. One
/// tire = one emitter. The payload's `id` is already normalized to a string
/// by `fluxfang_capture::rtl::parse`; no `id` → no stable identity → `None`.
/// Uses the default single-condition match rule (`id == <id>`), so a second
/// report from the same sensor auto-attaches to this emitter.
fn classify_tpms(payload: &Value) -> Option<Classification> {
    let id = non_empty_str(payload, "id")?;
    let model = payload.get("model").and_then(Value::as_str);
    Some(Classification {
        emitter_type: "tpms_sensor".to_string(),
        category: "tpms".to_string(),
        identity_field: "id".to_string(),
        identity_value: id.clone(),
        name: format!("TPMS_{id}"),
        attributes: serde_json::json!({
            "model": model,
            "sensor_id": id,
        }),
        match_criteria: None,
        // A TPMS sensor id isn't a MAC and is never randomized; `None`
        // keeps it out of the persistence filters and exempt from the
        // retention gate rather than forcing it into a class it isn't in.
        persistence: None,
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
        "bluetooth_device" => "Bluetooth Device",
        "tpms_sensor" => "TPMS Sensor",
        _ => "Unknown",
    }
}

/// Category key for an emitter type, for map/UI grouping (e.g. the overview
/// map's toggleable heatmap layers). Unrecognized keys map to `"other"`.
pub fn emitter_category(type_key: &str) -> &'static str {
    match type_key {
        "wifi_access_point" | "wifi_client" => "wifi",
        "bluetooth_device" => "bluetooth",
        "tpms_sensor" => "tpms",
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
        Some("tpms") => "tpms",
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
        "tpms" => vec![EmitterTypeInfo {
            key: "tpms_sensor",
            label: "TPMS Sensor",
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
        "wifi_access_point" | "wifi_client" | "bluetooth_device" | "tpms_sensor"
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

    // -- MacPersistence -----------------------------------------------------

    #[test]
    fn persistence_tokens_round_trip_and_reject_unknown() {
        for c in MacPersistence::ALL {
            assert_eq!(MacPersistence::parse(c.as_str()), Some(c));
        }
        assert_eq!(MacPersistence::parse("randomized"), None);
        assert_eq!(MacPersistence::parse(""), None);
    }

    #[test]
    fn persistence_ranks_are_strictly_descending_in_all_order() {
        let ranks: Vec<u8> = MacPersistence::ALL.iter().map(|c| c.rank()).collect();
        assert!(
            ranks.windows(2).all(|w| w[0] > w[1]),
            "ALL must be ordered most- to least-persistent, got {ranks:?}"
        );
    }

    #[test]
    fn badge_groups_per_network_and_session_as_longterm() {
        assert_eq!(MacPersistence::Stable.badge(), None);
        assert_eq!(
            MacPersistence::PerNetwork.badge(),
            Some("randomized-longterm")
        );
        assert_eq!(MacPersistence::Session.badge(), Some("randomized-longterm"));
        assert_eq!(MacPersistence::Ephemeral.badge(), Some("randomized"));
        assert_eq!(MacPersistence::Unlinkable.badge(), Some("randomized"));
    }

    #[test]
    fn is_randomized_is_true_for_every_class_but_stable() {
        assert!(!MacPersistence::Stable.is_randomized());
        for c in MacPersistence::ALL.into_iter().skip(1) {
            assert!(c.is_randomized(), "{c:?} should count as randomized");
        }
    }

    #[test]
    fn retained_at_keeps_own_class_and_everything_more_persistent() {
        // The spec'd example: selecting `session` stores session,
        // per_network and stable, and drops the two below it.
        let level = MacPersistence::Session;
        assert!(MacPersistence::Stable.retained_at(level));
        assert!(MacPersistence::PerNetwork.retained_at(level));
        assert!(MacPersistence::Session.retained_at(level));
        assert!(!MacPersistence::Ephemeral.retained_at(level));
        assert!(!MacPersistence::Unlinkable.retained_at(level));

        // The most permissive level keeps everything.
        for c in MacPersistence::ALL {
            assert!(c.retained_at(MacPersistence::Unlinkable));
        }
        // The strictest keeps only non-randomized addresses.
        for c in MacPersistence::ALL {
            assert_eq!(c.retained_at(MacPersistence::Stable), !c.is_randomized());
        }
    }

    // -- persistence_filter_classes -----------------------------------------

    #[test]
    fn filter_badge_tokens_expand_to_their_classes() {
        assert_eq!(
            persistence_filter_classes("randomized"),
            Some(vec![MacPersistence::Ephemeral, MacPersistence::Unlinkable])
        );
        assert_eq!(
            persistence_filter_classes("randomized-longterm"),
            Some(vec![MacPersistence::PerNetwork, MacPersistence::Session])
        );
    }

    #[test]
    fn filter_exact_class_tokens_select_only_themselves() {
        for c in MacPersistence::ALL {
            assert_eq!(persistence_filter_classes(c.as_str()), Some(vec![c]));
        }
    }

    #[test]
    fn filter_rejects_unknown_tokens() {
        for bad in ["", "random", "longterm", "randomised", "true"] {
            assert_eq!(persistence_filter_classes(bad), None, "token {bad:?}");
        }
    }

    #[test]
    fn every_advertised_filter_token_resolves() {
        for token in PERSISTENCE_FILTER_TOKENS {
            assert!(
                persistence_filter_classes(token).is_some(),
                "advertised token {token:?} must resolve"
            );
        }
        // ...and the two badges plus every class are exactly what's
        // advertised, so the dropdowns can't drift from the parser.
        assert_eq!(
            PERSISTENCE_FILTER_TOKENS.len(),
            2 + MacPersistence::ALL.len()
        );
    }

    // -- wifi_persistence / bluetooth_persistence ---------------------------

    #[test]
    fn wifi_probe_is_ephemeral_but_association_from_same_mac_is_per_network() {
        let mac = "3a:de:ad:be:ef:00";
        assert_eq!(
            wifi_persistence("probe_request", mac),
            MacPersistence::Ephemeral
        );
        assert_eq!(
            wifi_persistence("association_request", mac),
            MacPersistence::PerNetwork
        );
        assert_eq!(
            wifi_persistence("reassociation_request", mac),
            MacPersistence::PerNetwork
        );
    }

    #[test]
    fn wifi_non_randomized_mac_is_stable_in_every_frame_type() {
        for ft in ["probe_request", "association_request", "beacon", "junk"] {
            assert_eq!(
                wifi_persistence(ft, "00:11:22:33:44:55"),
                MacPersistence::Stable,
                "frame_type {ft}"
            );
        }
    }

    #[test]
    fn wifi_beacon_is_always_stable_even_with_a_locally_administered_bssid() {
        // A locally-administered BSSID is a virtual/multi-SSID interface,
        // not a privacy-randomized address -- it must not pick up a
        // "randomized" badge.
        assert_eq!(
            wifi_persistence("beacon", "02:11:22:33:44:55"),
            MacPersistence::Stable
        );
        assert_eq!(
            wifi_persistence("beacon", "00:11:22:33:44:55"),
            MacPersistence::Stable
        );
        assert_eq!(MacPersistence::Stable.badge(), None);
    }

    #[test]
    fn bluetooth_random_subtype_comes_from_top_two_bits_not_la_bit() {
        // 0xdb = 1101_1011: top bits 11 -> static random, weeks-long. Note
        // the LA bit (0x02) is *set* here, but it's the top bits that
        // decide. This is the real address from the operator's capture that
        // the old boolean lumped in with 15-minute RPAs.
        assert_eq!(
            bluetooth_persistence(Some("random"), "db:e5:df:32:9a:aa"),
            MacPersistence::Session
        );
        // 0xc0 = 1100_0000: top bits 11 -> static random, and the LA bit is
        // *clear*, so the 802.11 test would have called this a stable
        // vendor MAC.
        assert_eq!(
            bluetooth_persistence(Some("random"), "c0:11:22:33:44:55"),
            MacPersistence::Session
        );
        assert!(!is_randomized_mac("c0:11:22:33:44:55"));
        // 0x7a = 0111_1010: top bits 01 -> resolvable private, ~15 min.
        assert_eq!(
            bluetooth_persistence(Some("random"), "7a:11:22:33:44:55"),
            MacPersistence::Ephemeral
        );
        // 0x35 = 0011_0101: top bits 00 -> non-resolvable private.
        assert_eq!(
            bluetooth_persistence(Some("random"), "35:d7:ff:06:e7:a8"),
            MacPersistence::Unlinkable
        );
        // 0x80: top bits 10 is reserved by the spec -> claim the least.
        assert_eq!(
            bluetooth_persistence(Some("random"), "80:11:22:33:44:55"),
            MacPersistence::Ephemeral
        );
    }

    #[test]
    fn bluetooth_public_address_is_stable_even_with_static_random_bit_pattern() {
        // c8:9f:bb is a real OUI whose top two bits are 11. Because the
        // controller reported the address as public, the top-two-bits test
        // must not be applied.
        assert_eq!(
            bluetooth_persistence(Some("public"), "c8:9f:bb:fc:4f:ef"),
            MacPersistence::Stable
        );
    }

    #[test]
    fn bluetooth_absent_address_type_falls_back_to_la_bit() {
        assert_eq!(
            bluetooth_persistence(None, "7a:11:22:33:44:55"),
            MacPersistence::Ephemeral
        );
        assert_eq!(
            bluetooth_persistence(None, "00:11:22:33:44:55"),
            MacPersistence::Stable
        );
        // Malformed input must not panic.
        assert_eq!(
            bluetooth_persistence(Some("random"), "nonsense"),
            MacPersistence::Ephemeral
        );
    }

    #[test]
    fn classify_bluetooth_static_random_is_session_class() {
        let payload = serde_json::json!({
            "frame_type": "advertisement",
            "address": "db:e5:df:32:9a:aa",
            "address_type": "random"
        });
        let c = classify("bluetooth", &payload).unwrap();
        assert_eq!(c.persistence, Some(MacPersistence::Session));
        assert_eq!(c.attributes["mac_persistence"], "session");
        // Back-compatible flag still says "randomized".
        assert_eq!(c.attributes["randomized_mac"], serde_json::json!(true));
        assert_eq!(
            c.persistence.unwrap().badge(),
            Some("randomized-longterm"),
            "a static-random address must not be badged as short-lived"
        );
    }

    #[test]
    fn classify_wifi_association_is_per_network_class() {
        let payload = serde_json::json!({
            "frame_type": "association_request",
            "src_mac": "3a:de:ad:be:ef:00",
            "target_ssid": "HomeNet"
        });
        let c = classify("wifi", &payload).unwrap();
        assert_eq!(c.persistence, Some(MacPersistence::PerNetwork));
        assert_eq!(c.attributes["mac_persistence"], "per_network");
    }

    #[test]
    fn classify_tpms_has_no_persistence_class() {
        let payload = serde_json::json!({"id": "d8af50f2", "model": "Toyota"});
        let c = classify("tpms", &payload).unwrap();
        assert_eq!(c.persistence, None);
        assert!(c.attributes.get("mac_persistence").is_none());
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
            serde_json::json!({
                "bssid": "aa:bb:cc:dd:ee:ff",
                "ssid": "HomeNet",
                // An AP BSSID is always `stable`, even though 0xaa has the
                // locally-administered bit set -- that means virtual
                // interface, not privacy randomization.
                "mac_persistence": "stable",
            })
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

    #[test]
    fn wifi_beacon_classification_carries_security_attributes() {
        let payload = serde_json::json!({
            "bssid": "aa:bb:cc:dd:ee:ff",
            "ssid": "SecureNet",
            "frame_type": "beacon",
            "security": ["WPA2"],
            "auth": ["PSK"],
            "cipher": ["CCMP"],
            "transition_mode": false,
            "security_label": "WPA2-PSK (CCMP)"
        });
        let c = classify("wifi", &payload).unwrap();
        assert_eq!(c.emitter_type, "wifi_access_point");
        assert_eq!(c.attributes["security_label"], "WPA2-PSK (CCMP)");
        assert_eq!(c.attributes["security"], serde_json::json!(["WPA2"]));
        assert_eq!(c.attributes["auth"], serde_json::json!(["PSK"]));
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
            serde_json::json!({
                "src_mac": "3a:de:ad:be:ef:00",
                "randomized_mac": true,
                // A probe-request MAC is throwaway even though the same
                // device's association MAC would be per_network.
                "mac_persistence": "ephemeral",
            })
        );
        assert_eq!(c.persistence, Some(MacPersistence::Ephemeral));
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

    // -- classify: tpms -----------------------------------------------------

    #[test]
    fn classify_tpms_sensor_identity_name_and_attributes() {
        let payload = serde_json::json!({
            "id": "d8af50f2",
            "type": "TPMS",
            "model": "Toyota",
            "status": 128,
            "pressure_PSI": 31.0,
            "rssi": 1.0,
            "snr": 17.1
        });
        let c = classify("tpms", &payload).expect("tpms payload classifies");
        assert_eq!(c.emitter_type, "tpms_sensor");
        assert_eq!(c.category, "tpms");
        assert_eq!(c.identity_field, "id");
        assert_eq!(c.identity_value, "d8af50f2");
        assert_eq!(c.name, "TPMS_d8af50f2");
        assert_eq!(c.identity_key(), "tpms_sensor:d8af50f2");
        assert_eq!(
            c.attributes,
            serde_json::json!({"model": "Toyota", "sensor_id": "d8af50f2"})
        );
        // Default single-condition rule (id == <id>) — no override needed.
        assert!(c.match_criteria.is_none());
    }

    #[test]
    fn classify_tpms_missing_id_is_none() {
        let payload = serde_json::json!({"type": "TPMS", "model": "Toyota"});
        assert!(classify("tpms", &payload).is_none());
    }

    #[test]
    fn classify_tpms_missing_model_uses_null_attribute() {
        let payload = serde_json::json!({"id": "ab12", "type": "TPMS"});
        let c = classify("tpms", &payload).unwrap();
        assert_eq!(c.attributes["model"], serde_json::Value::Null);
        assert_eq!(c.name, "TPMS_ab12");
    }

    #[test]
    fn tpms_sensor_registered_in_type_registries() {
        assert_eq!(emitter_type_label("tpms_sensor"), "TPMS Sensor");
        assert_eq!(emitter_category("tpms_sensor"), "tpms");
        assert_eq!(catalog_kind_for(Some("tpms_sensor")), "tpms");
        assert!(is_known_emitter_type("tpms_sensor"));
        assert_eq!(
            emitter_types_for_kind("tpms"),
            vec![EmitterTypeInfo {
                key: "tpms_sensor",
                label: "TPMS Sensor"
            }]
        );
    }
}
