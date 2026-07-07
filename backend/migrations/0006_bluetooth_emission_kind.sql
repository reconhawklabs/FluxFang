-- =====================================================================
-- Task 3 follow-up: `emission.kind` has its own column-level CHECK
-- (`emission_kind_check`, from 0001_init.sql: `kind IN ('wifi')`),
-- separate from `data_source.kind`/`data_source_kind_mode_check` (widened
-- in 0005_bluetooth_datasource.sql). That migration only touched
-- `data_source`, so a bluetooth advertisement emission (`emission.kind =
-- 'bluetooth'`) is rejected by the DB even though a `bluetooth`/`scan`
-- data source is now accepted -- ingest would violate this constraint on
-- its first bluetooth insert. Widen it to admit 'bluetooth' alongside the
-- existing 'wifi'. See
-- `docs/superpowers/specs/2026-07-06-bluetooth-scanning-datasource-design.md`.
-- =====================================================================

ALTER TABLE emission DROP CONSTRAINT emission_kind_check;
ALTER TABLE emission
    ADD CONSTRAINT emission_kind_check
    CHECK (kind IN ('wifi', 'bluetooth'));
