//! Ingest throughput regression: a sensor batch must cost the same whether
//! the Standalone knows about 500 emitters or 10,000.
//!
//! ## The bug this pins
//!
//! `finalize_emission`'s auto-attach used to call `EmitterRepo::list` — a
//! full `SELECT * FROM emitter ORDER BY created_at` — and then
//! `serde_json::from_value::<Rule>` every row, **once per ingested
//! emission**. On a real deployment (9,375 emitters) that is ~1.9M rule
//! parses and 200 full-table fetches for a single 200-emission batch, which
//! comfortably outran the forwarder's HTTP timeout. The forwarder then never
//! received its ACK, retried the same rows forever, and the Sensor's cache
//! grew without bound while the Standalone showed the sensor offline.
//!
//! Because the cost is quadratic in "emitters discovered so far", a node
//! works fine when new and stops forwarding entirely once it has surveyed a
//! busy area — exactly the reported failure.
//!
//! ## Why a ratio and not a wall-clock bound
//!
//! An absolute "must finish in N seconds" bound bakes in the speed of
//! whatever machine runs the suite. The property that actually matters is
//! *scale invariance*: ingest must not get slower as the emitter table
//! grows. So this measures the same batch twice — once against a small
//! emitter table, once against a 20x larger one — and asserts the large run
//! stays within a generous multiple of the small one. The pre-fix code shows
//! a ~20x ratio here; the fix holds it near 1x, so the 4x threshold has a
//! wide margin on both sides and does not flake on a loaded CI box.

mod common;

use std::time::{Duration, Instant};

use fluxfang_db::{DataSourceRepo, NewDataSource, SensorRepo};
use sqlx::PgPool;

/// Emitters present for the baseline measurement.
const SMALL_EMITTERS: i64 = 500;
/// Emitters present for the comparison measurement (20x the baseline).
const LARGE_EMITTERS: i64 = 10_000;
/// Emissions per measured batch. Enough that per-emission cost dominates
/// the fixed per-batch overhead, small enough that the pre-fix run finishes.
const BATCH: usize = 40;
/// How much slower the 20x-emitters run may be before we call it "scales
/// with the emitter table". See the module docs for why this is a ratio.
const MAX_SLOWDOWN: u32 = 4;
/// Floor for the baseline duration. Guards the ratio against dividing by a
/// near-zero measurement on a fast machine, which would make any large-run
/// duration look like a huge multiple.
const BASELINE_FLOOR: Duration = Duration::from_millis(50);

async fn free_port() -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p = l.local_addr().unwrap().port();
    drop(l);
    p
}

/// Bulk-insert `count` emitters whose match rules have the exact shape
/// auto-create produces (`{match: all, conditions: [{field: bssid, op: eq,
/// value: <mac>}]}`) — 99.5% of emitters on a real deployment. None of them
/// can match the emissions this test ingests: the generated MACs are all in
/// the `de:ad:*` space and the emissions use `11:22:*`, so every ingest walks
/// the whole rule set without short-circuiting on an early hit. That is the
/// worst case, and the one a busy environment actually produces.
async fn seed_emitters(pool: &PgPool, count: i64, offset: i64) {
    sqlx::query(
        "INSERT INTO emitter (name, emitter_type, type, match_criteria, match_enabled, attributes)
         SELECT
           'seed-' || g,
           'wifi_ap',
           'WiFi AP',
           jsonb_build_object(
             'match', 'all',
             'conditions', jsonb_build_array(jsonb_build_object(
               'field', 'bssid',
               'op', 'eq',
               'value', 'de:ad:' || to_char(g / 16777216 % 256, 'FM00') || ':'
                         || to_char(g / 65536 % 256, 'FM00') || ':'
                         || to_char(g / 256 % 256, 'FM00') || ':'
                         || to_char(g % 256, 'FM00')
             ))
           ),
           true,
           '{}'::jsonb
         FROM generate_series($1::bigint, $1::bigint + $2::bigint - 1) AS g",
    )
    .bind(offset)
    .bind(count)
    .execute(pool)
    .await
    .expect("seed emitters");
}

