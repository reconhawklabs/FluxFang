//! `SessionRepo`: `survey_session` — bounds a continuous capture period.
//!
//! At most one session is "active" (`ended_at IS NULL`) at a time; the
//! application layer is responsible for calling `close_active` before
//! `open`-ing a new one if that invariant matters to a caller. As of Task
//! 5.1 this is *also* enforced at the DB level by the partial unique
//! index in `0002_single_active_session.sql` (belt-and-suspenders: the
//! index makes the bad state impossible to persist even if a caller
//! forgets to self-heal; `close_active` below is the self-heal itself).

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

    /// Close *every* currently-open session (`ended_at = now()` on every
    /// row where `ended_at IS NULL`), returning the most-recently-started
    /// one (or `None` if nothing was open).
    ///
    /// Self-healing: this closes *all* open rows, not just the most
    /// recent, on purpose. A prior version of this query only closed the
    /// single most-recent open row (via a `LIMIT 1` subquery) — correct
    /// only under the assumption that at most one row can ever have
    /// `ended_at IS NULL`, which nothing enforced at the time. If that
    /// assumption were ever violated (a race, a direct SQL insert
    /// bypassing `open`, restoring from a backup taken mid-session, ...),
    /// the old query would leave every open row but the newest dangling
    /// open forever. Closing every open row is correct regardless, and is
    /// a cheap no-op in the (now DB-enforced, see
    /// `0002_single_active_session.sql`) common case of at most one such
    /// row.
    pub async fn close_active(pool: &PgPool) -> Result<Option<SurveySession>, sqlx::Error> {
        let mut closed = sqlx::query_as::<_, SurveySession>(
            "UPDATE survey_session SET ended_at = now() \
             WHERE ended_at IS NULL \
             RETURNING *",
        )
        .fetch_all(pool)
        .await?;

        // Preserve the old "return the most-recently-started one" contract
        // for callers that only care about a single session.
        closed.sort_by_key(|s| std::cmp::Reverse(s.started_at));
        Ok(closed.into_iter().next())
    }
}
