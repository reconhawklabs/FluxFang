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

/// Emissions per forwarded batch. Larger batches amortize the HTTP round
/// trip and the AEAD seal over more rows, which is what lets a backlog drain
/// faster than it accumulates; the Standalone bounds its own per-batch work
/// with `sensor_listener::INGEST_BUDGET`, so raising this cannot push a batch
/// past the timeout below.
const FORWARD_BATCH_LIMIT: i64 = 500;

/// Client-side ceiling on one `/sensor/ingest` call.
///
/// This used to be 15s, which was the second half of the stall: a Standalone
/// that needed longer than that for a batch had the connection dropped
/// underneath it, so the handler was cancelled before it could ACK, and the
/// Sensor retried the identical rows on the next cycle -- forever, while its
/// cache kept growing. The Standalone now returns a partial ACK rather than
/// exceed its own budget, so this only has to be comfortably above that
/// budget; it is not a throughput knob.
const HTTP_TIMEOUT: Duration = Duration::from_secs(90);

/// Pause between cycles when there is nothing waiting to forward.
const FORWARD_IDLE: Duration = Duration::from_secs(2);

/// Base pause after a failed cycle. Jittered by [`backoff_with_jitter`]; see
/// there for why the jitter matters with more than one sensor.
const FORWARD_BACKOFF: Duration = Duration::from_secs(30);

/// How often to re-read the node config while the loop is paused for lack of
/// one.
///
/// Deliberately much shorter than [`FORWARD_BACKOFF`]. "No sensor config yet"
/// is not a failure to back off from -- it is the state a node sits in while
/// an operator is actively running first-run setup or typing a key into
/// Settings, watching for it to appear on the Standalone. Re-reading every
/// 30s there makes a working system feel broken. The poll is a single
/// indexed row read, so a few seconds costs nothing.
const PAUSED_POLL: Duration = Duration::from_secs(3);

const PRUNE_INTERVAL: Duration = Duration::from_secs(300);
/// How often an approved-but-idle sensor pings the Standalone so it keeps
/// showing "online" even with nothing to forward. Comfortably under the
/// Standalone's 60s online threshold.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
/// Fallback cache TTL when the sensor config is absent (7 days).
const DEFAULT_CACHE_TTL_SECS: i64 = 604_800;

/// What the forwarder is currently doing, for the Sensor node's own status
/// page.
///
/// The Sensor used to report only whether the Standalone's listener answered
/// a `/sensor/health` GET. That is a property of the network, not of
/// forwarding, so a sensor whose batches were all failing still showed
/// "connected" while the Standalone showed it offline -- the exact
/// contradiction that made this class of failure so hard to read. This
/// reports what the forwarding loop actually achieved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ForwardState {
    /// No usable sensor config (no key/host/port yet, or an invalid key).
    Paused,
    /// Registered but not yet approved by the Standalone's operator.
    Enrolling,
    /// Approved; batches are being sent.
    Forwarding,
}

/// A point-in-time view of [`ForwarderHealth`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct ForwarderSnapshot {
    pub state: ForwardState,
    /// Last time the Standalone answered anything at all (enroll or ingest).
    pub last_contact_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Last time a batch was actually accepted.
    pub last_delivery_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Emissions ACKed since this process started.
    pub delivered_since_start: u64,
    /// Why the last cycle failed, if it did. Cleared by a success, so a
    /// non-null value here means forwarding is broken *right now*.
    pub last_error: Option<String>,
}

/// Shared, live forwarding status. Written by the forwarding loop, read by
/// `GET /api/sensor/status`.
///
/// A `std::sync::Mutex` rather than a tokio one: every critical section is a
/// field assignment with no `.await` inside, so an async lock would buy
/// nothing.
#[derive(Debug)]
pub struct ForwarderHealth {
    inner: std::sync::Mutex<ForwarderSnapshot>,
}

impl Default for ForwarderHealth {
    fn default() -> Self {
        Self {
            inner: std::sync::Mutex::new(ForwarderSnapshot {
                state: ForwardState::Paused,
                last_contact_at: None,
                last_delivery_at: None,
                delivered_since_start: 0,
                last_error: None,
            }),
        }
    }
}

