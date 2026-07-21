-- =====================================================================
-- The `sensor` keyring: one row per enrolled/pending distributed Sensor
-- node reporting to a `sensor` (listener) datasource. See
-- docs/superpowers/specs/2026-07-21-distributed-sensor-nodes-design.md §4.4.
-- =====================================================================
CREATE TABLE sensor (
    id                  uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    created_at          timestamptz NOT NULL DEFAULT now(),
    data_source_id      uuid NOT NULL REFERENCES data_source(id) ON DELETE CASCADE,
    sensor_id           text NOT NULL,
    key                 text NOT NULL,               -- base64 symmetric key
    fingerprint         text NOT NULL,
    status              text NOT NULL DEFAULT 'pending'
                            CHECK (status IN ('pending', 'approved', 'revoked', 'rejected')),
    auto_group_emitters boolean NOT NULL DEFAULT true,
    source_ip           text,
    approved_at         timestamptz,
    last_seen_at        timestamptz,
    UNIQUE (data_source_id, sensor_id)
);

CREATE INDEX sensor_data_source_idx ON sensor (data_source_id);
