//! Per-node role configuration, persisted in `app_config.settings` (jsonb).
//! Pure serde types — no DB or HTTP here.

use serde::{Deserialize, Serialize};

/// Which role this FluxFang instance runs as. Chosen at first-run setup and
/// stored under `app_config.settings.role`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeRole {
    Standalone,
    Sensor,
}

/// Connection + caching settings a Sensor node needs to reach its Standalone.
/// Present only when `role == Sensor`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SensorConfig {
    pub host: String,
    pub port: u16,
    /// base64-encoded symmetric key (opaque at this layer).
    pub key: String,
    pub cache_ttl_secs: i64,
}

/// The node-role block stored under `app_config.settings`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeConfig {
    pub role: NodeRole,
    pub node_sensor_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensor: Option<SensorConfig>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn standalone_roundtrips_without_sensor_block() {
        let cfg = NodeConfig {
            role: NodeRole::Standalone,
            node_sensor_id: "local".to_string(),
            sensor: None,
        };
        let v = serde_json::to_value(&cfg).unwrap();
        assert_eq!(v, json!({ "role": "standalone", "node_sensor_id": "local" }));
        let back: NodeConfig = serde_json::from_value(v).unwrap();
        assert_eq!(back, cfg);
    }

    #[test]
    fn sensor_roundtrips_with_connection_block() {
        let cfg = NodeConfig {
            role: NodeRole::Sensor,
            node_sensor_id: "frontgate".to_string(),
            sensor: Some(SensorConfig {
                host: "base.example".to_string(),
                port: 9000,
                key: "a2V5".to_string(),
                cache_ttl_secs: 604_800,
            }),
        };
        let v = serde_json::to_value(&cfg).unwrap();
        assert_eq!(v["role"], "sensor");
        assert_eq!(v["sensor"]["port"], 9000);
        let back: NodeConfig = serde_json::from_value(v).unwrap();
        assert_eq!(back, cfg);
    }
}
