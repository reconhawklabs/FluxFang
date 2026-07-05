-- =====================================================================
-- Task: add a `scan` mode for `wifi` data sources, alongside the
-- existing `monitor` mode (managed-mode `iw dev <if> scan` polling
-- instead of monitor-mode packet capture -- see
-- `fluxfang-capture/src/wifi/scan.rs`).
-- =====================================================================
--
-- `0001_init.sql` created `data_source`'s kind/mode combination as an
-- *unnamed* table-level CHECK constraint:
--
--   CHECK ((kind = 'wifi' AND mode = 'monitor') OR (kind = 'gps' AND mode IN ('gpsd', 'serial')))
--
-- Postgres auto-names an unnamed table-level CHECK constraint
-- `<table>_check` (confirmed against a scratch table reproducing
-- 0001's exact DDL: the column-level `kind IN (...)` check became
-- `data_source_kind_check`, and this multi-column one became plain
-- `data_source_check`). Drop it by that name and replace it with an
-- explicitly-named constraint (so any *future* migration touching this
-- rule doesn't have to repeat this same "what did Postgres call it"
-- exercise).
ALTER TABLE data_source DROP CONSTRAINT data_source_check;

ALTER TABLE data_source
    ADD CONSTRAINT data_source_kind_mode_check
    CHECK (
        (kind = 'wifi' AND mode IN ('monitor', 'scan'))
        OR (kind = 'gps' AND mode IN ('gpsd', 'serial'))
    );
