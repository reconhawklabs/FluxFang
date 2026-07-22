//! `emitter_association`: bidirectional emitter<->emitter links (Spec B,
//! "Other Tires on the same Car"). Modeled on `AlertRuleRepo`'s join-table
//! methods. Every link is stored as two rows (a->b and b->a), written and
//! removed together in one transaction so either emitter lists the other.

use sqlx::PgPool;
use uuid::Uuid;

use crate::models::Emitter;
use crate::repo::emitter::emitter_columns_qualified;

/// One associated emitter plus how the link was made.
pub struct AssociatedEmitter {
    pub emitter: Emitter,
    pub source: String,
    pub confidence: Option<f64>,
}

/// Row shape for `list_for`'s join: the joined emitter's columns flattened,
/// plus the association's `source`/`confidence`.
///
/// The association's own `source` column is selected as `assoc_source` (see
/// `list_for`'s SQL) and mapped here via `#[sqlx(rename)]`: `emitter` now
/// also has its own `source` column (Task 2, migration `0012`), and Postgres
/// happily returns two result columns both literally named `source` for an
/// unaliased `SELECT ... e.source, ..., ea.source ...` — sqlx's by-name
/// column lookup would then resolve `AssocRow::source` to whichever of the
/// two same-named columns wins, silently corrupting one of the two `source`
/// values. Aliasing the association's copy avoids the collision entirely.
#[derive(sqlx::FromRow)]
struct AssocRow {
    #[sqlx(flatten)]
    emitter: Emitter,
    #[sqlx(rename = "assoc_source")]
    source: String,
    confidence: Option<f64>,
}

pub struct EmitterAssociationRepo;

impl EmitterAssociationRepo {
    /// Add a bidirectional association. Both directions are written in one
    /// transaction. Conflict handling makes `manual` authoritative: a manual
    /// add upgrades an existing auto row; an auto add never downgrades a
    /// manual row.
    pub async fn add(
        pool: &PgPool,
        emitter_id: Uuid,
        associated_id: Uuid,
        source: &str,
        confidence: Option<f64>,
    ) -> Result<(), sqlx::Error> {
        let mut tx = pool.begin().await?;
        for (a, b) in [(emitter_id, associated_id), (associated_id, emitter_id)] {
            // manual: upgrade an existing row; auto: leave any existing row
            // (manual OR auto) untouched.
            let conflict = if source == "manual" {
                "DO UPDATE SET source = 'manual', confidence = NULL"
            } else {
                "DO NOTHING"
            };
            let sql = format!(
                "INSERT INTO emitter_association \
                 (emitter_id, associated_emitter_id, source, confidence) \
                 VALUES ($1, $2, $3, $4) \
                 ON CONFLICT (emitter_id, associated_emitter_id) {conflict}"
            );
            sqlx::query(&sql)
                .bind(a)
                .bind(b)
                .bind(source)
                .bind(confidence)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await
    }

    /// Remove a bidirectional association (both rows), if present.
    pub async fn remove(
        pool: &PgPool,
        emitter_id: Uuid,
        associated_id: Uuid,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "DELETE FROM emitter_association \
             WHERE (emitter_id = $1 AND associated_emitter_id = $2) \
                OR (emitter_id = $2 AND associated_emitter_id = $1)",
        )
        .bind(emitter_id)
        .bind(associated_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// List the emitters associated with `emitter_id`, plus each link's
    /// source/confidence, ordered by the associated emitter's name.
    pub async fn list_for(
        pool: &PgPool,
        emitter_id: Uuid,
    ) -> Result<Vec<AssociatedEmitter>, sqlx::Error> {
        // Qualify EMITTER_COLUMNS with the `e` alias for the join (handles the
        // est_location `ST_X(...)` unpacking that a blind `e.<col>` prefix can't).
        let cols = emitter_columns_qualified("e");
        let sql = format!(
            "SELECT {cols}, ea.source AS assoc_source, ea.confidence \
             FROM emitter_association ea \
             JOIN emitter e ON e.id = ea.associated_emitter_id \
             WHERE ea.emitter_id = $1 \
             ORDER BY e.name"
        );
        let rows = sqlx::query_as::<_, AssocRow>(&sql)
            .bind(emitter_id)
            .fetch_all(pool)
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| AssociatedEmitter {
                emitter: r.emitter,
                source: r.source,
                confidence: r.confidence,
            })
            .collect())
    }

    /// Whether a link (in the given direction — they're kept symmetric) exists.
    pub async fn exists(
        pool: &PgPool,
        emitter_id: Uuid,
        associated_id: Uuid,
    ) -> Result<bool, sqlx::Error> {
        let found: Option<(Uuid,)> = sqlx::query_as(
            "SELECT emitter_id FROM emitter_association \
             WHERE emitter_id = $1 AND associated_emitter_id = $2",
        )
        .bind(emitter_id)
        .bind(associated_id)
        .fetch_optional(pool)
        .await?;
        Ok(found.is_some())
    }
}
