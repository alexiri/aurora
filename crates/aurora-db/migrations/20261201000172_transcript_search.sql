-- P5-CHAT-4 part A (D-229, C-727): message search index. One row per searchable durable event
-- (`turn.queued`, `text`, `attachment`), written in the transaction that appends the event,
-- deleted by redaction and by the cascade of a conversation delete. No Postgres extension:
-- `doc` combines the workspace language's stemming (weight A) with the `simple` configuration
-- (weight B, exact words in any language). Created empty (no rewrite or long lock of
-- `transcript_events`); a resumable background job fills it for existing events.
CREATE TABLE transcript_search (
    conversation_id uuid        NOT NULL,
    seq             bigint      NOT NULL,
    workspace_id    uuid        NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    agent_id        uuid        NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    type            text        NOT NULL CHECK (type IN ('turn.queued', 'text', 'attachment')),
    author_kind     text        NOT NULL CHECK (author_kind IN ('agent', 'person', 'other')),
    author_label    text        NOT NULL DEFAULT '',
    speaker_user_id uuid,
    has_file        boolean     NOT NULL DEFAULT false,
    at              timestamptz NOT NULL,
    body            text        NOT NULL CHECK (octet_length(body) <= 32768),
    doc             tsvector    NOT NULL,
    PRIMARY KEY (conversation_id, seq),
    FOREIGN KEY (conversation_id, seq) REFERENCES transcript_events (conversation_id, seq) ON DELETE CASCADE
);
CREATE INDEX transcript_search_doc_idx ON transcript_search USING gin (doc);
CREATE INDEX transcript_search_workspace_at_idx ON transcript_search (workspace_id, at DESC);
CREATE INDEX transcript_search_agent_at_idx ON transcript_search (agent_id, at DESC);

-- Backfill progress (one row): the last `(conversation_id, seq)` indexed, in key order.
CREATE TABLE transcript_search_backfill (
    singleton           boolean     PRIMARY KEY DEFAULT true CHECK (singleton),
    cursor_conversation uuid,
    cursor_seq          bigint,
    complete            boolean     NOT NULL DEFAULT false,
    indexed             bigint      NOT NULL DEFAULT 0,
    updated_at          timestamptz NOT NULL DEFAULT now()
);
INSERT INTO transcript_search_backfill (singleton) VALUES (true);
