-- =====================================================================
-- Phase A1 (emitter auto-classification design, "Data model" §): add the
-- columns an ingest-time classifier/auto-create pipeline needs on
-- `emitter`, without touching anything that already exists.
-- =====================================================================
--
-- See `docs/superpowers/specs/2026-07-05-emitter-classification-design.md`
-- for the full design. This migration only adds the four new columns;
-- no ingest/API/classification logic lives here (later phases).
--
-- `emitter_type`  -- machine key (e.g. "wifi_access_point", "wifi_client");
--                    NULL for a plain user-made emitter. Deliberately a
--                    free `text` column (not a CHECK-constrained enum,
--                    matching this schema's stated convention in
--                    0001_init.sql of using plain text for fields that
--                    grow over time) since new device kinds (Bluetooth,
--                    TPMS sensors, ...) add new type keys later with no
--                    migration required.
-- `attributes`    -- type-specific identifying info + metadata (ssid,
--                    bssid, src_mac, randomized_mac, ...) as JSONB, same
--                    "flexible, not locked to a fixed column set" idiom
--                    `match_criteria`/`config`/`payload` already use
--                    elsewhere in this schema.
-- `match_enabled` -- lets an emitter's auto-attach rule be toggled off
--                    without deleting the emitter or its history.
-- `identity_key`  -- stable de-dup key for auto-create (e.g.
--                    "wifi_access_point:aa:bb:cc:dd:ee:ff"), NULL for
--                    user-made emitters.
ALTER TABLE emitter
    ADD COLUMN emitter_type   text,
    ADD COLUMN attributes     jsonb NOT NULL DEFAULT '{}'::jsonb,
    ADD COLUMN match_enabled  boolean NOT NULL DEFAULT true,
    ADD COLUMN identity_key   text;

-- A unique index on a nullable column allows any number of NULL rows in
-- Postgres (NULLs are never considered equal to each other for
-- uniqueness purposes) -- exactly what's wanted here: user-made emitters
-- all have `identity_key IS NULL` and never collide with each other, while
-- auto-created emitters (`identity_key IS NOT NULL`) are de-duplicated.
-- This is also what makes `EmitterRepo::get_or_create_by_identity`'s
-- `INSERT ... ON CONFLICT (identity_key) DO NOTHING` race-safe under
-- concurrent ingest.
CREATE UNIQUE INDEX emitter_identity_key_uidx ON emitter(identity_key);
