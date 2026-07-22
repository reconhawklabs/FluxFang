-- Track WHEN a cached emission was forwarded, so a Sensor's Dashboard can show
-- a "delivered in the last hour" throughput metric. Nullable: existing rows and
-- undelivered rows have no delivery time.
ALTER TABLE cached_emission ADD COLUMN delivered_at timestamptz;

-- Supports the "delivered since T" count.
CREATE INDEX cached_emission_delivered_at_idx ON cached_emission (delivered_at)
    WHERE delivered = true;
