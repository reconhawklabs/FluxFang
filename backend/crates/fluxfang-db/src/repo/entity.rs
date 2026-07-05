//! `EntityRepo`: `entity` — the tracked real-world thing an operator groups
//! one or more `emitter`s under.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{Entity, NewEntity};

pub struct EntityRepo;

/// Column list shared by every query that produces an [`Entity`].
const ENTITY_COLUMNS: &str = "id, created_at, name, notes";

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
}
