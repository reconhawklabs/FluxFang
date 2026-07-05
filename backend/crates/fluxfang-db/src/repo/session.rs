//! `SessionRepo`: `survey_session` — bounds a continuous capture period.
//!
//! At most one session is "active" (`ended_at IS NULL`) at a time; the
//! application layer is responsible for calling `close_active` before
//! `open`-ing a new one if that invariant matters to a caller (this repo
//! does not enforce it with a DB constraint — see concerns in the task
//! report).

use sqlx::PgPool;

use crate::models::SurveySession;

pub struct SessionRepo;

impl SessionRepo {
    /// Start a new session (`started_at = now()`, `ended_at = NULL`).
    pub async fn open(pool: &PgPool) -> Result<SurveySession, sqlx::Error> {
        sqlx::query_as::<_, SurveySession>(
            "INSERT INTO survey_session (started_at) VALUES (now()) RETURNING *",
        )
        .fetch_one(pool)
        .await
    }

    /// The currently-open session (`ended_at IS NULL`), if any.
    pub async fn active(pool: &PgPool) -> Result<Option<SurveySession>, sqlx::Error> {
        sqlx::query_as::<_, SurveySession>(
            "SELECT * FROM survey_session WHERE ended_at IS NULL \
             ORDER BY started_at DESC LIMIT 1",
        )
        .fetch_optional(pool)
        .await
    }

    /// Close the currently-open session (`ended_at = now()`), returning it,
    /// or `None` if there was no active session.
    pub async fn close_active(pool: &PgPool) -> Result<Option<SurveySession>, sqlx::Error> {
        sqlx::query_as::<_, SurveySession>(
            "UPDATE survey_session SET ended_at = now() \
             WHERE id = ( \
                 SELECT id FROM survey_session \
                 WHERE ended_at IS NULL ORDER BY started_at DESC LIMIT 1 \
             ) \
             RETURNING *",
        )
        .fetch_optional(pool)
        .await
    }
}
