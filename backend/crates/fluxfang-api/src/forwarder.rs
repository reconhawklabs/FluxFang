//! `SensorForwarder`: ships a Sensor node's cached emissions to its Standalone
//! over the AEAD `/sensor/ingest` endpoint, self-registering (`/sensor/enroll`)
//! and retrying until the operator approves it.

use std::time::Duration;

use fluxfang_db::node_config::SensorConfig;
use fluxfang_db::CachedEmissionRepo;
use fluxfang_sensor_proto::{seal_batch, Key, SensorBatch, WireEmission};
use sqlx::PgPool;
use uuid::Uuid;

const FORWARD_BATCH_LIMIT: i64 = 200;
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug)]
pub enum ForwardOutcome {
    Delivered(usize),
    Nothing,
    NotApproved,
    Error(String),
}

pub struct SensorForwarder {
    pool: PgPool,
    base_url: String, // http://host:port
    key: Key,
    sensor_id: String,
    client: reqwest::Client,
}

impl SensorForwarder {
    pub fn new(pool: PgPool, sensor: &SensorConfig, sensor_id: String) -> anyhow::Result<Self> {
        let key = fluxfang_sensor_proto::decode_key(&sensor.key)
            .map_err(|_| anyhow::anyhow!("sensor key is not valid base64/32 bytes"))?;
        let client = reqwest::Client::builder().timeout(HTTP_TIMEOUT).build()?;
        Ok(Self {
            pool,
            base_url: format!("http://{}:{}", sensor.host, sensor.port),
            key,
            sensor_id,
            client,
        })
    }

    /// One forward cycle. On 403 (not approved) it (re-)enrolls and reports
    /// `NotApproved` so the caller backs off.
    pub async fn forward_once(&self) -> ForwardOutcome {
        let cached =
            match CachedEmissionRepo::list_undelivered(&self.pool, FORWARD_BATCH_LIMIT).await {
                Ok(c) => c,
                Err(e) => return ForwardOutcome::Error(format!("db: {e}")),
            };
        if cached.is_empty() {
            return ForwardOutcome::Nothing;
        }

        let emissions: Vec<WireEmission> = cached
            .iter()
            .map(|c| WireEmission {
                id: c.id,
                kind: c.kind.clone(),
                signal_strength: c.signal_strength,
                lat: c.lat,
                lon: c.lon,
                observed_at: c.observed_at,
                payload: c.payload.clone(),
            })
            .collect();
        let batch = SensorBatch {
            sensor_id: self.sensor_id.clone(),
            sent_at_ms: chrono::Utc::now().timestamp_millis(),
            emissions,
        };
        let sealed = match seal_batch(&self.key, &batch) {
            Ok(s) => s,
            Err(e) => return ForwardOutcome::Error(format!("seal: {e}")),
        };

        let resp = self
            .client
            .post(format!("{}/sensor/ingest", self.base_url))
            .header("X-Sensor-Id", &self.sensor_id)
            .body(sealed)
            .send()
            .await;
        let resp = match resp {
            Ok(r) => r,
            Err(e) => return ForwardOutcome::Error(format!("post: {e}")),
        };

        if resp.status() == reqwest::StatusCode::FORBIDDEN {
            self.enroll().await;
            return ForwardOutcome::NotApproved;
        }
        if !resp.status().is_success() {
            return ForwardOutcome::Error(format!("ingest status {}", resp.status()));
        }
        let accepted: AcceptResponse = match resp.json().await {
            Ok(a) => a,
            Err(e) => return ForwardOutcome::Error(format!("bad ack: {e}")),
        };
        match CachedEmissionRepo::mark_delivered(&self.pool, &accepted.accepted).await {
            Ok(_) => ForwardOutcome::Delivered(accepted.accepted.len()),
            Err(e) => ForwardOutcome::Error(format!("mark: {e}")),
        }
    }

    /// Self-register with the Standalone (idempotent while pending). Best-effort.
    async fn enroll(&self) {
        let key_b64 = fluxfang_sensor_proto::encode_key(&self.key);
        let _ = self
            .client
            .post(format!("{}/sensor/enroll", self.base_url))
            .json(&serde_json::json!({ "sensor_id": self.sensor_id, "key": key_b64 }))
            .send()
            .await;
    }
}

#[derive(serde::Deserialize)]
struct AcceptResponse {
    accepted: Vec<Uuid>,
}
