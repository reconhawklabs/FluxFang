-- =====================================================================
-- MCP server provenance + audit (see
-- docs/superpowers/specs/2026-07-17-fluxfang-mcp-server-design.md).
--
-- Adds a `source` provenance tag to emitter/entity so AI-originated rows
-- are distinguishable ('manual' default, 'ai' for MCP writes), an
-- `ai_confidence` on entity, widens the emitter_association `source` CHECK
-- to allow 'ai' (drop-and-re-add pattern, as in 0005/0007/0011), and adds
-- the ai_audit_log table backing the AI Audit Log page.
-- =====================================================================

ALTER TABLE emitter
    ADD COLUMN source text NOT NULL DEFAULT 'manual'
        CHECK (source IN ('manual', 'ai'));

ALTER TABLE entity
    ADD COLUMN source text NOT NULL DEFAULT 'manual'
        CHECK (source IN ('manual', 'ai')),
    ADD COLUMN ai_confidence double precision;

ALTER TABLE emitter_association DROP CONSTRAINT emitter_association_source_check;
ALTER TABLE emitter_association
    ADD CONSTRAINT emitter_association_source_check
    CHECK (source IN ('manual', 'auto', 'ai'));

CREATE TABLE ai_audit_log (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    created_at    timestamptz NOT NULL DEFAULT now(),
    tool          text NOT NULL,
    action        text NOT NULL CHECK (action IN ('add', 'remove')),
    summary       text NOT NULL,
    args          jsonb NOT NULL DEFAULT '{}'::jsonb,
    result        jsonb,
    affected_ids  uuid[] NOT NULL DEFAULT '{}',
    status        text NOT NULL CHECK (status IN ('ok', 'error')),
    error         text
);

CREATE INDEX ai_audit_log_created_at_idx ON ai_audit_log (created_at DESC);
