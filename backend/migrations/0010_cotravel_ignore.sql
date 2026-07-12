-- Emitters the operator has chosen to hide from the Co-Travel Detection page.
-- Global + permanent: an ignored emitter is excluded from that page for every
-- time window, but remains fully visible on every other page. FK CASCADE so a
-- deleted emitter drops its ignore row too.
CREATE TABLE cotravel_ignore (
    emitter_id uuid PRIMARY KEY REFERENCES emitter(id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now()
);
