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
const FORWARD_IDLE: Duration = Duration::from_secs(2);
const FORWARD_BACKOFF: Duration = Duration::from_secs(30);
const PRUNE_INTERVAL: Duration = Duration::from_secs(300);

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
            // Not (or no longer) approved. The spawn loop owns re-enrollment,
            // so just report it and let the loop drop back to enrolling.
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

    /// Self-register with the Standalone and learn our approval status.
    ///
    /// Transmits only `{sensor_id, fingerprint}` — the key NEVER leaves this
    /// node. The operator types the key into the Standalone's approval dialog
    /// out-of-band; the fingerprint (a one-way hash) is what lets them verify
    /// they entered the right key. Idempotent while pending; best-effort — any
    /// transport/HTTP error maps to `Pending` so the loop simply retries.
    pub async fn enroll(&self) -> EnrollResult {
        let fingerprint = fluxfang_sensor_proto::fingerprint(&self.sensor_id, &self.key);
        let resp = self
            .client
            .post(format!("{}/sensor/enroll", self.base_url))
            .json(&serde_json::json!({ "sensor_id": self.sensor_id, "fingerprint": fingerprint }))
            .send()
            .await;
        match resp {
            Ok(r) if r.status().is_success() => match r.json::<EnrollResponse>().await {
                Ok(body) if body.status == "approved" => EnrollResult::Approved,
                _ => EnrollResult::Pending,
            },
            // Non-2xx (window closed / conflict / etc.) or a transport error:
            // we're not approved yet — keep retrying.
            _ => EnrollResult::Pending,
        }
    }
}

/// Outcome of a self-enrollment attempt.
#[derive(Debug, PartialEq, Eq)]
pub enum EnrollResult {
    /// The Standalone confirms this sensor is approved — ready to forward.
    Approved,
    /// Registered/updated but still awaiting approval (or the window is closed,
    /// or a transient error). Keep retrying.
    Pending,
}

#[derive(serde::Deserialize)]
struct EnrollResponse {
    status: String,
}

#[derive(serde::Deserialize)]
struct AcceptResponse {
    accepted: Vec<Uuid>,
}

/// Background loop.
///
/// While NOT approved, it proactively self-enrolls every 30s — independent of
/// whether anything has been captured yet. This is what lets a freshly
/// provisioned sensor (no capture hardware, empty cache) appear in the
/// Standalone's pending list the moment its enrollment window opens; without
/// it, a sensor with nothing to forward would never contact the Standalone and
/// could never be approved. Once approved, it forwards continuously, dropping
/// back to enrolling if approval is ever revoked (ingest returns 403).
pub fn spawn_forwarder(forwarder: SensorForwarder) {
    tokio::spawn(async move {
        let mut approved = false;
        loop {
            let delay = if !approved {
                match forwarder.enroll().await {
                    EnrollResult::Approved => {
                        approved = true;
                        // Loop straight into forwarding whatever is queued.
                        FORWARD_IDLE
                    }
                    // Still pending / window not open yet / transient error.
                    EnrollResult::Pending => FORWARD_BACKOFF,
                }
            } else {
                match forwarder.forward_once().await {
                    ForwardOutcome::Delivered(_) | ForwardOutcome::Nothing => FORWARD_IDLE,
                    ForwardOutcome::NotApproved => {
                        // Approval was revoked — go back to enrolling.
                        approved = false;
                        FORWARD_BACKOFF
                    }
                    ForwardOutcome::Error(e) => {
                        eprintln!("SensorForwarder: {e}");
                        FORWARD_BACKOFF
                    }
                }
            };
            tokio::time::sleep(delay).await;
        }
    });
}

/// Background loop: every 5 min, delete cached rows older than the TTL.
pub fn spawn_pruner(pool: PgPool, cache_ttl_secs: i64) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(PRUNE_INTERVAL);
        loop {
            ticker.tick().await;
            let cutoff = chrono::Utc::now() - chrono::Duration::seconds(cache_ttl_secs.max(0));
            match CachedEmissionRepo::prune_older_than(&pool, cutoff).await {
                Ok(n) if n > 0 => eprintln!("SensorForwarder: pruned {n} cached emission(s)"),
                Ok(_) => {}
                Err(e) => eprintln!("SensorForwarder: prune failed: {e}"),
            }
        }
    });
}
