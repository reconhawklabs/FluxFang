CREATE EXTENSION IF NOT EXISTS postgis;
CREATE EXTENSION IF NOT EXISTS pgcrypto;

-- =====================================================================
-- Task 1.2: Core schema (all tables per design spec §4 "Data model")
-- =====================================================================
--
-- Conventions:
--   * Every table has `id UUID PRIMARY KEY DEFAULT gen_random_uuid()` and
--     `created_at timestamptz NOT NULL DEFAULT now()`.
--   * Small enumerated text fields use CHECK constraints (not native
--     Postgres ENUM types) so new values can be added later with a plain
--     ALTER TABLE ... DROP/ADD CONSTRAINT instead of ALTER TYPE plumbing.
--   * All geographic locations are `geography(Point,4326)`.

-- ---------------------------------------------------------------------
-- app_config: single-row application configuration.
-- ---------------------------------------------------------------------
CREATE TABLE app_config (
    id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    created_at      timestamptz NOT NULL DEFAULT now(),
    password_hash   text,                 -- argon2; null = first run not completed
    session_secret  text,
    settings        jsonb NOT NULL DEFAULT '{}'::jsonb
);

-- ---------------------------------------------------------------------
-- data_source: a configured capture device (wifi or gps).
-- ---------------------------------------------------------------------
CREATE TABLE data_source (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    created_at  timestamptz NOT NULL DEFAULT now(),
    kind        text NOT NULL CHECK (kind IN ('wifi', 'gps')),
    mode        text NOT NULL, -- 'monitor' (wifi); 'gpsd' | 'serial' (gps)
    interface   text,          -- wifi: e.g. wlan0
    status      text NOT NULL CHECK (status IN ('stopped', 'starting', 'running', 'error')),
    config      jsonb NOT NULL DEFAULT '{}'::jsonb,
    last_error  text
);

-- ---------------------------------------------------------------------
-- survey_session: bounds a continuous capture period ("outing"/"trip").
-- ---------------------------------------------------------------------
CREATE TABLE survey_session (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    created_at  timestamptz NOT NULL DEFAULT now(),
    started_at  timestamptz NOT NULL,
    ended_at    timestamptz,        -- null = active
    label       text
);

-- ---------------------------------------------------------------------
-- location_fix: continuous log of the host's own position over time.
-- ---------------------------------------------------------------------
CREATE TABLE location_fix (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    created_at   timestamptz NOT NULL DEFAULT now(),
    session_id   uuid NOT NULL REFERENCES survey_session(id) ON DELETE CASCADE,
    observed_at  timestamptz NOT NULL,
    location     geography(Point, 4326) NOT NULL,
    altitude     double precision,
    speed        double precision,
    heading      double precision,
    fix_quality  text
);

-- ---------------------------------------------------------------------
-- entity: the tracked real-world thing. Owns many emitters.
-- ---------------------------------------------------------------------
CREATE TABLE entity (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    created_at  timestamptz NOT NULL DEFAULT now(),
    name        text NOT NULL,
    notes       text
);

-- ---------------------------------------------------------------------
-- emitter: a distinct identified source (e.g. a specific access point).
-- ---------------------------------------------------------------------
CREATE TABLE emitter (
    id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    created_at      timestamptz NOT NULL DEFAULT now(),
    name            text NOT NULL,
    type            text,        -- user-labeled string, e.g. "Access Point"
    entity_id       uuid REFERENCES entity(id) ON DELETE SET NULL,
    match_criteria  jsonb NOT NULL DEFAULT '{}'::jsonb,
    first_seen_at   timestamptz,
    last_seen_at    timestamptz
);

-- ---------------------------------------------------------------------
-- emission: one captured observation. High-volume, time-indexed.
-- ---------------------------------------------------------------------
CREATE TABLE emission (
    id               uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    created_at       timestamptz NOT NULL DEFAULT now(),
    data_source_id   uuid NOT NULL REFERENCES data_source(id) ON DELETE CASCADE,
    emitter_id       uuid REFERENCES emitter(id) ON DELETE SET NULL,
    session_id       uuid REFERENCES survey_session(id) ON DELETE SET NULL,
    observed_at      timestamptz NOT NULL,
    signal_strength  int,                      -- RSSI dBm, nullable
    location         geography(Point, 4326),   -- nullable
    kind             text NOT NULL CHECK (kind IN ('wifi')),
    payload          jsonb NOT NULL DEFAULT '{}'::jsonb
);

