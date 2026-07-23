//! [`EmitterMatchIndex`]: the parsed emitter auto-attach rule set, held in
//! memory and refreshed only when it actually changes.
//!
//! ## Why this exists
//!
//! Auto-attach asks the same question of every ingested emission: "which is
//! the oldest enabled emitter whose rule matches this payload?" Answering it
//! used to mean a full `SELECT * FROM emitter ORDER BY created_at` plus a
//! `serde_json::from_value::<Rule>` of every row — per emission. Both halves
//! scale with the emitter table, so a node got slower at ingest the more
//! devices it had seen:
//!
//! | emitters | per-emission auto-attach |
//! |----------|--------------------------|
//! | 500      | ~24ms                    |
//! | 10,000   | ~262ms                   |
//!
//! On a Standalone holding ~9,000 emitters that put a 200-emission sensor
//! batch at roughly 49 seconds, past the forwarder's HTTP timeout. The
//! forwarder never got its ACK, retried the identical rows forever, and the
//! Sensor's local cache grew without bound while the Standalone showed the
//! sensor offline — see `tests/sensor_ingest_scale.rs`.
//!
//! ## What changed, and what deliberately did not
//!
//! The rules are fetched and parsed once per *change*, not once per
//! emission. Matching itself still calls the same
//! [`fluxfang_core::rule::eval`] over the same rules in the same
//! `created_at ASC` order, so first-match-wins and every other matching
//! semantic is bit-for-bit what it was. This is purely about not redoing the
//! fetch and the parse.
//!
//! ## Staleness
//!
//! [`fluxfang_db::EmitterRepo::match_version`] is a single-row counter a DB
//! trigger bumps on any insert/delete/rule-change of an emitter (migration
//! 0021). [`EmitterMatchIndex::first_match`] reads it before every match and
//! rebuilds when it moved, so the snapshot is never stale — it is a cache
//! with an exact invalidation signal, not a TTL. A read costs one
//! primary-key lookup; a rebuild costs one narrow scan.
//!
//! Correctness does not depend on the counter being perfectly tight. It may
//! over-report (a rolled-back write still counts, causing a harmless extra
//! rebuild); it cannot under-report, because the trigger's UPDATE commits in
//! the same transaction as the change that fired it.
//!
//! ## Why appends are structural, not a `Vec` copy
//!
//! Ingest creates emitters as it discovers devices, and every creation bumps
//! the version — so without special handling, a busy environment would
//! rebuild the whole index per emission and stay exactly as linear as
//! before. [`EmitterMatchIndex::note_created`] folds a just-created emitter
//! in instead. That path has to be O(1): copying a 10,000-entry rule vector
//! per discovered device is the same quadratic cost wearing a different hat.
//! Hence the [`Snapshot`] split below — a shared immutable `base` behind an
//! `Arc` that is never copied, plus a short `appended` tail.

use std::sync::Arc;

use fluxfang_core::rule::{eval, Rule};
use fluxfang_db::EmitterRepo;
use sqlx::PgPool;
use tokio::sync::RwLock;
use uuid::Uuid;

/// How many auto-created emitters may accumulate in a snapshot's `appended`
/// tail before the next lookup is made to rebuild from scratch instead.
///
/// Each append copies the tail (not the base), so an unbounded tail would be
/// quadratic in its own right. Rebuilding folds the tail into the shared
/// base, resetting that cost. The exact number matters little: it trades a
/// bounded per-append copy against how often a full reload happens.
const MAX_APPENDED: usize = 256;

/// One emitter's parsed rule, ready to evaluate.
struct ParsedRule {
    emitter_id: Uuid,
    rule: Rule,
}

/// An immutable rule set plus the [`EmitterRepo::match_version`] it was built
/// from. Shared behind an `Arc` so a match can evaluate against it without
/// holding the lock.
///
/// Split in two so [`EmitterMatchIndex::note_created`] is O(tail) rather than
/// O(all emitters): `base` is whatever the last full reload produced and is
/// shared by pointer, `appended` holds emitters auto-created since. Both are
/// in `created_at ASC` order and everything in `appended` was created after
/// everything in `base`, so iterating base-then-appended is the exact order
/// `EmitterRepo::list_match_rules` would have returned.
struct Snapshot {
    version: i64,
    base: Arc<Vec<ParsedRule>>,
    appended: Vec<ParsedRule>,
}

impl Snapshot {
    fn rules(&self) -> impl Iterator<Item = &ParsedRule> {
        self.base.iter().chain(self.appended.iter())
    }
}

/// The result of a match, including the rule-set version it was answered
/// from — [`EmitterMatchIndex::note_created`] needs that to tell "the only
/// change since I looked was my own insert" from "somebody else changed
/// something too".
pub struct MatchOutcome {
    pub emitter_id: Option<Uuid>,
    pub version: i64,
}

/// Cache of the parsed emitter rule set. Cheap to clone (one `Arc`); intended
/// to live in `IngestCtx` and be shared by every ingest path on the node.
#[derive(Clone)]
pub struct EmitterMatchIndex {
    inner: Arc<RwLock<Option<Arc<Snapshot>>>>,
}

