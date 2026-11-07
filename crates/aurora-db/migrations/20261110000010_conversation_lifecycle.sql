-- RFE-010 (phase 2): conversation archive, transcript redaction, harness session purges.
-- Additive only: nullable columns and one new table; no existing data is rewritten.

ALTER TABLE conversations
    ADD COLUMN archived_at timestamptz,
    ADD COLUMN archived_by_user_id uuid REFERENCES users (id) ON DELETE SET NULL;
CREATE INDEX conversations_agent_archived_idx ON conversations (agent_id, archived_at)
    WHERE archived_at IS NOT NULL;

ALTER TABLE transcript_events
    ADD COLUMN redacted_at timestamptz,
    ADD COLUMN redacted_by_user_id uuid REFERENCES users (id) ON DELETE SET NULL;

CREATE TABLE session_purges (
    id              uuid PRIMARY KEY,
    agent_id        uuid NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    conversation_id uuid, -- the conversation that is gone (no FK)
    session_ref     text NOT NULL,
    leased_until    timestamptz, -- live-delivery lease (the item is re-sent after it lapses)
    created_at      timestamptz NOT NULL DEFAULT now(),
    done_at         timestamptz
);
CREATE INDEX session_purges_pending_idx ON session_purges (agent_id) WHERE done_at IS NULL;
