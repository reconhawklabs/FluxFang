//! Pure mapping from a BlueZ `org.bluez.Device1` property snapshot to a
//! `RawObservation` — the testable half of the bluetooth capturer. The
//! D-Bus I/O in `scan.rs` builds a `DeviceProps` and calls
//! `device_props_to_observation`; this file has no I/O and is fully
//! unit-tested (unlike the D-Bus loop, which needs a live bus — see
//! `scan.rs`, same convention as the wifi capturers).

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::RawObservation;

/// The subset of `org.bluez.Device1` properties the capturer reads.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeviceProps {
    pub address: Option<String>,
    pub address_type: Option<String>,
    pub name: Option<String>,
    pub rssi: Option<i32>,
    pub tx_power: Option<i32>,
    pub uuids: Vec<String>,
    /// company id → raw manufacturer bytes (BlueZ `ManufacturerData`).
    pub manufacturer_data: BTreeMap<u16, Vec<u8>>,
    pub appearance: Option<u16>,
    pub class_of_device: Option<u32>,
}

/// Build a `RawObservation` from a `DeviceProps`, or `None` if there is no
/// address (no stable identity). Absent optional fields are omitted from the
/// payload (never `null`), matching the wifi convention. The advertised
/// address is lowercased. When present, the first (lowest-id) manufacturer
/// data entry sets `company_id` + hex `manufacturer_data`.
pub fn device_props_to_observation(
    props: &DeviceProps,
    observed_at: DateTime<Utc>,
) -> Option<RawObservation> {
    let address = props.address.as_ref()?.to_ascii_lowercase();

    let mut payload = json!({
        "frame_type": "advertisement",
        "address": address,
    });
    let obj = payload.as_object_mut().expect("json object");

    if let Some(at) = &props.address_type {
        obj.insert("address_type".into(), Value::String(at.clone()));
    }
    if let Some(name) = &props.name {
        obj.insert("name".into(), Value::String(name.clone()));
    }
    if let Some(rssi) = props.rssi {
        obj.insert("rssi".into(), Value::from(rssi));
    }
    if let Some(tx) = props.tx_power {
        obj.insert("tx_power".into(), Value::from(tx));
    }
    if !props.uuids.is_empty() {
        obj.insert("service_uuids".into(), json!(props.uuids));
    }
    if let Some((company_id, bytes)) = props.manufacturer_data.iter().next() {
        obj.insert("company_id".into(), Value::from(*company_id));
        obj.insert("manufacturer_data".into(), Value::String(to_hex(bytes)));
    }
    if let Some(appearance) = props.appearance {
        obj.insert("appearance".into(), Value::from(appearance));
    }
    if let Some(cod) = props.class_of_device {
        obj.insert("class_of_device".into(), Value::from(cod));
    }
    obj.insert(
        "transport".into(),
        Value::String(
            if props.class_of_device.is_some() {
                "classic"
            } else {
                "le"
            }
            .to_string(),
        ),
    );

    Some(RawObservation {
        kind: "bluetooth".to_string(),
        observed_at,
        signal_strength: props.rssi,
        payload,
    })
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    #[test]
    fn maps_a_named_le_device_with_manufacturer_data() {
        let mut md = BTreeMap::new();
        md.insert(76u16, vec![0x02, 0x15, 0xAA]);
        let props = DeviceProps {
            address: Some("7A:11:22:33:44:55".to_string()),
            address_type: Some("random".to_string()),
            name: Some("Johns iPhone".to_string()),
            rssi: Some(-55),
            tx_power: Some(-12),
            uuids: vec!["0000180f-0000-1000-8000-00805f9b34fb".to_string()],
            manufacturer_data: md,
            appearance: Some(64),
            class_of_device: None,
        };
        let obs = device_props_to_observation(&props, ts()).unwrap();
        assert_eq!(obs.kind, "bluetooth");
        assert_eq!(obs.signal_strength, Some(-55));
        // Address lowercased for consistency with wifi MAC handling.
        assert_eq!(obs.payload["address"], "7a:11:22:33:44:55");
        assert_eq!(obs.payload["address_type"], "random");
        assert_eq!(obs.payload["name"], "Johns iPhone");
        assert_eq!(obs.payload["frame_type"], "advertisement");
        assert_eq!(obs.payload["rssi"], -55);
        assert_eq!(obs.payload["tx_power"], -12);
        assert_eq!(obs.payload["company_id"], 76);
        assert_eq!(obs.payload["manufacturer_data"], "0215aa");
        assert_eq!(obs.payload["appearance"], 64);
        assert_eq!(obs.payload["transport"], "le");
        assert_eq!(
            obs.payload["service_uuids"][0],
            "0000180f-0000-1000-8000-00805f9b34fb"
        );
    }

    #[test]
    fn classic_device_with_class_of_device_is_classic_transport() {
        let props = DeviceProps {
            address: Some("00:11:22:33:44:55".to_string()),
            class_of_device: Some(0x5A020C),
            ..Default::default()
        };
        let obs = device_props_to_observation(&props, ts()).unwrap();
        assert_eq!(obs.payload["transport"], "classic");
        assert_eq!(obs.payload["class_of_device"], 0x5A020C);
        // Absent optional fields are omitted, not null.
        assert!(obs.payload.get("name").is_none());
        assert!(obs.payload.get("rssi").is_none());
    }

    #[test]
    fn no_address_yields_none() {
        let props = DeviceProps {
            address: None,
            ..Default::default()
        };
        assert!(device_props_to_observation(&props, ts()).is_none());
    }
}
