-- =====================================================================
-- Sensor-node local cache: emissions captured on a Sensor node, awaiting
-- forwarding to the Standalone. `id` is generated here and becomes the
-- Standalone emission's primary key (so re-delivery de-dupes). See spec §4.5.
-- =====================================================================
CREATE TABLE cached_emission (
    id               uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    created_at       timestamptz NOT NULL DEFAULT now(),
    kind             text NOT NULL,
    signal_strength  int,
    lat              double precision,
    lon              double precision,
    observed_at      timestamptz NOT NULL,
    payload          jsonb NOT NULL DEFAULT '{}'::jsonb,
    data_source_id   uuid,
    delivered        boolean NOT NULL DEFAULT false
);

CREATE INDEX cached_emission_delivered_created_idx ON cached_emission (delivered, created_at);
