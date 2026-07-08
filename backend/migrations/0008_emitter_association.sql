-- =====================================================================
-- emitter_association: bidirectional emitter<->emitter links ("Other Tires
-- on the same Car"), modeled on the alert_rule_method join table
-- (0001_init.sql). Every association is stored as TWO rows (a->b and b->a);
-- the repo writes/deletes both in one transaction. `source` distinguishes a
-- user-made link ('manual') from one inferred by the TPMS correlation engine
-- ('auto'); `confidence` is set for auto links, NULL for manual. See
-- docs/superpowers/specs/2026-07-07-tpms-associations-and-correlation-design.md
-- =====================================================================

CREATE TABLE emitter_association (
    emitter_id            uuid NOT NULL REFERENCES emitter(id) ON DELETE CASCADE,
    associated_emitter_id uuid NOT NULL REFERENCES emitter(id) ON DELETE CASCADE,
    source                text NOT NULL CHECK (source IN ('manual', 'auto')),
    confidence            double precision,
    created_at            timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (emitter_id, associated_emitter_id),
    CHECK (emitter_id <> associated_emitter_id)
);

-- The PK covers lookups by emitter_id; this index covers the reverse
-- direction (find rows pointing AT an emitter) for cascade/cleanup queries.
CREATE INDEX emitter_association_assoc_idx
    ON emitter_association (associated_emitter_id);
