-- =====================================================================
-- Decouple location tagging from GPS presence, and split datasource
-- "what the user wants" from "actual runtime state".
-- See docs/superpowers/specs/2026-07-09-location-datasource-decouple-design.md
--
--   * emission.location_quality — why an emission's location is what it
--     is: 'fresh' (real coords attached), 'stale' (a fix existed but was
--     too old/low-quality — location NULL), 'none' (no fix at all — NULL).
--   * data_source.desired_state — user intent ('running' | 'stopped'),
--     which the reconciler drives actual `status` toward and retries on
--     failure. Split from `status` (actual runtime state).
--   * data_source.last_ok_at — last time the source was confirmed healthy,
--     for the UI health column's "down for Ns".
-- =====================================================================

ALTER TABLE emission
    ADD COLUMN location_quality text NOT NULL DEFAULT 'none'
        CHECK (location_quality IN ('fresh', 'stale', 'none'));

-- Backfill: existing geolocated emissions were tagged from a live fix.
UPDATE emission
    SET location_quality = 'fresh'
    WHERE location IS NOT NULL;

ALTER TABLE data_source
    ADD COLUMN desired_state text NOT NULL DEFAULT 'stopped'
        CHECK (desired_state IN ('running', 'stopped'));

ALTER TABLE data_source
    ADD COLUMN last_ok_at timestamptz;

-- Backfill: a source persisted as 'running' was one the user wanted up.
UPDATE data_source
    SET desired_state = 'running'
    WHERE status = 'running';