impl ForwarderHealth {
    pub fn snapshot(&self) -> ForwarderSnapshot {
        self.lock().clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ForwarderSnapshot> {
        // A panic while holding this lock would only have left a status
        // field half-written; recovering the data is strictly better than
        // taking down forwarding over a display value.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set_state(&self, state: ForwardState) {
        self.lock().state = state;
    }

    /// The Standalone answered. Clears `last_error` -- it describes the last
    /// *failed* cycle, so leaving it set after a success would keep showing a
    /// problem that has resolved.
    fn record_contact(&self, delivered: usize) {
        let now = chrono::Utc::now();
        let mut guard = self.lock();
        guard.last_contact_at = Some(now);
        guard.last_error = None;
        if delivered > 0 {
            guard.last_delivery_at = Some(now);
            guard.delivered_since_start += delivered as u64;
        }
    }

    fn record_error(&self, err: String) {
        self.lock().last_error = Some(err);
    }
}

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
    health: std::sync::Arc<ForwarderHealth>,
}

impl SensorForwarder {
    pub fn new(pool: PgPool, health: std::sync::Arc<ForwarderHealth>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            pool,
            client,
            health,
        }
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
            // Carry the Standalone's own explanation through to the operator.
            // It returns three different 400s -- `stale batch`, `sensor_id
            // mismatch`, `missing X-Sensor-Id` -- and reporting only the
            // status code collapsed them into one useless message.
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return ForwardOutcome::Error(describe_ingest_rejection(status, &body));
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
        // Every non-approved outcome is still `Pending` (keep retrying), but
        // record *which* one. Enrollment previously failed completely
        // silently, so an operator debugging a sensor that never came online
        // had no way to tell "waiting for you to click Approve" from "the
        // window is shut" from "another node already claimed this
        // sensor_id" from "the host is unreachable".
        match resp {
            Ok(r) if r.status().is_success() => match r.json::<EnrollResponse>().await {
                Ok(body) if body.status == "approved" => return EnrollResult::Approved,
                Ok(body) => self.health.record_error(format!(
                    "awaiting operator approval (status: {})",
                    body.status
                )),
                Err(e) => self
                    .health
                    .record_error(format!("bad enroll response: {e}")),
            },
            Ok(r) if r.status() == reqwest::StatusCode::FORBIDDEN => self.health.record_error(
                "the Standalone's enrollment window is closed — open it on its Sensors page"
                    .to_string(),
            ),
            Ok(r) if r.status() == reqwest::StatusCode::CONFLICT => {
                self.health.record_error(format!(
                    "sensor id \"{}\" is already approved on the Standalone under a different \
                     key — rotate the key there or pick a different sensor id",
                    target.sensor_id
                ))
            }
            Ok(r) => self
                .health
                .record_error(format!("enroll status {}", r.status())),
            Err(e) => self.health.record_error(format!("enroll: {e}")),
        }
        EnrollResult::Pending
    }
}

/// Turn a rejected `/sensor/ingest` response into something an operator can
/// act on.
///
/// `stale batch` gets special treatment because the phrase does not name its
/// own cause: it means the two nodes' clocks disagree by more than the
/// replay window, which is the ordinary state of a just-rebooted node with no
/// RTC. The sensor enrolls fine and shows *online* -- enrollment carries no
/// timestamp -- while every batch is refused, so without naming the clock the
/// operator has no reason to suspect it.
fn describe_ingest_rejection(status: reqwest::StatusCode, body: &str) -> String {
    let body = body.trim();
    if body.contains("stale batch") {
        return format!(
            "the Standalone rejected the batch as stale ({status}): this node's clock and the \
             Standalone's differ by more than {} minutes. Check NTP/timezone on both — \
             enrollment has no timestamp, which is why this node still shows as online while \
             nothing is delivered.",
            MAX_SKEW_MS / 60_000,
        );
    }
    if body.is_empty() {
        format!("ingest status {status}")
    } else {
        format!("ingest status {status}: {body}")
    }
}

