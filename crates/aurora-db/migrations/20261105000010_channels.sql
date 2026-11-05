-- 03 §10 Channels. channel_inbox / channel_ops.connector_id and presence_id etc. are plain columns
-- per 03 (no FK stated); only the FKs listed in 03 are created.

CREATE TABLE connectors (
    id                       text PRIMARY KEY CHECK (id ~ '^[a-z][a-z0-9-]{1,31}$'),
    token_hash               bytea NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    display_name             text NOT NULL,
    version                  text,
    contract                 integer,
    manifest                 jsonb,
    manifest_digest          text,
    approved_manifest_digest text,
    status                   text NOT NULL CHECK (status IN ('awaiting_registration', 'pending_approval', 'active', 'disabled')),
    approved_by              uuid,
    approved_at              timestamptz,
    created_at               timestamptz NOT NULL DEFAULT now(),
    updated_at               timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE connector_replicas (
    id           uuid PRIMARY KEY,
    connector_id text NOT NULL REFERENCES connectors (id) ON DELETE CASCADE,
    version      text NOT NULL,
    last_seen_at timestamptz NOT NULL, -- churn column
    retired_at   timestamptz,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX connector_replicas_connector_idx ON connector_replicas (connector_id);

CREATE TABLE channel_integrations (
    id                  uuid PRIMARY KEY,
    org_id              uuid NOT NULL REFERENCES organizations (id) ON DELETE RESTRICT,
    connector_id        text NOT NULL REFERENCES connectors (id) ON DELETE RESTRICT,
    tenant_ref          text NOT NULL,
    tenant_label        text NOT NULL,
    config              jsonb NOT NULL,
    credentials_ref     text NOT NULL,
    credentials_version bigint NOT NULL DEFAULT 1,
    shared_bot          jsonb,
    status              text NOT NULL CHECK (status IN ('validating', 'active', 'needs_attention', 'disabled')),
    status_reason       text,
    owner_replica_id    uuid, -- churn column
    owner_lease_until   timestamptz, -- churn column
    created_by          uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    UNIQUE (connector_id, tenant_ref)
);
CREATE INDEX channel_integrations_org_idx ON channel_integrations (org_id);
CREATE INDEX channel_integrations_created_by_idx ON channel_integrations (created_by) WHERE created_by IS NOT NULL;

CREATE TABLE channel_presences (
    id                  uuid PRIMARY KEY,
    agent_id            uuid NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    integration_id      uuid NOT NULL REFERENCES channel_integrations (id) ON DELETE CASCADE,
    mode                text NOT NULL CHECK (mode IN ('dedicated', 'shared')),
    platform_user_ref   text,
    handle              text,
    credentials_ref     text,
    credentials_version bigint NOT NULL DEFAULT 1,
    status              text NOT NULL CHECK (status IN ('pending_setup', 'active', 'disabled', 'needs_attention')),
    status_reason       text,
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    UNIQUE (agent_id, integration_id),
    UNIQUE (integration_id, platform_user_ref)
);

CREATE TABLE space_bindings (
    id                     uuid PRIMARY KEY,
    integration_id         uuid NOT NULL REFERENCES channel_integrations (id) ON DELETE CASCADE,
    space_ref              text NOT NULL,
    space_label            text NOT NULL,
    agent_id               uuid NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    listen                 text NOT NULL DEFAULT 'mentions' CHECK (listen IN ('mentions', 'all')),
    followups_need_mention boolean NOT NULL DEFAULT false,
    active                 boolean NOT NULL DEFAULT true,
    bound_by_user_id       uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at             timestamptz NOT NULL DEFAULT now(),
    updated_at             timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX space_bindings_active_space_uq ON space_bindings (integration_id, space_ref) WHERE active;
CREATE INDEX space_bindings_agent_idx ON space_bindings (agent_id);
CREATE INDEX space_bindings_bound_by_idx ON space_bindings (bound_by_user_id) WHERE bound_by_user_id IS NOT NULL;

CREATE TABLE dm_defaults (
    integration_id    uuid NOT NULL REFERENCES channel_integrations (id) ON DELETE CASCADE,
    platform_user_ref text NOT NULL,
    agent_id          uuid NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (integration_id, platform_user_ref)
);
CREATE INDEX dm_defaults_agent_idx ON dm_defaults (agent_id);

CREATE TABLE user_links (
    id                uuid PRIMARY KEY,
    integration_id    uuid NOT NULL REFERENCES channel_integrations (id) ON DELETE CASCADE,
    platform_user_ref text NOT NULL,
    user_id           uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    method            text NOT NULL CHECK (method IN ('verified_email', 'sign_in_link', 'admin')),
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now(),
    UNIQUE (integration_id, platform_user_ref)
);
CREATE INDEX user_links_user_idx ON user_links (user_id);

CREATE TABLE link_requests (
    id                uuid PRIMARY KEY,
    integration_id    uuid NOT NULL REFERENCES channel_integrations (id) ON DELETE CASCADE,
    platform_user_ref text NOT NULL,
    token_hash        bytea NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    expires_at        timestamptz NOT NULL, -- 30 min
    used_at           timestamptz,
    user_id           uuid,
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX link_requests_integration_idx ON link_requests (integration_id);

CREATE TABLE thread_links (
    id              uuid PRIMARY KEY,
    integration_id  uuid NOT NULL REFERENCES channel_integrations (id) ON DELETE CASCADE,
    agent_id        uuid NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    conversation_id uuid NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    kind            text NOT NULL CHECK (kind IN ('direct', 'group')),
    space_ref       text NOT NULL,
    thread_ref      text NOT NULL, -- '' when the platform has no threads
    sender          jsonb NOT NULL,
    revision        bigint NOT NULL DEFAULT 0,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    UNIQUE (integration_id, agent_id, space_ref, thread_ref)
);
CREATE INDEX thread_links_agent_idx ON thread_links (agent_id);
CREATE INDEX thread_links_conversation_idx ON thread_links (conversation_id);

CREATE TABLE reach_grants (
    id                 uuid PRIMARY KEY,
    agent_id           uuid NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    integration_id     uuid NOT NULL REFERENCES channel_integrations (id) ON DELETE CASCADE,
    subject_kind       text NOT NULL CHECK (subject_kind IN ('space', 'person')),
    subject_ref        text NOT NULL,
    subject_label      text NOT NULL,
    state              text NOT NULL CHECK (state IN ('pending', 'approved', 'blocked', 'left', 'expired')),
    listen             text NOT NULL DEFAULT 'mentions' CHECK (listen IN ('mentions', 'all')),
    revision           bigint NOT NULL DEFAULT 0,
    decided_by_user_id uuid,
    decided_at         timestamptz,
    expires_at         timestamptz,
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now(),
    UNIQUE (agent_id, integration_id, subject_kind, subject_ref)
);
CREATE INDEX reach_grants_integration_idx ON reach_grants (integration_id);

CREATE TABLE mention_anchors (
    conversation_id uuid NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    alias           text NOT NULL, -- normalized
    integration_id  uuid NOT NULL REFERENCES channel_integrations (id) ON DELETE CASCADE,
    kind            text NOT NULL CHECK (kind IN ('user', 'space', 'bot')),
    ref             text NOT NULL,
    label           text NOT NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (conversation_id, alias)
);
CREATE INDEX mention_anchors_integration_idx ON mention_anchors (integration_id);

-- Work queue; plain id columns (no FKs in 03).
CREATE TABLE channel_inbox (
    id              uuid PRIMARY KEY,
    connector_id    text NOT NULL,
    integration_id  uuid,
    event_id        text NOT NULL,
    kind            text NOT NULL,
    payload         jsonb NOT NULL,
    status          text NOT NULL CHECK (status IN ('pending', 'processing', 'done', 'failed', 'expired')),
    attempts        integer NOT NULL DEFAULT 0,
    next_attempt_at timestamptz NOT NULL,
    lease_until     timestamptz, -- claim clock
    outcome         jsonb,
    error           text,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    UNIQUE (connector_id, event_id)
);
CREATE INDEX channel_inbox_claim_idx ON channel_inbox (status, next_attempt_at);

CREATE TABLE channel_ops (
    id                 uuid PRIMARY KEY,
    connector_id       text NOT NULL,
    integration_id     uuid REFERENCES channel_integrations (id) ON DELETE CASCADE,
    presence_id        uuid,
    kind               text NOT NULL,
    payload            jsonb NOT NULL,
    dedupe_key         text UNIQUE,
    correlation_id     uuid,
    status             text NOT NULL CHECK (status IN ('pending', 'claimed', 'done', 'failed', 'dropped')),
    attempts           integer NOT NULL DEFAULT 0,
    not_before         timestamptz NOT NULL DEFAULT now(),
    lease_until        timestamptz, -- claim clock
    claimed_by_replica uuid,
    result             jsonb,
    error_code         text,
    completed_at       timestamptz,
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX channel_ops_claim_idx ON channel_ops (connector_id, status, not_before);
CREATE INDEX channel_ops_integration_idx ON channel_ops (integration_id) WHERE integration_id IS NOT NULL;

CREATE TABLE channel_cards (
    id             uuid PRIMARY KEY,
    subject_kind   text NOT NULL CHECK (subject_kind IN ('approval', 'action_approval', 'reach', 'binding', 'link', 'agent_link', 'connection_request')),
    subject_id     uuid NOT NULL,
    integration_id uuid NOT NULL REFERENCES channel_integrations (id) ON DELETE CASCADE,
    post_op_id     uuid NOT NULL,
    space_ref      text NOT NULL,
    thread_ref     text NOT NULL,
    message_ref    text,
    state          text NOT NULL CHECK (state IN ('posting', 'posted', 'settled', 'failed')),
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX channel_cards_subject_idx ON channel_cards (subject_kind, subject_id);
CREATE INDEX channel_cards_integration_idx ON channel_cards (integration_id);

-- Single-use ledger; pruned after expiry. Immutable: no updated_at.
CREATE TABLE action_token_uses (
    jti        uuid PRIMARY KEY,
    used_at    timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX action_token_uses_expires_idx ON action_token_uses (expires_at);
