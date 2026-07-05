//! Per-data-source-kind field catalogs: which fields a [`crate::rule::Rule`]
//! may reference for a given data source `kind`, what type each field is,
//! and which [`Op`]s are valid for it.

use crate::rule::Op;
use serde::{Deserialize, Serialize};

/// The value domain of a catalog field, used to decide which [`Op`]s make
/// sense for it (e.g. a MAC address isn't ordered, so no `gte`/`lte`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FieldType {
    Text,
    Mac,
    Number,
    Enum(Vec<String>),
}

/// One field exposed by a data source kind's catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldDef {
    pub key: String,
    pub label: String,
    pub ty: FieldType,
    pub ops: Vec<Op>,
}

fn field(key: &str, label: &str, ty: FieldType) -> FieldDef {
    let ops = match &ty {
        FieldType::Text | FieldType::Mac => vec![Op::Eq, Op::Neq, Op::Matches, Op::In],
        FieldType::Number => vec![Op::Eq, Op::Neq, Op::Gte, Op::Lte, Op::In],
        FieldType::Enum(_) => vec![Op::Eq, Op::Neq, Op::In],
    };
    FieldDef {
        key: key.to_string(),
        label: label.to_string(),
        ty,
        ops,
    }
}

fn wifi_catalog() -> Vec<FieldDef> {
    vec![
        field("bssid", "BSSID", FieldType::Mac),
        field("mac", "MAC address", FieldType::Mac),
        field("src_mac", "Source MAC (client)", FieldType::Mac),
        field("ssid", "SSID", FieldType::Text),
        field(
            "frame_type",
            "Frame type",
            FieldType::Enum(vec!["beacon".to_string(), "probe_request".to_string()]),
        ),
        field("channel", "Channel", FieldType::Number),
        field("signal_strength", "Signal strength", FieldType::Number),
    ]
}

/// Return the field catalog for a data source `kind` (e.g. `"wifi"`).
/// Unknown kinds return an empty catalog.
pub fn catalog_for(kind: &str) -> Vec<FieldDef> {
    match kind {
        "wifi" => wifi_catalog(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wifi_catalog_exposes_bssid_with_eq_and_matches() {
        let c = catalog_for("wifi");
        let bssid = c.iter().find(|f| f.key == "bssid").unwrap();
        assert!(bssid.ops.contains(&Op::Eq));
        assert!(bssid.ops.contains(&Op::Matches));
        assert!(!bssid.ops.contains(&Op::Gte)); // mac isn't ordered
    }

    #[test]
    fn wifi_catalog_exposes_src_mac_as_mac_field() {
        let c = catalog_for("wifi");
        let src_mac = c.iter().find(|f| f.key == "src_mac").unwrap();
        assert_eq!(src_mac.ty, FieldType::Mac);
        assert!(src_mac.ops.contains(&Op::Eq));
    }

    #[test]
    fn unknown_kind_returns_empty_catalog() {
        assert!(catalog_for("bluetooth").is_empty());
    }

    #[test]
    fn number_field_gets_ordering_ops_but_not_matches() {
        let c = catalog_for("wifi");
        let channel = c.iter().find(|f| f.key == "channel").unwrap();
        assert!(channel.ops.contains(&Op::Gte));
        assert!(channel.ops.contains(&Op::Lte));
        assert!(!channel.ops.contains(&Op::Matches));
    }
}
