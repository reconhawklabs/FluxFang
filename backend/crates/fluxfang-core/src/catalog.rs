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
            FieldType::Enum(vec![
                "beacon".to_string(),
                "probe_request".to_string(),
                "association_request".to_string(),
                "reassociation_request".to_string(),
            ]),
        ),
        field("channel", "Channel", FieldType::Number),
        field("signal_strength", "Signal strength", FieldType::Number),
    ]
}

fn bluetooth_catalog() -> Vec<FieldDef> {
    vec![
        field("address", "Address", FieldType::Mac),
        field("name", "Local name", FieldType::Text),
        field("vendor", "Vendor", FieldType::Text),
        field("device_type", "Device type", FieldType::Text),
        field("service_uuids", "Service UUIDs", FieldType::Text),
        field("company_id", "Company ID", FieldType::Number),
        field("rssi", "RSSI", FieldType::Number),
        field("tx_power", "TX power", FieldType::Number),
        field(
            "address_type",
            "Address type",
            FieldType::Enum(vec!["public".to_string(), "random".to_string()]),
        ),
        field(
            "frame_type",
            "Frame type",
            FieldType::Enum(vec!["advertisement".to_string()]),
        ),
    ]
}

fn tpms_catalog() -> Vec<FieldDef> {
    vec![
        field("id", "Sensor ID", FieldType::Text),
        field("model", "Model", FieldType::Text),
        field("type", "Type", FieldType::Enum(vec!["TPMS".to_string()])),
        field("status", "Status", FieldType::Number),
        field("pressure_PSI", "Pressure (PSI)", FieldType::Number),
        field("temperature_C", "Temperature (C)", FieldType::Number),
        field("rssi", "RSSI", FieldType::Number),
        field("snr", "SNR", FieldType::Number),
    ]
}

/// Return the field catalog for a data source `kind` (e.g. `"wifi"`).
/// Unknown kinds return an empty catalog.
pub fn catalog_for(kind: &str) -> Vec<FieldDef> {
    match kind {
        "wifi" => wifi_catalog(),
        "bluetooth" => bluetooth_catalog(),
        "tpms" => tpms_catalog(),
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
        assert!(catalog_for("zigbee").is_empty());
    }

    #[test]
    fn bluetooth_catalog_exposes_address_and_name() {
        let c = catalog_for("bluetooth");
        assert!(c
            .iter()
            .any(|f| f.key == "address" && f.ty == FieldType::Mac));
        assert!(c.iter().any(|f| f.key == "name" && f.ty == FieldType::Text));
    }

    #[test]
    fn number_field_gets_ordering_ops_but_not_matches() {
        let c = catalog_for("wifi");
        let channel = c.iter().find(|f| f.key == "channel").unwrap();
        assert!(channel.ops.contains(&Op::Gte));
        assert!(channel.ops.contains(&Op::Lte));
        assert!(!channel.ops.contains(&Op::Matches));
    }

    #[test]
    fn wifi_catalog_frame_type_enum_includes_association_frames() {
        let c = catalog_for("wifi");
        let ft = c.iter().find(|f| f.key == "frame_type").unwrap();
        match &ft.ty {
            FieldType::Enum(values) => {
                assert!(values.contains(&"beacon".to_string()));
                assert!(values.contains(&"association_request".to_string()));
                assert!(values.contains(&"reassociation_request".to_string()));
            }
            other => panic!("frame_type should be an Enum, got {other:?}"),
        }
    }

    #[test]
    fn tpms_catalog_exposes_id_model_and_numeric_pressure() {
        let c = catalog_for("tpms");
        assert!(c.iter().any(|f| f.key == "id" && f.ty == FieldType::Text));
        assert!(c
            .iter()
            .any(|f| f.key == "model" && f.ty == FieldType::Text));
        let pressure = c.iter().find(|f| f.key == "pressure_PSI").unwrap();
        assert_eq!(pressure.ty, FieldType::Number);
        assert!(pressure.ops.contains(&Op::Gte)); // numeric → ordered
    }
}
