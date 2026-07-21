-- =====================================================================
-- Add the `sensor` data source kind (mode `listener`) — a network listener
-- that accepts connections from distributed Sensor nodes, rather than a
-- local capture device. See
-- docs/superpowers/specs/2026-07-21-distributed-sensor-nodes-design.md §6.
--
-- Same drop-and-re-add pattern as 0005/0007/0011. `emission_kind_check` is
-- intentionally NOT touched: a listener does not itself emit; remote
-- emissions are ingested through the normal pipeline in a later phase.
-- =====================================================================

ALTER TABLE data_source DROP CONSTRAINT data_source_kind_check;
ALTER TABLE data_source
    ADD CONSTRAINT data_source_kind_check
    CHECK (kind IN ('wifi', 'gps', 'bluetooth', 'rtl_sdr', 'sensor'));

ALTER TABLE data_source DROP CONSTRAINT data_source_kind_mode_check;
ALTER TABLE data_source
    ADD CONSTRAINT data_source_kind_mode_check
    CHECK (
        (kind = 'wifi' AND mode IN ('monitor', 'scan'))
        OR (kind = 'gps' AND mode IN ('gpsd', 'serial', 'manual'))
        OR (kind = 'bluetooth' AND mode = 'scan')
        OR (kind = 'rtl_sdr' AND mode = 'tpms')
        OR (kind = 'sensor' AND mode = 'listener')
    );
