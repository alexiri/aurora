-- 03 §4 Policy, grants, approvals.
-- Foreign keys to tables created later are added by ALTER TABLE in later files:
--   grants.agent_id, action_approvals.agent_id, agent_links.*, peer_tasks.from/to_agent_id -> agents: 0005_agents.sql
--   action_approvals.conversation_id, peer_tasks.origin/task_conversation_id -> conversations: BE-005 (§6 file)

CREATE TABLE policy_generations (
    id                       uuid PRIMARY KEY,
    org_id                   uuid NOT NULL REFERENCES organizations (id) ON DELETE RESTRICT,
    workspace_id             uuid REFERENCES workspaces (id) ON DELETE CASCADE,
    number                   integer NOT NULL,
    status                   text NOT NULL CHECK (status IN ('draft', 'published', 'superseded')),
    default_action           text NOT NULL DEFAULT 'none' CHECK (default_action IN ('none', 'allow', 'block')),
    uncredentialed_default   text NOT NULL DEFAULT 'none' CHECK (uncredentialed_default IN ('none', 'block')),
    uncredentialed_allowlist jsonb NOT NULL DEFAULT '[]'
        CONSTRAINT policy_generations_allowlist_ck CHECK (jsonb_typeof(uncredentialed_allowlist) = 'array' AND jsonb_array_length(uncredentialed_allowlist) <= 500),
    draft_revision           bigint NOT NULL DEFAULT 0,
    published_by             uuid REFERENCES users (id) ON DELETE SET NULL,
    published_at             timestamptz,
    created_at               timestamptz NOT NULL DEFAULT now(),
    updated_at               timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT policy_generations_number_uq UNIQUE NULLS NOT DISTINCT (org_id, workspace_id, number),
    CONSTRAINT policy_generations_uncredentialed_scope_ck CHECK (workspace_id IS NOT NULL OR uncredentialed_default = 'none')
);
-- NULLS NOT DISTINCT so the org scope (workspace_id IS NULL) is also limited to one row.
CREATE UNIQUE INDEX policy_generations_one_draft_uq ON policy_generations (org_id, workspace_id) NULLS NOT DISTINCT WHERE status = 'draft';
CREATE UNIQUE INDEX policy_generations_one_published_uq ON policy_generations (org_id, workspace_id) NULLS NOT DISTINCT WHERE status = 'published';

CREATE TABLE policy_rules (
    id               uuid PRIMARY KEY,
    generation_id    uuid NOT NULL REFERENCES policy_generations (id) ON DELETE CASCADE,
    logical_id       uuid NOT NULL,
    priority         integer NOT NULL,
    name             text NOT NULL,
    description      text,
    enabled          boolean NOT NULL DEFAULT true,
    identities       jsonb NOT NULL,
    targets          jsonb NOT NULL,
    action           text NOT NULL CHECK (action IN ('allow', 'block')),
    require_approval boolean NOT NULL DEFAULT false,
    rate_limit       jsonb,
    conditions       jsonb NOT NULL DEFAULT '[]',
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now(),
    UNIQUE (generation_id, priority),
    CONSTRAINT policy_rules_allow_only_ck CHECK (action = 'allow' OR (NOT require_approval AND rate_limit IS NULL))
);
CREATE INDEX policy_rules_logical_id_idx ON policy_rules (logical_id);

CREATE TABLE grants (
    id            uuid PRIMARY KEY,
    workspace_id  uuid NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    agent_id      uuid NOT NULL, -- FK->agents (cascade) added in 0005
    connection_id uuid REFERENCES app_connections (id) ON DELETE CASCADE,
    secret_id     uuid REFERENCES secrets (id) ON DELETE CASCADE,
    mode          text CHECK (mode IN ('full', 'custom')),
    allow_tools   text[] NOT NULL DEFAULT '{}',
    ask_tools     text[] NOT NULL DEFAULT '{}',
    created_by    uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    UNIQUE (agent_id, connection_id),
    UNIQUE (agent_id, secret_id),
    CONSTRAINT grants_exactly_one_target_ck CHECK ((connection_id IS NULL) <> (secret_id IS NULL)),
    CONSTRAINT grants_mode_connections_only_ck CHECK (secret_id IS NULL OR mode IS NULL)
);
CREATE INDEX grants_workspace_id_idx ON grants (workspace_id);
CREATE INDEX grants_connection_id_idx ON grants (connection_id);
CREATE INDEX grants_secret_id_idx ON grants (secret_id);

CREATE TABLE derived_rules (
    id           uuid PRIMARY KEY,
    org_id       uuid NOT NULL REFERENCES organizations (id) ON DELETE RESTRICT,
    workspace_id uuid REFERENCES workspaces (id) ON DELETE CASCADE,
    source_kind  text NOT NULL CHECK (source_kind IN ('grant', 'blocklist')),
    source_id    uuid NOT NULL,
    ordinal      integer NOT NULL,
    logical_id   uuid NOT NULL,
    rule         jsonb NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (source_kind, source_id, ordinal)
);
CREATE INDEX derived_rules_scope_idx ON derived_rules (org_id, workspace_id);

