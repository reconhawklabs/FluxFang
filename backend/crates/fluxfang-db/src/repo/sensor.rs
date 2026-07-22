//! `SensorRepo`: the per-listener keyring of distributed Sensor nodes.

use sqlx::PgPool;
use uuid::Uuid;

use crate::models::Sensor;

pub struct SensorRepo;

impl SensorRepo {
    /// Create a new `pending` sensor for `data_source_id`.
    ///
    /// The symmetric key is NOT known at enrollment time — a sensor transmits
    /// only its id + one-way `fingerprint`. The operator supplies the key in
    /// the approval dialog, which is when [`Self::set_key`] fills it in. Until
    /// then `key` is stored as the empty string (the codebase's "no key yet"
    /// sentinel); an empty key can never `decode_key`, so such a row is
    /// unusable for ingest — exactly right for a not-yet-approved sensor.
    pub async fn insert_pending(
        pool: &PgPool,
        data_source_id: Uuid,
        sensor_id: &str,
        fingerprint: &str,
        source_ip: Option<&str>,
    ) -> Result<Sensor, sqlx::Error> {
        sqlx::query_as::<_, Sensor>(
            "INSERT INTO sensor (data_source_id, sensor_id, key, fingerprint, source_ip, last_seen_at) \
             VALUES ($1, $2, '', $3, $4, now()) RETURNING *",
        )
        .bind(data_source_id)
        .bind(sensor_id)
        .bind(fingerprint)
        .bind(source_ip)
        .fetch_one(pool)
        .await
    }

    pub async fn get(pool: &PgPool, id: Uuid) -> Result<Option<Sensor>, sqlx::Error> {
        sqlx::query_as::<_, Sensor>("SELECT * FROM sensor WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await
    }

    pub async fn get_by_sensor_id(
        pool: &PgPool,
        data_source_id: Uuid,
        sensor_id: &str,
    ) -> Result<Option<Sensor>, sqlx::Error> {
        sqlx::query_as::<_, Sensor>(
            "SELECT * FROM sensor WHERE data_source_id = $1 AND sensor_id = $2",
        )
        .bind(data_source_id)
        .bind(sensor_id)
        .fetch_optional(pool)
        .await
    }

    pub async fn list(pool: &PgPool) -> Result<Vec<Sensor>, sqlx::Error> {
        sqlx::query_as::<_, Sensor>("SELECT * FROM sensor ORDER BY created_at")
            .fetch_all(pool)
            .await
    }

    /// Update the fingerprint/source_ip and bump last_seen for a sensor
    /// re-enrolling while still `pending`. The key is never carried on the
    /// wire, so this only refreshes the claimed fingerprint (the operator
    /// still supplies the actual key at approval).
    ///
    /// Guarded by `status = 'pending'` in the WHERE clause: without it, a
    /// TOCTOU window exists between the enroll handler's read of the sensor
    /// row and this write — if an operator approves the sensor in that gap,
    /// this write would land on the now-`approved` row and silently swap in
    /// an attacker-supplied fingerprint the operator never verified. The
    /// guard makes that race safe: if the row is no longer `pending` by the
    /// time this UPDATE runs, it matches zero rows and we return `Ok(None)`
    /// instead of overwriting.
    pub async fn update_pending_fingerprint(
        pool: &PgPool,
        id: Uuid,
        fingerprint: &str,
        source_ip: Option<&str>,
    ) -> Result<Option<Sensor>, sqlx::Error> {
        sqlx::query_as::<_, Sensor>(
            "UPDATE sensor SET fingerprint = $2, source_ip = $3, last_seen_at = now() \
             WHERE id = $1 AND status = 'pending' RETURNING *",
        )
        .bind(id)
        .bind(fingerprint)
        .bind(source_ip)
        .fetch_optional(pool)
        .await
    }

    /// Bump `last_seen_at` for an `approved` sensor. Guarded by
    /// `status = 'approved'` (defense in depth) so a sensor that was just
    /// revoked mid-request doesn't get its last-seen/online state bumped
    /// back up by a race with the revoke.
    pub async fn touch_last_seen(pool: &PgPool, id: Uuid) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE sensor SET last_seen_at = now() WHERE id = $1 AND status = 'approved'")
            .bind(id)
            .execute(pool)
            .await
            .map(|_| ())
    }

    /// Set status; when `stamp_approved` is true also set `approved_at = now()`.
    pub async fn set_status(
        pool: &PgPool,
        id: Uuid,
        status: &str,
        stamp_approved: bool,
    ) -> Result<Sensor, sqlx::Error> {
        sqlx::query_as::<_, Sensor>(
            "UPDATE sensor SET status = $2, \
                approved_at = CASE WHEN $3 THEN now() ELSE approved_at END \
             WHERE id = $1 RETURNING *",
        )
        .bind(id)
        .bind(status)
        .bind(stamp_approved)
        .fetch_one(pool)
        .await
    }

    pub async fn set_auto_group(pool: &PgPool, id: Uuid, on: bool) -> Result<Sensor, sqlx::Error> {
        sqlx::query_as::<_, Sensor>(
            "UPDATE sensor SET auto_group_emitters = $2 WHERE id = $1 RETURNING *",
        )
        .bind(id)
        .bind(on)
        .fetch_one(pool)
        .await
    }

    pub async fn set_key(
        pool: &PgPool,
        id: Uuid,
        key: &str,
        fingerprint: &str,
    ) -> Result<Sensor, sqlx::Error> {
        sqlx::query_as::<_, Sensor>(
            "UPDATE sensor SET key = $2, fingerprint = $3 WHERE id = $1 RETURNING *",
        )
        .bind(id)
        .bind(key)
        .bind(fingerprint)
        .fetch_one(pool)
        .await
    }
}