/// A sealed batch of `BATCH` emissions, each with a distinct BSSID that
/// matches none of the seeded emitters.
fn sealed_batch(key: &fluxfang_sensor_proto::Key, tag: u16) -> Vec<u8> {
    let emissions = (0..BATCH)
        .map(|i| fluxfang_sensor_proto::WireEmission {
            id: uuid::Uuid::new_v4(),
            kind: "wifi".into(),
            signal_strength: Some(-40),
            lat: None,
            lon: None,
            observed_at: chrono::Utc::now(),
            payload: serde_json::json!({
                "bssid": format!("11:22:33:44:{:02x}:{:02x}", tag, i),
                "frame_type": "beacon",
            }),
        })
        .collect();
    let batch = fluxfang_sensor_proto::SensorBatch {
        sensor_id: "frontgate".into(),
        sent_at_ms: chrono::Utc::now().timestamp_millis(),
        emissions,
    };
    fluxfang_sensor_proto::seal_batch(key, &batch).unwrap()
}

/// POST one sealed batch and return how long the Standalone took to accept
/// it. Asserts the batch was fully accepted so a timing win from silently
/// dropping work can't pass as a speedup.
async fn time_batch(port: u16, key: &fluxfang_sensor_proto::Key, tag: u16) -> Duration {
    let sealed = sealed_batch(key, tag);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .unwrap();
    let started = Instant::now();
    let resp = client
        .post(format!("http://127.0.0.1:{port}/sensor/ingest"))
        .header("X-Sensor-Id", "frontgate")
        .body(sealed)
        .send()
        .await
        .expect("ingest request");
    assert!(
        resp.status().is_success(),
        "ingest failed: {:?}",
        resp.status()
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    let elapsed = started.elapsed();
    assert_eq!(
        body["accepted"].as_array().map(Vec::len),
        Some(BATCH),
        "the whole batch must be ACKed; a partial ACK would make the timing meaningless",
    );
    elapsed
}

#[tokio::test]
async fn sensor_batch_ingest_does_not_slow_down_as_the_emitter_table_grows() {
    let pool = common::fresh_pool_shared().await;
    let port = free_port().await;
    let ds = DataSourceRepo::insert(
        &pool,
        NewDataSource {
            kind: "sensor".into(),
            mode: "listener".into(),
            interface: None,
            config: serde_json::json!({"bind_ip":"127.0.0.1","bind_port":port}),
        },
    )
    .await
    .unwrap();

    let key = fluxfang_sensor_proto::generate_key();
    let key_b64 = fluxfang_sensor_proto::encode_key(&key);
    let fp = fluxfang_sensor_proto::fingerprint("frontgate", &key);
    let s = SensorRepo::insert_pending(&pool, ds.id, "frontgate", &fp, None)
        .await
        .unwrap();
    SensorRepo::set_key(&pool, s.id, &key_b64, &fp)
        .await
        .unwrap();
    SensorRepo::set_status(&pool, s.id, "approved", true)
        .await
        .unwrap();
    // auto_group_emitters defaults true, so these batches take the full
    // auto-attach path (`GroupingPolicy::RemoteGrouped`) -- the path that
    // holds the bug. With it off the emissions would be strays and skip
    // matching entirely, testing nothing.
    SensorRepo::set_auto_group(&pool, s.id, true).await.unwrap();

    let mgr = common::sensor_manager(pool.clone());
    mgr.start(ds.id).await;

    seed_emitters(&pool, SMALL_EMITTERS, 0).await;
    // Discard the first batch: it pays one-time costs (connection warmup,
    // query plan caching) that would inflate the baseline and mask a real
    // regression.
    let _warmup = time_batch(port, &key, 0).await;
    let small = time_batch(port, &key, 1).await;

    seed_emitters(&pool, LARGE_EMITTERS - SMALL_EMITTERS, SMALL_EMITTERS).await;
    let large = time_batch(port, &key, 2).await;

    let baseline = small.max(BASELINE_FLOOR);
    assert!(
        large <= baseline * MAX_SLOWDOWN,
        "ingest cost scales with the emitter table: {BATCH} emissions took {small:?} with \
         {SMALL_EMITTERS} emitters but {large:?} with {LARGE_EMITTERS} ({}x the emitters). \
         Auto-attach must not re-read and re-parse every emitter rule per emission.",
        LARGE_EMITTERS / SMALL_EMITTERS,
    );

    mgr.stop(ds.id).await;
}
