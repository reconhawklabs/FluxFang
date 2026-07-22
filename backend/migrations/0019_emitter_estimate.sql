-- RSSI-based emitter localization: a periodic Standalone pass estimates each
-- emitter's real-world position from the GPS location + signal_strength of its
-- emissions (see docs/superpowers/specs/2026-07-22-emitter-rssi-localization).
-- All nullable: null est_location = not yet localizable (too few distinct
-- observation locations) -> the map falls back to the latest-emission marker.
ALTER TABLE emitter
    ADD COLUMN est_location      geography(Point, 4326),
    ADD COLUMN est_uncertainty_m double precision,
    ADD COLUMN est_bin_count     integer,
    ADD COLUMN est_updated_at    timestamptz;
