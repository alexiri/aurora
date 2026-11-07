-- 03 §5 Agents, gateway credentials; plus deferred FKs to agents from earlier files.
-- gateway_credentials.conversation_id -> conversations is added by BE-005 (§6 file).

CREATE TABLE agents (
    id                   uuid PRIMARY KEY,
    org_id               uuid NOT NULL REFERENCES organizations (id) ON DELETE RESTRICT,
    workspace_id         uuid NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    handle               text NOT NULL,
    name                 text NOT NULL,
    kind                 text NOT NULL CHECK (kind IN ('byo', 'hosted')),
    is_default           boolean NOT NULL DEFAULT false,
    harness              text,
    model_provider       text,
    model_endpoint_id    text,
    model_id             text,
    proactive_messaging  text NOT NULL DEFAULT 'ask' CHECK (proactive_messaging IN ('ask', 'allow', 'off')),
    effort               text CHECK (effort IN ('low', 'medium', 'high', 'max')),
    brief                text NOT NULL DEFAULT '' CHECK (char_length(brief) <= 20000),
    avatar_blob_key      text,
    avatar_public_key    text,
    resource_overrides   jsonb,
    home_sync_generation bigint NOT NULL DEFAULT 0,
    created_by           uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at           timestamptz NOT NULL DEFAULT now(),
    updated_at           timestamptz NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, handle)
);
CREATE UNIQUE INDEX agents_one_default_per_workspace_uq ON agents (workspace_id) WHERE is_default;

CREATE TABLE gateway_credentials (
    id              uuid PRIMARY KEY,
    kind            text NOT NULL CHECK (kind IN ('agent', 'boot', 'context')),
    token_hash      bytea NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    display_prefix  text NOT NULL,
    agent_id        uuid NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    boot_id         uuid,
    conversation_id uuid, -- FK->conversations (cascade) added by BE-005
    turn_id         uuid,
    context_label   text CHECK (char_length(context_label) <= 128),
    expires_at      timestamptz,
    revoked_at      timestamptz,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX gateway_credentials_agent_kind_idx ON gateway_credentials (agent_id, kind);
-- Invariant: at most one non-revoked agent credential per agent.
CREATE UNIQUE INDEX gateway_credentials_one_agent_cred_uq ON gateway_credentials (agent_id) WHERE kind = 'agent' AND revoked_at IS NULL;

-- Deferred foreign keys to agents (tables created in earlier files).
ALTER TABLE secrets ADD CONSTRAINT secrets_proposed_by_agent_id_fkey
    FOREIGN KEY (proposed_by_agent_id) REFERENCES agents (id) ON DELETE SET NULL;
ALTER TABLE grants ADD CONSTRAINT grants_agent_id_fkey
    FOREIGN KEY (agent_id) REFERENCES agents (id) ON DELETE CASCADE;
ALTER TABLE action_approvals ADD CONSTRAINT action_approvals_agent_id_fkey
    FOREIGN KEY (agent_id) REFERENCES agents (id) ON DELETE CASCADE;
ALTER TABLE agent_links ADD CONSTRAINT agent_links_agent_a_id_fkey
    FOREIGN KEY (agent_a_id) REFERENCES agents (id) ON DELETE CASCADE;
ALTER TABLE agent_links ADD CONSTRAINT agent_links_agent_b_id_fkey
    FOREIGN KEY (agent_b_id) REFERENCES agents (id) ON DELETE CASCADE;
ALTER TABLE peer_tasks ADD CONSTRAINT peer_tasks_from_agent_id_fkey
    FOREIGN KEY (from_agent_id) REFERENCES agents (id) ON DELETE CASCADE;
ALTER TABLE peer_tasks ADD CONSTRAINT peer_tasks_to_agent_id_fkey
    FOREIGN KEY (to_agent_id) REFERENCES agents (id) ON DELETE CASCADE;
CREATE INDEX secrets_proposed_by_agent_idx ON secrets (proposed_by_agent_id);
CREATE INDEX grants_agent_id_idx ON grants (agent_id);
