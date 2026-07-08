-- =====================================================================
-- Add the `rtl_sdr` data source kind (first mode: `tpms`), alongside
-- `wifi`, `gps`, and `bluetooth`. See
-- `docs/superpowers/specs/2026-07-07-rtl-sdr-tpms-datasource-design.md`.
--
-- Three CHECK constraints must widen (same pattern as 0005/0006 for
-- bluetooth):
--   * data_source_kind_check      — column-level valid `kind` set.
--   * data_source_kind_mode_check — table-level kind/mode pairing.
--   * emission_kind_check         — separate emission.kind set; without
--                                   this the first `tpms` emission insert
--                                   would violate the constraint.
-- =====================================================================

ALTER TABLE data_source DROP CONSTRAINT data_source_kind_check;
ALTER TABLE data_source
    ADD CONSTRAINT data_source_kind_check
    CHECK (kind IN ('wifi', 'gps', 'bluetooth', 'rtl_sdr'));

ALTER TABLE data_source DROP CONSTRAINT data_source_kind_mode_check;
ALTER TABLE data_source
    ADD CONSTRAINT data_source_kind_mode_check
    CHECK (
        (kind = 'wifi' AND mode IN ('monitor', 'scan'))
        OR (kind = 'gps' AND mode IN ('gpsd', 'serial'))
        OR (kind = 'bluetooth' AND mode = 'scan')
        OR (kind = 'rtl_sdr' AND mode = 'tpms')
    );

ALTER TABLE emission DROP CONSTRAINT emission_kind_check;
ALTER TABLE emission
    ADD CONSTRAINT emission_kind_check
    CHECK (kind IN ('wifi', 'bluetooth', 'tpms'));