impl Default for EmitterMatchIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl EmitterMatchIndex {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(None)),
        }
    }

    /// The emitter id of the first enabled rule matching `payload`, in
    /// `created_at ASC` order, plus the rule-set version that answer was
    /// computed against.
    ///
    /// Errors propagate: a caller that cannot read the rule set must not
    /// silently treat the emission as unmatched, because auto-create would
    /// then mint a duplicate emitter for an identity that already has one.
    pub async fn first_match(
        &self,
        pool: &PgPool,
        payload: &serde_json::Value,
    ) -> Result<MatchOutcome, sqlx::Error> {
        let snapshot = self.snapshot(pool).await?;
        let emitter_id = snapshot
            .rules()
            .find(|entry| eval(&entry.rule, payload))
            .map(|entry| entry.emitter_id);
        Ok(MatchOutcome {
            emitter_id,
            version: snapshot.version,
        })
    }

    /// Fold an emitter that ingest *itself* just auto-created into the
    /// snapshot, instead of letting the resulting version bump throw the
    /// whole rule set away.
    ///
    /// Without this the fix is self-defeating at exactly the scale it was
    /// written for: discovering a new device bumps the version, which forces
    /// the next emission to reload from scratch, so an environment full of
    /// unfamiliar (or randomized) addresses stays linear in the emitter
    /// count. On the 10,000-emitter regression test a 40-emission batch takes
    /// ~5.6s reloading per discovery and ~0.3s with this.
    ///
    /// `base_version` is the version [`Self::first_match`] just answered
    /// from. The append is only taken when the database is now exactly one
    /// bump ahead of the snapshot, which — given a successful insert bumps
    /// exactly once — proves this insert is the only change the snapshot has
    /// not seen. Any other delta (a concurrent rule edit, another node
    /// action) leaves the snapshot untouched and stale, so the next lookup
    /// reloads. Being conservative here costs a reload; being optimistic
    /// would silently swallow somebody else's change.
    ///
    /// The caller must only report a genuinely *new* row. Appending assumes
    /// this emitter sorts last by `created_at`, which is what keeps
    /// first-match-wins intact; an emitter that already existed could belong
    /// anywhere in the order.
    pub async fn note_created(
        &self,
        pool: &PgPool,
        base_version: i64,
        emitter_id: Uuid,
        match_criteria: &serde_json::Value,
    ) -> Result<(), sqlx::Error> {
        let Ok(rule) = serde_json::from_value::<Rule>(match_criteria.clone()) else {
            // Unparseable rule: a reload would have dropped it too, so
            // leaving it out is consistent. Skip without adopting the new
            // version — the next lookup reloads, which is correct and rare.
            return Ok(());
        };

        let version = EmitterRepo::match_version(pool).await?;
        let mut guard = self.inner.write().await;
        let Some(current) = guard.as_ref() else {
            return Ok(()); // Nothing cached yet; the next lookup builds it.
        };
        if current.version != base_version
            || version != base_version + 1
            || current.appended.len() >= MAX_APPENDED
        {
            // Someone else changed the rule set too, or the tail is long
            // enough that a reload is the cheaper way to absorb it. Leave the
            // snapshot stale so the next lookup rebuilds.
            return Ok(());
        }

        let mut appended = Vec::with_capacity(current.appended.len() + 1);
        appended.extend(current.appended.iter().map(|entry| ParsedRule {
            emitter_id: entry.emitter_id,
            rule: entry.rule.clone(),
        }));
        appended.push(ParsedRule { emitter_id, rule });
        *guard = Some(Arc::new(Snapshot {
            version,
            // Shared by pointer: the expensive part is never copied.
            base: current.base.clone(),
            appended,
        }));
        Ok(())
    }

    /// The current snapshot, reloading first if the rule set changed.
    async fn snapshot(&self, pool: &PgPool) -> Result<Arc<Snapshot>, sqlx::Error> {
        let version = EmitterRepo::match_version(pool).await?;

        // Fast path: a read lock and an integer compare, which is what the
        // overwhelming majority of emissions hit.
        if let Some(current) = self.inner.read().await.as_ref() {
            if current.version == version {
                return Ok(current.clone());
            }
        }

        let mut guard = self.inner.write().await;
        // Re-check under the write lock: several emissions can find the
        // snapshot stale at once, and only the first should pay for the
        // reload.
        if let Some(current) = guard.as_ref() {
            if current.version == version {
                return Ok(current.clone());
            }
        }

        // The version was read *before* the rules and is the one we store. If
        // an emitter changes while this fetch is in flight, the stored value
        // is the pre-change one, so the next lookup sees a mismatch and
        // reloads. Re-reading it afterwards could pin a snapshot to a version
        // whose change it never observed.
        let rows = EmitterRepo::list_match_rules(pool).await?;
        let base: Vec<ParsedRule> = rows
            .into_iter()
            .filter_map(|row| {
                // Malformed `match_criteria` makes that one emitter
                // non-matching rather than failing ingest for everyone — the
                // same tolerance the per-emission parse had.
                serde_json::from_value::<Rule>(row.match_criteria)
                    .ok()
                    .map(|rule| ParsedRule {
                        emitter_id: row.id,
                        rule,
                    })
            })
            .collect();

        let snapshot = Arc::new(Snapshot {
            version,
            base: Arc::new(base),
            appended: Vec::new(),
        });
        *guard = Some(snapshot.clone());
        Ok(snapshot)
    }
}
