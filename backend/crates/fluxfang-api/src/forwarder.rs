//! `SensorForwarder`: ships a Sensor node's cached emissions to its Standalone
//! over the AEAD `/sensor/ingest` endpoint, self-registering (`/sensor/enroll`)
//! and retrying until the operator approves it.
//!
//! ## Live config
//!
//! The forwarder re-reads its target (`sensor_id`, key, host/port) from the
//! node config in the database (`app_config.settings`) on every loop cycle,
//! rather than capturing it once at startup. Saving a new key/host/port in the
//! Sensor's Settings (a `PATCH /api/config`) therefore takes effect within one
//! cycle with **no backend restart** — the DB is the single source of truth, so
//! there's no second copy to keep in sync. A missing/invalid config just pauses
//! the loop (it never dies), and changing the key resets approval so the sensor
//! re-enrolls under its new fingerprint (a key rotation must be re-approved on
//! the Standalone — inherent to the out-of-band-key model, not the reload).

use std::time::Duration;

use fluxfang_db::{AppConfigRepo, CachedEmissionRepo, NodeRole};
use fluxfang_sensor_proto::{seal_batch, Key, SensorBatch, WireEmission};
use sqlx::PgPool;
use uuid::Uuid;

const FORWARD_BATCH_LIMIT: i64 = 200;
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const FORWARD_IDLE: Duration = Duration::from_secs(2);
const FORWARD_BACKOFF: Duration = Duration::from_secs(30);
const PRUNE_INTERVAL: Duration = Duration::from_secs(300);
/// How often an approved-but-idle sensor pings the Standalone so it keeps
/// showing "online" even with nothing to forward. Comfortably under the
/// Standalone's 60s online threshold.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
/// Fallback cache TTL when the sensor config is absent (7 days).
const DEFAULT_CACHE_TTL_SECS: i64 = 604_800;

#[derive(Debug)]
pub enum ForwardOutcome {
    Delivered(usize),
    Nothing,
    NotApproved,
    Error(String),
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

/// The current forwarding target, derived fresh from the node config each
/// cycle: who we are, the key we seal with, and where the Standalone is.
#[derive(Clone)]
pub struct ForwarderTarget {
    pub sensor_id: String,
    pub key: Key,
    pub base_url: String, // http://host:port
}

impl ForwarderTarget {
    /// A stable identity for change-detection: any change to the sensor id,
    /// destination, or key (via its fingerprint) yields a different string, so
    /// the loop knows to reset approval and re-enroll.
    fn ident(&self) -> String {
        let fp = fluxfang_sensor_proto::fingerprint(&self.sensor_id, &self.key);
        format!("{}|{}|{}", self.sensor_id, self.base_url, fp)
    }
}

/// Load the current forwarding target from the DB node config. Returns `None`
/// (→ the loop pauses) when this isn't a sensor node, there's no sensor config,
/// or the configured key isn't valid base64/32 bytes.
pub async fn load_target(pool: &PgPool) -> Option<ForwarderTarget> {
    let node = AppConfigRepo::node_config(pool).await.ok().flatten()?;
    if node.role != NodeRole::Sensor {
        return None;
    }
    let sensor = node.sensor?;
    let key = fluxfang_sensor_proto::decode_key(&sensor.key).ok()?;
    Some(ForwarderTarget {
        sensor_id: node.node_sensor_id,
        key,
        base_url: format!("http://{}:{}", sensor.host, sensor.port),
    })
}

pub struct SensorForwarder {
    pool: PgPool,
    client: reqwest::Client,
}

impl SensorForwarder {
    pub fn new(pool: PgPool) -> Self {
        let client = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { pool, client }
    }

    /// One forward cycle against `target`. On 403 (not approved) it reports
    /// `NotApproved` so the loop drops back to enrolling.
    pub async fn forward_once(&self, target: &ForwarderTarget) -> ForwardOutcome {
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
            sensor_id: target.sensor_id.clone(),
            sent_at_ms: chrono::Utc::now().timestamp_millis(),
            emissions,
        };
        let sealed = match seal_batch(&target.key, &batch) {
            Ok(s) => s,
            Err(e) => return ForwardOutcome::Error(format!("seal: {e}")),
        };

