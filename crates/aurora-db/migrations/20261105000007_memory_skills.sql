-- 03 §7 Memory and skills.
-- Content size caps (MEMORY_CONTENT_MAX) are enforced by the repository.
-- No tsvector column/index is specified by 03 §7; none is created here.

CREATE TABLE memories (
    id          uuid PRIMARY KEY,
    agent_id    uuid NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    key         text NOT NULL CHECK (char_length(key) <= 64 AND key ~ '^[a-z0-9]+(-[a-z0-9]+)*$'),
    title       text CHECK (char_length(title) <= 120),
    description text CHECK (char_length(description) <= 240),
    content     text NOT NULL,
    revision    integer NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    UNIQUE (agent_id, key)
);

-- Append-only apart from in-place redaction: no updated_at.
CREATE TABLE memory_revisions (
    id             uuid PRIMARY KEY,
    memory_id      uuid NOT NULL REFERENCES memories (id) ON DELETE CASCADE,
    revision       integer NOT NULL,
    op             text NOT NULL CHECK (op IN ('save', 'restore')),
    author_kind    text NOT NULL CHECK (author_kind IN ('user', 'agent')),
    author_user_id uuid,
    author_label   text NOT NULL,
    title          text,
    description    text,
    content        text NOT NULL,
    provenance     jsonb NOT NULL DEFAULT '{}'::jsonb, -- dangling ids ok: no FKs
    redacted_at    timestamptz,
    redacted_by    uuid,
    created_at     timestamptz NOT NULL DEFAULT now(),
    UNIQUE (memory_id, revision)
);

CREATE TABLE skills (
    id              uuid PRIMARY KEY,
    org_id          uuid NOT NULL REFERENCES organizations (id) ON DELETE RESTRICT,
    agent_id        uuid REFERENCES agents (id) ON DELETE CASCADE, -- null => org skill
    name            text NOT NULL CHECK (char_length(name) <= 64 AND name ~ '^[a-z0-9]+(-[a-z0-9]+)*$'),
    description     text NOT NULL CHECK (char_length(description) <= 500 AND description !~ E'[\\r\\n]'),
    body            text NOT NULL,
    enabled         boolean NOT NULL DEFAULT true,
    created_by_kind text NOT NULL CHECK (created_by_kind IN ('user', 'agent')),
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX skills_agent_name_uq ON skills (agent_id, name) WHERE agent_id IS NOT NULL;
CREATE UNIQUE INDEX skills_org_name_uq ON skills (org_id, name) WHERE agent_id IS NULL;

CREATE TABLE skill_files (
    skill_id   uuid NOT NULL REFERENCES skills (id) ON DELETE CASCADE,
    path       text NOT NULL,
    content    bytea NOT NULL,
    executable boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (skill_id, path)
);
