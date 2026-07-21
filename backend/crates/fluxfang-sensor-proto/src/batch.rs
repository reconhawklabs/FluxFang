use crate::error::ProtoError;
use crate::seal::{open, seal};
use crate::Key;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One captured emission in transit from a sensor. Mirrors the sensor's
/// local cache row (Phase 4) minus delivery bookkeeping. `observed_at` and
/// `lat`/`lon` are attached by the sensor at capture time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireEmission {
    pub id: Uuid,
    pub kind: String,
    pub signal_strength: Option<i32>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub observed_at: DateTime<Utc>,
    pub payload: serde_json::Value,
}

/// A batch of emissions a sensor forwards in one request. `sent_at_ms` is the
/// sensor's wall-clock at send time (unix epoch millis), used for replay
/// defense via [`within_replay_window`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SensorBatch {
    pub sensor_id: String,
    pub sent_at_ms: i64,
    pub emissions: Vec<WireEmission>,
}

/// Serialize a batch to JSON and AEAD-seal it under `key`.
pub fn seal_batch(key: &Key, batch: &SensorBatch) -> Result<Vec<u8>, ProtoError> {
    let json = serde_json::to_vec(batch).map_err(|e| ProtoError::Json(e.to_string()))?;
    Ok(seal(key, &json))
}

/// AEAD-open a sealed batch and deserialize it. `Malformed`/`Decrypt` come
/// from [`open`]; a valid-AEAD-but-not-a-batch payload yields `Json`.
pub fn open_batch(key: &Key, sealed: &[u8]) -> Result<SensorBatch, ProtoError> {
    let plaintext = open(key, sealed)?;
    serde_json::from_slice(&plaintext).map_err(|e| ProtoError::Json(e.to_string()))
}

/// True when `sent_at_ms` is within `max_skew_ms` of `now_ms` in either
/// direction (tolerates modest clock skew; rejects stale replays and
/// far-future timestamps).
pub fn within_replay_window(sent_at_ms: i64, now_ms: i64, max_skew_ms: i64) -> bool {
    (now_ms - sent_at_ms).abs() <= max_skew_ms
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generate_key;
    use serde_json::json;

    fn sample_batch() -> SensorBatch {
        SensorBatch {
            sensor_id: "frontgate".to_string(),
            sent_at_ms: 1_700_000_000_000,
            emissions: vec![
                WireEmission {
                    id: Uuid::nil(),
                    kind: "wifi".to_string(),
                    signal_strength: Some(-42),
                    lat: Some(1.5),
                    lon: Some(2.5),
                    observed_at: DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap(),
                    payload: json!({ "bssid": "aa:bb:cc:dd:ee:ff" }),
                },
                WireEmission {
                    id: Uuid::from_u128(1),
                    kind: "tpms".to_string(),
                    signal_strength: None,
                    lat: None,
                    lon: None,
                    observed_at: DateTime::<Utc>::from_timestamp(1_700_000_001, 0).unwrap(),
                    payload: json!({ "sensor_id": "0x1234" }),
                },
            ],
        }
    }

    #[test]
    fn seal_open_batch_roundtrips() {
        let key = generate_key();
        let batch = sample_batch();
        let sealed = seal_batch(&key, &batch).unwrap();
        assert_eq!(open_batch(&key, &sealed).unwrap(), batch);
    }

    #[test]
    fn open_batch_wrong_key_is_decrypt_error() {
        let sealed = seal_batch(&generate_key(), &sample_batch()).unwrap();
        assert_eq!(
            open_batch(&generate_key(), &sealed),
            Err(ProtoError::Decrypt)
        );
    }

    #[test]
    fn open_batch_on_valid_aead_but_non_json_is_json_error() {
        // Seal bytes that decrypt fine but aren't a SensorBatch JSON.
        let key = generate_key();
        let sealed = seal(&key, b"not json at all");
        assert!(matches!(
            open_batch(&key, &sealed),
            Err(ProtoError::Json(_))
        ));
    }

    #[test]
    fn open_batch_truncated_is_malformed() {
        let key = generate_key();
        assert_eq!(open_batch(&key, &[0u8; 4]), Err(ProtoError::Malformed));
    }

    #[test]
    fn replay_window_accepts_within_and_rejects_outside() {
        let now = 1_000_000i64;
        let skew = 30_000i64; // 30s
        assert!(within_replay_window(now, now, skew));
        assert!(within_replay_window(now - 29_000, now, skew));
        assert!(within_replay_window(now + 29_000, now, skew)); // clock ahead, still ok
        assert!(!within_replay_window(now - 31_000, now, skew)); // too old
        assert!(!within_replay_window(now + 31_000, now, skew)); // too far future
    }
}