/// Mirrors the Standalone's `sensor_listener::MAX_SKEW_MS`, for the operator-
/// facing message above. Kept as a plain constant rather than shared: this
/// side only ever *describes* the window, and a Sensor may be talking to a
/// Standalone on a different build.
const MAX_SKEW_MS: i64 = 300_000;

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
        // Consecutive failed cycles, for the backoff jitter.
        let mut failures: u32 = 0;
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
                forwarder.health.set_state(ForwardState::Paused);
                tokio::time::sleep(PAUSED_POLL).await;
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
                forwarder.health.set_state(ForwardState::Enrolling);
                match forwarder.enroll(&target).await {
                    EnrollResult::Approved => {
                        approved = true;
                        failures = 0;
                        last_contact = std::time::Instant::now();
                        forwarder.health.set_state(ForwardState::Forwarding);
                        forwarder.health.record_contact(0);
                        FORWARD_IDLE
                    }
                    EnrollResult::Pending => {
                        failures = failures.saturating_add(1);
                        backoff_with_jitter(failures)
                    }
                }
            } else {
                forwarder.health.set_state(ForwardState::Forwarding);
                match forwarder.forward_once(&target).await {
                    ForwardOutcome::Delivered(n) => {
                        failures = 0;
                        last_contact = std::time::Instant::now();
                        forwarder.health.record_contact(n);
                        // A full batch means more is queued behind it. Sleeping
                        // the idle interval here caps the drain rate at
                        // FORWARD_BATCH_LIMIT per FORWARD_IDLE regardless of how
                        // fast the link and the Standalone actually are, so a
                        // backlog built in a noisy environment could never be
                        // worked off. Go straight back for the next batch and
                        // let the round trip set the pace.
                        if n as i64 >= FORWARD_BATCH_LIMIT {
                            Duration::ZERO
                        } else {
                            FORWARD_IDLE
                        }
                    }
                    ForwardOutcome::Nothing => {
                        failures = 0;
                        FORWARD_IDLE
                    }
                    ForwardOutcome::NotApproved => {
                        approved = false;
                        failures = failures.saturating_add(1);
                        forwarder.health.record_error(
                            "the Standalone revoked or does not recognise this \
                                           sensor — re-approve it there"
                                .to_string(),
                        );
                        backoff_with_jitter(failures)
                    }
                    ForwardOutcome::Error(e) => {
                        eprintln!("SensorForwarder: {e}");
                        forwarder.health.record_error(e);
                        failures = failures.saturating_add(1);
                        backoff_with_jitter(failures)
                    }
                }
            };

            // Heartbeat, evaluated every cycle rather than only when there was
            // nothing to send.
            //
            // It used to live inside the `Nothing` arm, which meant precisely
            // the sensors that most needed to report in never did: one with a
            // backlog never returns `Nothing`, and one whose batches are
            // failing never gets there either. Both went quiet, the Standalone
            // aged them out at its 60s threshold, and it displayed "offline"
            // for a sensor that was running and trying the whole time.
            //
            // An approved re-enroll only bumps `last_seen_at` (it bypasses the
            // enrollment window), and it is best-effort: a failed heartbeat
            // must not drop approval or interrupt forwarding. Only an ingest
            // 403 authoritatively means approval is gone.
            if approved && last_contact.elapsed() >= HEARTBEAT_INTERVAL {
                if let EnrollResult::Approved = forwarder.enroll(&target).await {
                    last_contact = std::time::Instant::now();
                }
            }

            tokio::time::sleep(delay).await;
        }
    });
}

/// [`FORWARD_BACKOFF`] with up to +50% of random jitter, and no growth beyond
/// the base interval.
///
/// The jitter is what matters here, not the (deliberately absent) escalation.
/// Several sensors reporting to one Standalone tend to fail together — the
/// Standalone restarts, or the operator closes an enrollment window — and a
/// fixed 30s retry then locks them into the same phase, so every future retry
/// arrives as a simultaneous burst. Spreading them stops the fleet from
/// synchronising.
///
/// The interval stays flat because these are operator-resolved conditions
/// ("click Approve", "the host is back"): a sensor that backs off to minutes
/// would take minutes to notice the fix. `failures` is taken so the seed
/// varies per attempt.
fn backoff_with_jitter(failures: u32) -> Duration {
    // A cheap deterministic-per-process spread: the low bits of the current
    // nanosecond clock mixed with the attempt count. Nothing here needs
    // cryptographic randomness, and this avoids pulling in an RNG dependency.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let spread = (nanos ^ failures.wrapping_mul(2_654_435_761)) % 50;
    FORWARD_BACKOFF + (FORWARD_BACKOFF * spread) / 100
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