-- History survives agent deletion: no FK on agent (or any other column).
CREATE TABLE approvals (
    id                  uuid PRIMARY KEY,
    org_id              uuid NOT NULL,
    workspace_id        uuid NOT NULL,
    agent_id            uuid NOT NULL,
    conversation_id     uuid,
    turn_id             uuid,
    gateway_instance_id uuid NOT NULL,
    method              text NOT NULL,
    host                text NOT NULL,
    path                text NOT NULL,
    app_id              text,
    tool_id             text,
    connection_id       uuid,
    rule_logical_id     uuid,
    rule_name           text,
    summary             jsonb NOT NULL,
    batch_id            text CHECK (char_length(batch_id) <= 128),
    batch_label         text CHECK (char_length(batch_label) <= 128),
    batch_total         integer,
    status              text NOT NULL CHECK (status IN ('pending', 'approved', 'denied', 'expired', 'abandoned')),
    decided_by_user_id  uuid,
    decided_via         text CHECK (decided_via IN ('dashboard', 'api', 'channel')),
    decided_at          timestamptz,
    deadline            timestamptz NOT NULL,
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX approvals_workspace_status_idx ON approvals (workspace_id, status);

-- Ledger: no FKs, range-partitioned by month on ts. Immutable apart from insert.
-- Only the DEFAULT partition is created here; the API creates monthly partitions
-- (current + next 2) and drops expired ones (REQUEST_LOG_RETENTION) later.
-- Note: rows landing in the default partition block creating an overlapping
-- monthly partition until they are moved; partition maintenance must handle that.
CREATE TABLE request_log (
    id              uuid NOT NULL,
    ts              timestamptz NOT NULL,
    org_id          uuid NOT NULL,
    workspace_id    uuid NOT NULL,
    agent_id        uuid NOT NULL,
    conversation_id uuid,
    turn_id         uuid,
    credential_kind text NOT NULL CHECK (credential_kind IN ('agent', 'boot', 'context')),
    lane            text NOT NULL CHECK (lane IN ('mitm', 'open', 'db')),
    method          text NOT NULL,
    host            text NOT NULL,
    path            text NOT NULL,
    app_id          text,
    tool_id         text,
    connection_id   uuid,
    secret_id       uuid,
    decision        text NOT NULL CHECK (decision IN (
        'allow', 'blocked', 'default_denied', 'egress_denied', 'governance_refused',
        'rate_limited', 'approval_denied', 'approval_expired', 'approved', 'guard_refused', 'error')),
    rule_logical_id uuid,
    rule_name       text,
    upstream_status integer,
    status          integer NOT NULL,
    error_code      text CHECK (char_length(error_code) <= 64),
    injected        smallint NOT NULL DEFAULT 0,
    latency_ms      integer NOT NULL,
    bytes_up        bigint NOT NULL DEFAULT 0,
    bytes_down      bigint NOT NULL DEFAULT 0,
    created_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (id, ts)
) PARTITION BY RANGE (ts);
CREATE TABLE request_log_default PARTITION OF request_log DEFAULT;
CREATE INDEX request_log_workspace_ts_idx ON request_log (workspace_id, ts DESC);

-- 03: rate_limit_windows is reserved and not used in phase 0.

CREATE TABLE action_approvals (
    id                 uuid PRIMARY KEY,
    workspace_id       uuid NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    agent_id           uuid NOT NULL, -- FK->agents (cascade) added in 0005
    conversation_id    uuid,          -- FK->conversations (set null) added by BE-005
    turn_id            uuid,
    action_key         text NOT NULL,
    payload            jsonb NOT NULL,
    summary            text NOT NULL,
    status             text NOT NULL CHECK (status IN ('pending', 'approved', 'executed', 'failed', 'rejected', 'expired')),
    decided_by_user_id uuid,
    decided_at         timestamptz,
    executed_at        timestamptz,
    error              text,
    expires_at         timestamptz NOT NULL,
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX action_approvals_workspace_status_idx ON action_approvals (workspace_id, status);

CREATE TABLE agent_links (
    id         uuid PRIMARY KEY,
    agent_a_id uuid NOT NULL, -- FK->agents (cascade) added in 0005
    agent_b_id uuid NOT NULL, -- FK->agents (cascade) added in 0005
    policy_a   text NOT NULL CHECK (policy_a IN ('ask', 'allow', 'blocked')),
    policy_b   text NOT NULL CHECK (policy_b IN ('ask', 'allow', 'blocked')),
    revision   bigint NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (agent_a_id, agent_b_id),
    CONSTRAINT agent_links_ordered_ck CHECK (agent_a_id < agent_b_id)
);
CREATE INDEX agent_links_agent_b_idx ON agent_links (agent_b_id);

CREATE TABLE peer_tasks (
    id                    uuid PRIMARY KEY,
    link_id               uuid NOT NULL REFERENCES agent_links (id) ON DELETE CASCADE,
    from_agent_id         uuid NOT NULL, -- FK->agents (cascade) added in 0005
    to_agent_id           uuid NOT NULL, -- FK->agents (cascade) added in 0005
    requester_user_id     uuid REFERENCES users (id) ON DELETE SET NULL,
    origin_conversation_id uuid NOT NULL, -- FK->conversations (cascade) added by BE-005
    task_conversation_id  uuid NOT NULL,  -- FK->conversations (cascade) added by BE-005
    status                text NOT NULL CHECK (status IN ('queued', 'open', 'completed', 'cancelled', 'expired')),
    messages_used         integer NOT NULL DEFAULT 0,
    message_budget        integer NOT NULL,
    report                text,
    expires_at            timestamptz NOT NULL,
    created_at            timestamptz NOT NULL DEFAULT now(),
    updated_at            timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX peer_tasks_one_open_uq ON peer_tasks (from_agent_id, to_agent_id) WHERE status = 'open';
CREATE INDEX peer_tasks_link_id_idx ON peer_tasks (link_id);
