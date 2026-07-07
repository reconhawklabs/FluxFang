//! Compiled-in Bluetooth vendor / device-type lookup (see the design doc's
//! "Vendor / device-type lookup" section). Data is embedded with
//! `include_str!` from the checked-in TSVs under `fluxfang-core/data/` and
//! parsed once — no runtime file read, no network. The seed TSVs cover a
//! curated subset (incl. these tests' fixtures); expand them to the full
//! IEEE OUI / Bluetooth SIG sets with `scripts/fetch-bt-vendor-data.sh`.

use std::collections::HashMap;
use std::sync::OnceLock;

/// Parse `#`-commented, tab-separated `key<TAB>value` lines. `value` is a
/// `&'static str` slice of the embedded input, so no allocation of the value.
fn parse_map<K, F>(raw: &'static str, parse_key: F) -> HashMap<K, &'static str>
where
    K: std::hash::Hash + Eq,
    F: Fn(&str) -> Option<K>,
{
    raw.lines()
        .filter(|line| !line.trim_start().starts_with('#') && !line.trim().is_empty())
        .filter_map(|line| {
            let (key, value) = line.split_once('\t')?;
            let value = value.trim();
            if value.is_empty() {
                return None;
            }
            Some((parse_key(key.trim())?, value))
        })
        .collect()
}

/// Bluetooth SIG company id → company name.
pub fn company_name(id: u16) -> Option<&'static str> {
    static MAP: OnceLock<HashMap<u16, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| {
        parse_map(include_str!("../../data/bt_company_ids.tsv"), |k| {
            k.parse::<u16>().ok()
        })
    })
    .get(&id)
    .copied()
}

/// IEEE OUI (first three octets of a public MAC) → vendor. Case-insensitive;
/// `address` may be a full `aa:bb:cc:dd:ee:ff` — only the first three octets
/// are used. Returns `None` for malformed input or an unlisted OUI.
pub fn oui_vendor(address: &str) -> Option<&'static str> {
    static MAP: OnceLock<HashMap<String, &'static str>> = OnceLock::new();
    let map = MAP.get_or_init(|| {
        parse_map(include_str!("../../data/ieee_oui.tsv"), |k| {
            Some(k.to_ascii_uppercase())
        })
    });
    let octets: Vec<&str> = address.split(':').take(3).collect();
    if octets.len() != 3 || octets.iter().any(|o| o.len() != 2) {
        return None;
    }
    let oui = octets.join(":").to_ascii_uppercase();
    map.get(&oui).copied()
}

/// BLE Appearance value → coarse device-type category (category = value >> 6).
pub fn appearance_device_type(value: u16) -> Option<&'static str> {
    match value >> 6 {
        1 => Some("Phone"),
        2 => Some("Computer"),
        3 => Some("Watch"),
        4 => Some("Clock"),
        5 => Some("Display"),
        6 => Some("Remote Control"),
        7 => Some("Eye-glasses"),
        8 => Some("Tag"),
        10 => Some("Media Player"),
        13 => Some("Heart Rate Sensor"),
        _ => None,
    }
}

/// Classic Class-of-Device → major-device-class category
/// (major class = (cod >> 8) & 0x1F).
pub fn cod_device_type(cod: u32) -> Option<&'static str> {
    match (cod >> 8) & 0x1F {
        1 => Some("Computer"),
        2 => Some("Phone"),
        3 => Some("Network"),
        4 => Some("Audio/Video"),
        5 => Some("Peripheral"),
        6 => Some("Imaging"),
        7 => Some("Wearable"),
        8 => Some("Toy"),
        9 => Some("Health"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn company_name_known_and_unknown() {
        assert_eq!(company_name(76), Some("Apple, Inc."));
        assert_eq!(company_name(6), Some("Microsoft"));
        assert_eq!(company_name(0xFFFE), None);
    }

    #[test]
    fn oui_vendor_normalizes_case_and_takes_first_three_octets() {
        assert_eq!(oui_vendor("3C:15:C2:AA:BB:CC"), Some("Apple, Inc."));
        assert_eq!(oui_vendor("3c:15:c2:aa:bb:cc"), Some("Apple, Inc."));
        assert_eq!(oui_vendor("00:1A:11:00:00:00"), Some("Google, Inc."));
        assert_eq!(oui_vendor("de:ad:be:ef:00:00"), None);
        assert_eq!(oui_vendor("garbage"), None);
    }

    #[test]
    fn appearance_device_type_uses_category_bits() {
        // Appearance category = value >> 6. 64 >> 6 == 1 == Phone.
        assert_eq!(appearance_device_type(64), Some("Phone"));
        // 192 >> 6 == 3 == Watch.
        assert_eq!(appearance_device_type(192), Some("Watch"));
        assert_eq!(appearance_device_type(0), None); // Unknown category
    }

    #[test]
    fn cod_device_type_uses_major_device_class() {
        // Major device class = (cod >> 8) & 0x1F. 0x5A020C -> 2 == Phone.
        assert_eq!(cod_device_type(0x5A020C), Some("Phone"));
        // Major class 4 == Audio/Video.
        assert_eq!(cod_device_type(0x000400), Some("Audio/Video"));
        assert_eq!(cod_device_type(0x000000), None); // Miscellaneous
    }
}
