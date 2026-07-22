-- =====================================================================
-- Back-compat for installs that completed first-run setup BEFORE the
-- node-role feature existed: their `app_config.settings` has no `role`, so
-- `AppConfigRepo::node_config` returns None and `GET /api/config` 404s —
-- which leaves the freshly-upgraded UI stuck on "Loading". Backfill those
-- rows as a Standalone node (what a pre-sensor install effectively was) so
-- an upgrade heals itself. Only touches setup-completed rows that lack a
-- role; the password hash and any other settings are left intact.
-- =====================================================================
UPDATE app_config
SET settings = settings || '{"role": "standalone", "node_sensor_id": "local"}'::jsonb
WHERE password_hash IS NOT NULL
  AND NOT (settings ? 'role');
