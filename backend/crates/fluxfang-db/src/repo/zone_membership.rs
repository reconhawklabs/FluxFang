//! `ZoneMembershipRepo`: `zone_membership` — ingest-maintained last-known
//! membership state for one subject (`emitter`, `entity`, or the singular
//! `host`) in one `zone`, used so enter/leave alert triggers fire once per
//! transition rather than once per emission.
//!
//! ## The `host` subject and `subject_id = NULL`
//!
//! `subject_type = 'host'` rows always have `subject_id = NULL` (there is
//! only one host — the machine doing the surveying — so it needs no id).
//! The schema's unique index is declared `NULLS NOT DISTINCT`
//! (`zone_membership_subject_zone_uidx` in `0001_init.sql`), which makes
//! Postgres treat two NULLs in the `subject_id` column as *equal* for
//! uniqueness purposes — so there can only ever be one `('host', NULL,
//! zone_id)` row, which is exactly what `upsert`'s `ON CONFLICT` target
//! needs to land on.
//!
//! Ordinary SQL equality (`subject_id = $2`) never matches when `$2` is
//! NULL (`NULL = NULL` is NULL, not true), so [`ZoneMembershipRepo::get`]
//! uses `subject_id IS NOT DISTINCT FROM $2` instead — this correctly
//! matches the host row when `subject_id: None` is passed, while still
//! behaving like ordinary equality for non-NULL subject ids.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::ZoneMembership;

pub struct ZoneMembershipRepo;

impl ZoneMembershipRepo {
    /// Look up the membership row for one `(subject_type, subject_id,
    /// zone_id)`. `subject_id: None` matches the `host` row (see module
    /// docs on why `IS NOT DISTINCT FROM` is required here instead of `=`).
    pub async fn get(
        pool: &PgPool,
        subject_type: &str,
        subject_id: Option<Uuid>,
        zone_id: Uuid,
    ) -> Result<Option<ZoneMembership>, sqlx::Error> {
        sqlx::query_as::<_, ZoneMembership>(
            "SELECT * FROM zone_membership \
             WHERE subject_type = $1 \
               AND subject_id IS NOT DISTINCT FROM $2 \
               AND zone_id = $3",
        )
        .bind(subject_type)
        .bind(subject_id)
        .bind(zone_id)
        .fetch_optional(pool)
        .await
    }

    /// Insert-or-update the membership row for one `(subject_type,
    /// subject_id, zone_id)`. Relies on the schema's `NULLS NOT DISTINCT`
    /// unique index to dedupe `subject_id = NULL` (`host`) rows to exactly
    /// one per zone — a second call with the same `subject_type`/
    /// `subject_id`/`zone_id` updates that same row's `inside`/`since`
    /// rather than inserting a duplicate (see module docs, and the
    /// `upsert_host_subject_twice_yields_exactly_one_row...` test in
    /// `tests/repo_zone_membership.rs` which asserts this directly against
    /// the DB row count).
    pub async fn upsert(
        pool: &PgPool,
        subject_type: &str,
        subject_id: Option<Uuid>,
        zone_id: Uuid,
        inside: bool,
        since: DateTime<Utc>,
    ) -> Result<ZoneMembership, sqlx::Error> {
        sqlx::query_as::<_, ZoneMembership>(
            "INSERT INTO zone_membership (subject_type, subject_id, zone_id, inside, since) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (subject_type, subject_id, zone_id) DO UPDATE \
             SET inside = excluded.inside, since = excluded.since \
             RETURNING *",
        )
        .bind(subject_type)
        .bind(subject_id)
        .bind(zone_id)
        .bind(inside)
        .bind(since)
        .fetch_one(pool)
        .await
    }
}