        let resp = self
            .client
            .post(format!("{}/sensor/ingest", target.base_url))
            .header("X-Sensor-Id", &target.sensor_id)
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
    pub async fn enroll(&self, target: &ForwarderTarget) -> EnrollResult {
        let fingerprint = fluxfang_sensor_proto::fingerprint(&target.sensor_id, &target.key);
        let resp = self
            .client
            .post(format!("{}/sensor/enroll", target.base_url))
            .json(&serde_json::json!({ "sensor_id": target.sensor_id, "fingerprint": fingerprint }))
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

#[derive(serde::Deserialize)]
struct EnrollResponse {
    status: String,
}

#[derive(serde::Deserialize)]
struct AcceptResponse {
    accepted: Vec<Uuid>,
}

/// Background loop. Always spawn this for a Sensor node — it reloads its target
/// from the DB each cycle, so it pauses cleanly until a valid config exists and
/// picks up a saved key/host/port change live (no restart).
///
/// While NOT approved, it proactively self-enrolls every 30s — independent of
/// whether anything has been captured yet. This is what lets a freshly
/// provisioned sensor (no capture hardware, empty cache) appear in the
/// Standalone's pending list the moment its enrollment window opens. Once
/// approved, it forwards continuously, dropping back to enrolling if approval
/// is revoked (ingest 403) or the config changes under it.
pub fn spawn_forwarder(forwarder: SensorForwarder) {
    tokio::spawn(async move {
        let mut approved = false;
        let mut last_ident: Option<String> = None;
        let mut paused_logged = false;
        // Last time we successfully reached the Standalone (enroll or forward);
        // drives the idle heartbeat so we stay "online" with nothing to send.
        let mut last_contact = std::time::Instant::now();
        loop {
            let Some(target) = load_target(&forwarder.pool).await else {
                // No valid sensor config yet — pause (don't die). Log once per
                // paused stretch so a bad/absent key is visible without spam.
                if !paused_logged {
                    eprintln!(
                        "SensorForwarder: no valid sensor config (paused — set a key in Settings)"
                    );
                    paused_logged = true;
                }
                approved = false;
                last_ident = None;
                tokio::time::sleep(FORWARD_BACKOFF).await;
                continue;
            };
            paused_logged = false;

            // A changed target (new key/host/sensor_id) means we must re-enroll
            // — our old approval no longer describes this key.
            let ident = target.ident();
            if last_ident.as_ref() != Some(&ident) {
                approved = false;
                last_ident = Some(ident);
            }

            let delay = if !approved {
                match forwarder.enroll(&target).await {
                    EnrollResult::Approved => {
                        approved = true;
                        last_contact = std::time::Instant::now();
                        FORWARD_IDLE
                    }
                    EnrollResult::Pending => FORWARD_BACKOFF,
                }
            } else {
                match forwarder.forward_once(&target).await {
                    ForwardOutcome::Delivered(_) => {
                        last_contact = std::time::Instant::now();
                        FORWARD_IDLE
                    }
                    ForwardOutcome::Nothing => {
                        // Nothing to forward. Heartbeat if it's been a while so
                        // the Standalone keeps us "online"; an approved re-enroll
                        // just bumps last_seen (it bypasses the window).
                        //
                        // Best-effort: a failed heartbeat (a transient blip, or
                        // a Standalone not yet running the approved-bypass code)
                        // must NOT drop approval or stop forwarding. Only an
                        // ingest 403 below authoritatively means we lost it.
                        if last_contact.elapsed() >= HEARTBEAT_INTERVAL {
                            if let EnrollResult::Approved = forwarder.enroll(&target).await {
                                last_contact = std::time::Instant::now();
                            }
                        }
                        FORWARD_IDLE
                    }
                    ForwardOutcome::NotApproved => {
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

/// Background loop: every 5 min, delete cached rows older than the TTL. The TTL
/// is re-read from the node config each tick, so a changed `cache_ttl_secs`
/// takes effect without a restart; falls back to 7 days when unset.
pub fn spawn_pruner(pool: PgPool) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(PRUNE_INTERVAL);
        loop {
            ticker.tick().await;
            let ttl = AppConfigRepo::node_config(&pool)
                .await
                .ok()
                .flatten()
                .and_then(|n| n.sensor)
                .map(|s| s.cache_ttl_secs)
                .unwrap_or(DEFAULT_CACHE_TTL_SECS);
            let cutoff = chrono::Utc::now() - chrono::Duration::seconds(ttl.max(0));
            match CachedEmissionRepo::prune_older_than(&pool, cutoff).await {
                Ok(n) if n > 0 => eprintln!("SensorForwarder: pruned {n} cached emission(s)"),
                Ok(_) => {}
                Err(e) => eprintln!("SensorForwarder: prune failed: {e}"),
            }
        }
    });
}
