-- =====================================================================
-- A change token for the emitter auto-attach rule set.
--
-- Auto-attach (`finalize_emission`) used to re-read every `emitter` row and
-- re-parse every `match_criteria` for EACH ingested emission. That cost is
-- linear in the emitter table, so ingest slowed down as a node discovered
-- more devices -- measured at ~24ms/emission with 500 emitters and
-- ~262ms/emission with 10,000. A Sensor forwarding a 200-emission batch to a
-- Standalone that knew ~9,000 emitters therefore needed ~49s per batch,
-- outran the forwarder's HTTP timeout, never received an ACK, and retried the
-- same rows forever while its local cache grew without bound.
--
-- The backend now keeps the parsed rule set in memory. This counter is how it
-- knows when that snapshot is stale. Readers do one indexed single-row SELECT
-- and rebuild only when the number changed.
--
-- Why a DB trigger rather than invalidating from the write paths: matching is
-- mutated from many places (the emitters API, MCP write/subtraction tools,
-- auto-create during ingest, the ephemeral age-out sweep) and more will be
-- added. A trigger cannot be forgotten by a future caller, and it also covers
-- rows changed by hand in psql during an incident.
-- =====================================================================

CREATE TABLE emitter_match_version (
    -- Single-row table: the CHECK plus the PK make a second row impossible,
    -- so readers can `SELECT version FROM emitter_match_version` with no
    -- WHERE clause and always get the one true value.
    only_row boolean PRIMARY KEY DEFAULT true CHECK (only_row),
    version  bigint  NOT NULL DEFAULT 1
);

INSERT INTO emitter_match_version (only_row, version) VALUES (true, 1);

-- ---------------------------------------------------------------------
-- The bump must be *exact in both directions*.
--
-- Missing a real change would serve a stale rule set indefinitely -- the one
-- outcome that is actually incorrect.
--
-- But bumping when nothing changed is nearly as damaging here, just for
-- performance instead of correctness: ingest auto-creates emitters as it
-- discovers devices, and its `INSERT ... ON CONFLICT (identity_key) DO
-- NOTHING` runs on every emission that failed to auto-attach. A plain
-- statement-level trigger fires even when that insert affects zero rows, so
-- in a busy environment the cache would be invalidated by emissions that
-- changed nothing, and the per-emission full rebuild this migration exists to
-- remove would come straight back.
--
-- Hence transition tables (`REFERENCING ... TABLE AS`, PostgreSQL 10+): the
-- trigger sees exactly which rows the statement touched and bumps only if
-- there were any. Still FOR EACH STATEMENT, so deleting 9,000 emitters is one
-- bump rather than 9,000.
-- ---------------------------------------------------------------------

CREATE FUNCTION bump_emitter_match_version_on_insert() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM inserted) THEN
        UPDATE emitter_match_version SET version = version + 1;
    END IF;
    RETURN NULL;  -- AFTER ... FOR EACH STATEMENT ignores the return value.
END;
$$;

CREATE FUNCTION bump_emitter_match_version_on_delete() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM deleted) THEN
        UPDATE emitter_match_version SET version = version + 1;
    END IF;
    RETURN NULL;
END;
$$;

CREATE FUNCTION bump_emitter_match_version_always() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    UPDATE emitter_match_version SET version = version + 1;
    RETURN NULL;
END;
$$;

-- The bump is a normal transactional UPDATE, so the new version becomes
-- visible at exactly the moment the emitter change itself does. A sequence
-- would be cheaper but is non-transactional: a reader could observe the new
-- version before the not-yet-committed emitter row, cache a snapshot missing
-- that emitter, and never be told to rebuild again.

CREATE TRIGGER emitter_match_version_insert
    AFTER INSERT ON emitter
    REFERENCING NEW TABLE AS inserted
    FOR EACH STATEMENT
    EXECUTE FUNCTION bump_emitter_match_version_on_insert();

CREATE TRIGGER emitter_match_version_delete
    AFTER DELETE ON emitter
    REFERENCING OLD TABLE AS deleted
    FOR EACH STATEMENT
    EXECUTE FUNCTION bump_emitter_match_version_on_delete();

-- UPDATE is the one case that uses a column list instead of a transition
-- table: PostgreSQL forbids combining the two ("transition tables cannot be
-- specified for triggers with column lists"), and here the column list is
-- worth far more.
--
-- It keeps this trigger off the hot path entirely. The per-emission emitter
-- writes -- `touch_seen` on first/last_seen_at, `merge_*_attributes`,
-- `set_estimate` -- name none of these columns, so they never fire it at all.
-- A transition-table version would instead run on every one of them just to
-- discover nothing changed.
--
-- What that costs: a statement that names one of these columns bumps even if
-- it writes an identical value (e.g. re-enabling an already-enabled emitter).
-- Only `update_rule` and `set_match_enabled` do that -- rare operator
-- actions -- and the penalty is one extra reload.
CREATE TRIGGER emitter_match_version_update
    AFTER UPDATE OF match_criteria, match_enabled ON emitter
    FOR EACH STATEMENT
    EXECUTE FUNCTION bump_emitter_match_version_always();

-- TRUNCATE bypasses row-level DELETE triggers and supports no transition
-- tables, so it gets an unconditional bump of its own.
CREATE TRIGGER emitter_match_version_truncate
    AFTER TRUNCATE ON emitter
    FOR EACH STATEMENT
    EXECUTE FUNCTION bump_emitter_match_version_always();
