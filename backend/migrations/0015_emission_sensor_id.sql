-- =====================================================================
-- Tag every emission with the id of the sensor that captured it. Local
-- captures use the node's own `node_sensor_id` (default 'local'); remote
-- captures use the originating distributed Sensor's id. See spec §4.2/§8.
-- =====================================================================
ALTER TABLE emission
    ADD COLUMN sensor_id text NOT NULL DEFAULT 'local';

CREATE INDEX emission_sensor_id_observed_at_idx ON emission (sensor_id, observed_at);
