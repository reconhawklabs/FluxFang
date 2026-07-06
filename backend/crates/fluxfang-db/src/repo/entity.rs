//! `EntityRepo`: `entity` — the tracked real-world thing an operator groups
//! one or more `emitter`s under.
//!
//! ## Phase 1b: `query` (list search + pagination)
//!
//! [`EntityRepo::query`] is the entities list page's search + pagination,
//! following the same dynamic-WHERE/bind-threading shape as
//! `repo::emission::EmissionRepo::query`/`repo::emitter::EmitterRepo::query`:
//! `filter.search` (case-insensitive substring over `name`/`notes`) is
//! parameterized as `'%' || $n || '%'`, bound as a plain string parameter —
//! never interpolated into the SQL text — so it's not susceptible to SQL
//! injection.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{Entity, NewEntity};

pub struct EntityRepo;

/// Column list shared by every query that produces an [`Entity`]. `pub(crate)`
/// so `repo::emitter`'s `create_with_entity` (Task 6.4) can reuse it for the
/// entity-insert half of its atomic entity+emitter transaction, same
/// precedent as `repo::alert_rule` reusing `repo::alert_method`'s
/// `ALERT_METHOD_COLUMNS`.
pub(crate) const ENTITY_COLUMNS: &str = "id, created_at, name, notes";

/// Filter/paginate criteria for [`EntityRepo::query`] (Phase 1b). `search`,
/// when `Some`, is a case-insensitive substring matched across `name` and
/// `notes`. `limit`/`offset` default to a sane first page, same convention
/// as `repo::emission::EmissionFilter`/`repo::emitter::EmitterListFilter`.
#[derive(Debug, Clone)]
pub struct EntityListFilter {
    pub search: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

impl Default for EntityListFilter {
    fn default() -> Self {
        Self {
            search: None,
            limit: 50,
            offset: 0,
        }
    }
}

impl EntityRepo {
    pub async fn insert(pool: &PgPool, new: NewEntity) -> Result<Entity, sqlx::Error> {
        let sql =
            format!("INSERT INTO entity (name, notes) VALUES ($1, $2) RETURNING {ENTITY_COLUMNS}");
        sqlx::query_as::<_, Entity>(&sql)
            .bind(new.name)
            .bind(new.notes)
            .fetch_one(pool)
            .await
    }

    pub async fn list(pool: &PgPool) -> Result<Vec<Entity>, sqlx::Error> {
        let sql = format!("SELECT {ENTITY_COLUMNS} FROM entity ORDER BY created_at ASC");
        sqlx::query_as::<_, Entity>(&sql).fetch_all(pool).await
    }

    pub async fn get(pool: &PgPool, id: Uuid) -> Result<Option<Entity>, sqlx::Error> {
        let sql = format!("SELECT {ENTITY_COLUMNS} FROM entity WHERE id = $1");
        sqlx::query_as::<_, Entity>(&sql)
            .bind(id)
            .fetch_optional(pool)
            .await
    }

    /// Filter/paginate entities for the entities list page (Phase 1b): see
    /// module docs and [`EntityListFilter`] for the search semantics.
    /// Ordered `created_at ASC`, same as [`Self::list`]. Returns the
    /// requested page plus `total`, the count of matching rows ignoring
    /// `limit`/`offset` (for pagination UIs).
    pub async fn query(
        pool: &PgPool,
        filter: EntityListFilter,
    ) -> Result<(Vec<Entity>, i64), sqlx::Error> {
        let where_sql = if filter.search.is_some() {
            "(name ILIKE $1 OR notes ILIKE $1)"
        } else {
            "TRUE"
        };

        let count_sql = format!("SELECT COUNT(*) FROM entity WHERE {where_sql}");
        let mut count_q = sqlx::query_as::<_, (i64,)>(&count_sql);
        if let Some(ref search) = filter.search {
            count_q = count_q.bind(format!("%{search}%"));
        }
        let (total,) = count_q.fetch_one(pool).await?;

        let (limit_idx, offset_idx) = if filter.search.is_some() {
            (2, 3)
        } else {
            (1, 2)
        };
        let data_sql = format!(
            "SELECT {ENTITY_COLUMNS} FROM entity WHERE {where_sql} \
             ORDER BY created_at ASC LIMIT ${limit_idx} OFFSET ${offset_idx}"
        );
        let mut data_q = sqlx::query_as::<_, Entity>(&data_sql);
        if let Some(ref search) = filter.search {
            data_q = data_q.bind(format!("%{search}%"));
        }
        let rows = data_q
            .bind(filter.limit)
            .bind(filter.offset)
            .fetch_all(pool)
            .await?;

        Ok((rows, total))
    }

    /// `MAX(emission.observed_at)` over every emission whose `emitter_id`
    /// belongs to an emitter with this `entity_id`. `None` when the entity
    /// has no emitters, or its emitters have no emissions yet.
    pub async fn last_seen(
        pool: &PgPool,
        entity_id: Uuid,
    ) -> Result<Option<DateTime<Utc>>, sqlx::Error> {
        let sql = "SELECT MAX(emission.observed_at) \
                   FROM emission \
                   JOIN emitter ON emitter.id = emission.emitter_id \
                   WHERE emitter.entity_id = $1";
        let (max,): (Option<DateTime<Utc>>,) =
            sqlx::query_as(sql).bind(entity_id).fetch_one(pool).await?;
        Ok(max)
    }

    /// Full replacement of `name`/`notes`, same "PATCH re-sends the row's
    /// full desired value for every field it's touching" convention
    /// `ZoneRepo::update`/`EmitterRepo::update_basic` use — the caller
    /// (`fluxfang-api`'s handler) is responsible for resolving "field
    /// omitted from the request" against the existing row before calling
    /// this.
    pub async fn update(
        pool: &PgPool,
        id: Uuid,
        name: &str,
        notes: Option<&str>,
    ) -> Result<Entity, sqlx::Error> {
        let sql = format!(
            "UPDATE entity SET name = $2, notes = $3 WHERE id = $1 RETURNING {ENTITY_COLUMNS}"
        );
        sqlx::query_as::<_, Entity>(&sql)
            .bind(id)
            .bind(name)
            .bind(notes)
            .fetch_one(pool)
            .await
    }

    /// Delete an entity, returning whether a row was actually removed.
    /// `emitter.entity_id` is `ON DELETE SET NULL` (see
    /// `migrations/0001_init.sql`), so any emitters previously grouped under
    /// this entity survive, just detached (their own emissions untouched).
    pub async fn delete(pool: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM entity WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}
