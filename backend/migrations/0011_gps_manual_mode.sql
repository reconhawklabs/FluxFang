-- =====================================================================
-- Add the `manual` GPS mode (operator-typed static lat/lon) alongside the
-- existing `gpsd` and `serial` gps modes. See
-- `docs/superpowers/specs/2026-07-16-gps-manual-mode-design.md`.
--
-- Only the table-level kind/mode pairing constraint widens; `gps` is
-- already an allowed `kind`, so `data_source_kind_check` is untouched.
-- Same drop-and-re-add pattern as 0005/0007.
-- =====================================================================

ALTER TABLE data_source DROP CONSTRAINT data_source_kind_mode_check;
ALTER TABLE data_source
    ADD CONSTRAINT data_source_kind_mode_check
    CHECK (
        (kind = 'wifi' AND mode IN ('monitor', 'scan'))
        OR (kind = 'gps' AND mode IN ('gpsd', 'serial', 'manual'))
        OR (kind = 'bluetooth' AND mode = 'scan')
        OR (kind = 'rtl_sdr' AND mode = 'tpms')
    );
