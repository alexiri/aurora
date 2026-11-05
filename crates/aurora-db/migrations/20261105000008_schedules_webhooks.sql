-- 03 §8 Schedules, webhooks, processes, watches.
-- processes.sandbox_id -> sandboxes (cascade) is added in 0009 (sandboxes created there).

CREATE TABLE schedules (
    id                     uuid PRIMARY KEY,
    agent_id               uuid NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    name                   text NOT NULL,
    prompt                 text NOT NULL CHECK (char_length(prompt) <= 20000),
    cron                   text NOT NULL,
    timezone               text NOT NULL,
    gate_command           text CHECK (char_length(gate_command) <= 2000),
    gate_description       text CHECK (char_length(gate_description) <= 200),
    enabled                boolean NOT NULL DEFAULT true,
    disabled_reason        text CHECK (disabled_reason IN ('user', 'authorization', 'failures')),
    consecutive_failures   integer NOT NULL DEFAULT 0,
    creator_user_id        uuid REFERENCES users (id) ON DELETE SET NULL,
    origin_conversation_id uuid REFERENCES conversations (id) ON DELETE SET NULL,
    run_conversation_id    uuid REFERENCES conversations (id) ON DELETE SET NULL,
    next_fire_at           timestamptz,
    fire_lease_until       timestamptz, -- claim clock
    last_fire_at           timestamptz,
    last_outcome           text CHECK (last_outcome IN ('ok', 'failed', 'skipped_busy', 'gated_quiet')),
    created_at             timestamptz NOT NULL DEFAULT now(),
    updated_at             timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX schedules_agent_idx ON schedules (agent_id);
CREATE INDEX schedules_due_idx ON schedules (next_fire_at) WHERE enabled AND next_fire_at IS NOT NULL;
CREATE INDEX schedules_creator_idx ON schedules (creator_user_id) WHERE creator_user_id IS NOT NULL;
CREATE INDEX schedules_origin_conversation_idx ON schedules (origin_conversation_id) WHERE origin_conversation_id IS NOT NULL;
CREATE INDEX schedules_run_conversation_idx ON schedules (run_conversation_id) WHERE run_conversation_id IS NOT NULL;

-- Immutable run log: no updated_at.
CREATE TABLE schedule_runs (
    id          uuid PRIMARY KEY,
    schedule_id uuid NOT NULL REFERENCES schedules (id) ON DELETE CASCADE,
    turn_id     uuid,
    outcome     text NOT NULL,
    fired_at    timestamptz NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX schedule_runs_schedule_idx ON schedule_runs (schedule_id, fired_at DESC);

CREATE TABLE webhooks (
    id                     uuid PRIMARY KEY,
    agent_id               uuid NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    name                   text NOT NULL,
    instructions           text NOT NULL,
    token_hash             bytea NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    display_prefix         text NOT NULL,
    verification           jsonb,
    enabled                boolean NOT NULL DEFAULT true,
    disabled_reason        text CHECK (disabled_reason IN ('user', 'authorization', 'failures')),
    consecutive_failures   integer NOT NULL DEFAULT 0,
    creator_user_id        uuid REFERENCES users (id) ON DELETE SET NULL,
    origin_conversation_id uuid REFERENCES conversations (id) ON DELETE SET NULL,
    run_conversation_id    uuid REFERENCES conversations (id) ON DELETE SET NULL,
    created_at             timestamptz NOT NULL DEFAULT now(),
    updated_at             timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX webhooks_agent_idx ON webhooks (agent_id);
CREATE INDEX webhooks_creator_idx ON webhooks (creator_user_id) WHERE creator_user_id IS NOT NULL;
CREATE INDEX webhooks_origin_conversation_idx ON webhooks (origin_conversation_id) WHERE origin_conversation_id IS NOT NULL;
CREATE INDEX webhooks_run_conversation_idx ON webhooks (run_conversation_id) WHERE run_conversation_id IS NOT NULL;

-- Immutable delivery log: no updated_at.
CREATE TABLE webhook_deliveries (
    id         uuid PRIMARY KEY,
    webhook_id uuid NOT NULL REFERENCES webhooks (id) ON DELETE CASCADE,
    turn_id    uuid,
    status     text NOT NULL CHECK (status IN ('accepted', 'rejected', 'paused')),
    size_bytes integer NOT NULL,
    reason     text,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX webhook_deliveries_webhook_idx ON webhook_deliveries (webhook_id, created_at DESC);

CREATE TABLE processes (
    id             uuid PRIMARY KEY, -- supervisor-assigned
    agent_id       uuid NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    sandbox_id     uuid NOT NULL,    -- FK->sandboxes (cascade) added in 0009
    boot_id        uuid NOT NULL,
    name           text NOT NULL,
    command        text NOT NULL,
    status         text NOT NULL CHECK (status IN ('running', 'exited', 'lost', 'parked')),
    busy           boolean NOT NULL DEFAULT false, -- churn column
    exit_code      integer,
    signal         text,
    tail           text NOT NULL DEFAULT '', -- <= PROCESS_TAIL; repository enforces
    last_output_at timestamptz,              -- churn column
    started_at     timestamptz NOT NULL,
    ended_at       timestamptz,
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX processes_agent_idx ON processes (agent_id);
CREATE INDEX processes_sandbox_idx ON processes (sandbox_id);

CREATE TABLE watches (
    id              uuid PRIMARY KEY,
    process_id      uuid NOT NULL REFERENCES processes (id) ON DELETE CASCADE,
    agent_id        uuid NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    conversation_id uuid NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    kind            text NOT NULL CHECK (kind IN ('exit', 'output', 'silence')),
    pattern         text,
    silence_secs    integer,
    status          text NOT NULL CHECK (status IN ('armed', 'fired', 'lost', 'parked', 'expired', 'cancelled')),
    fired_turn_id   uuid,
    expires_at      timestamptz NOT NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX watches_process_idx ON watches (process_id);
CREATE INDEX watches_agent_idx ON watches (agent_id);
CREATE INDEX watches_conversation_idx ON watches (conversation_id);
CREATE INDEX watches_armed_expiry_idx ON watches (expires_at) WHERE status = 'armed';