CREATE INDEX emission_emitter_id_observed_at_idx ON emission(emitter_id, observed_at);
CREATE INDEX emission_location_gist_idx ON emission USING gist(location);
CREATE INDEX emission_observed_at_idx ON emission(observed_at);
CREATE INDEX emission_payload_gin_idx ON emission USING gin(payload jsonb_path_ops);

-- ---------------------------------------------------------------------
-- zone: a user-named geofence. Membership is computed, not stored, from
-- ST_DWithin(location, center, radius_m) — see zone_membership below for
-- the *transition-tracking* state used by alert rules.
-- ---------------------------------------------------------------------
CREATE TABLE zone (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    created_at  timestamptz NOT NULL DEFAULT now(),
    name        text NOT NULL,
    center      geography(Point, 4326) NOT NULL,
    radius_m    double precision NOT NULL,
    notes       text
);

CREATE INDEX zone_center_gist_idx ON zone USING gist(center);

-- ---------------------------------------------------------------------
-- zone_membership: ingest-maintained last-known-membership state so
-- enter/leave alert triggers fire once per transition, not per emission.
-- subject_type = 'host' has subject_id = null (there is only one host).
-- ---------------------------------------------------------------------
CREATE TABLE zone_membership (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    created_at    timestamptz NOT NULL DEFAULT now(),
    subject_type  text NOT NULL CHECK (subject_type IN ('emitter', 'entity', 'host')),
    subject_id    uuid,
    zone_id       uuid NOT NULL REFERENCES zone(id) ON DELETE CASCADE,
    inside        boolean NOT NULL,
    since         timestamptz NOT NULL
);

CREATE UNIQUE INDEX zone_membership_subject_zone_uidx
    ON zone_membership(subject_type, subject_id, zone_id);

-- ---------------------------------------------------------------------
-- alert_method: a reusable, user-configured delivery channel.
-- Non-secret config (e.g. webhook url/headers, smtp host/port/from/to,
-- tls flag) lives in `config`; anything secret (smtp password, webhook
-- secret) is encrypted at rest and stored in `config_encrypted` (Phase 8
-- wires up the actual encryption/decryption; this task only creates the
-- column as an opaque ciphertext blob).
-- ---------------------------------------------------------------------
CREATE TABLE alert_method (
    id                uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    created_at        timestamptz NOT NULL DEFAULT now(),
    name              text NOT NULL,
    type              text NOT NULL CHECK (type IN ('email', 'in_app', 'webhook')),
    enabled           boolean NOT NULL DEFAULT true,
    config            jsonb NOT NULL DEFAULT '{}'::jsonb,
    config_encrypted  bytea
);

-- ---------------------------------------------------------------------
-- alert_rule: watches a target (emitter/entity) or, for host-zone rules,
-- no target at all (target_type/target_id null).
-- ---------------------------------------------------------------------
CREATE TABLE alert_rule (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    created_at   timestamptz NOT NULL DEFAULT now(),
    name         text NOT NULL,
    enabled      boolean NOT NULL DEFAULT true,
    target_type  text CHECK (target_type IN ('emitter', 'entity')),
    target_id    uuid,   -- null for host-zone rules; no FK (polymorphic on target_type)
    trigger      jsonb NOT NULL
    -- trigger.on ∈ 'detected' | 'enters_zone' | 'leaves_zone'
    --            | 'host_enters_zone' | 'host_leaves_zone'
    -- trigger.zone_id (optional), trigger.content_match (optional)
);

-- ---------------------------------------------------------------------
-- alert_rule_method: join table — which alert_method(s) deliver a rule.
-- ---------------------------------------------------------------------
CREATE TABLE alert_rule_method (
    alert_rule_id    uuid NOT NULL REFERENCES alert_rule(id) ON DELETE CASCADE,
    alert_method_id  uuid NOT NULL REFERENCES alert_method(id) ON DELETE CASCADE,
    PRIMARY KEY (alert_rule_id, alert_method_id)
);

-- ---------------------------------------------------------------------
-- notification: fired-alert log; also the source for the in-app
-- Notifications page (read_at nullable = unread).
-- ---------------------------------------------------------------------
CREATE TABLE notification (
    id               uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    created_at       timestamptz NOT NULL DEFAULT now(),
    alert_rule_id    uuid REFERENCES alert_rule(id) ON DELETE CASCADE,
    alert_method_id  uuid REFERENCES alert_method(id) ON DELETE SET NULL,
    fired_at         timestamptz NOT NULL DEFAULT now(),
    payload          jsonb NOT NULL DEFAULT '{}'::jsonb,
    delivery_status  text NOT NULL CHECK (delivery_status IN ('pending', 'sent', 'failed')),
    read_at          timestamptz
);
