-- =====================================================================
-- Task: add the `bluetooth` data source kind (Scanning mode), alongside
-- the existing `wifi` and `gps` kinds. See
-- `docs/superpowers/specs/2026-07-06-bluetooth-scanning-datasource-design.md`.
-- =====================================================================
--
-- Two CHECK constraints gate `data_source.kind`/`mode`:
--   * `data_source_kind_check`      — column-level `kind IN ('wifi','gps')`
--                                     (from 0001_init.sql).
--   * `data_source_kind_mode_check` — table-level kind/mode pairing
--                                     (renamed/added in 0003_wifi_scan_mode.sql).
-- Replace both to admit `kind = 'bluetooth'` with `mode = 'scan'`.

ALTER TABLE data_source DROP CONSTRAINT data_source_kind_check;
ALTER TABLE data_source
    ADD CONSTRAINT data_source_kind_check
    CHECK (kind IN ('wifi', 'gps', 'bluetooth'));

ALTER TABLE data_source DROP CONSTRAINT data_source_kind_mode_check;
ALTER TABLE data_source
    ADD CONSTRAINT data_source_kind_mode_check
    CHECK (
        (kind = 'wifi' AND mode IN ('monitor', 'scan'))
        OR (kind = 'gps' AND mode IN ('gpsd', 'serial'))
        OR (kind = 'bluetooth' AND mode = 'scan')
    );
