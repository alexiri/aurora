-- 03 §9 Runners and sandboxes; plus the deferred processes.sandbox_id FK from 0008.

CREATE TABLE runners (
    id                       uuid PRIMARY KEY,
    name                     text NOT NULL UNIQUE,
    token_hash               bytea NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    installation_fingerprint text NOT NULL,
    backend                  text,
    capabilities             jsonb,
    last_registered_at       timestamptz,
    created_at               timestamptz NOT NULL DEFAULT now(),
    updated_at               timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE runner_replicas (
    id              uuid PRIMARY KEY,
    runner_id       uuid NOT NULL REFERENCES runners (id) ON DELETE CASCADE,
    advertised_host text NOT NULL,
    version         text NOT NULL,
    started_at      timestamptz NOT NULL,
    last_seen_at    timestamptz NOT NULL, -- churn column; poll = heartbeat
    retired_at      timestamptz,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX runner_replicas_runner_idx ON runner_replicas (runner_id);

CREATE TABLE sandboxes (
    id                      uuid PRIMARY KEY,
    agent_id                uuid NOT NULL UNIQUE REFERENCES agents (id) ON DELETE CASCADE,
    runner_id               uuid REFERENCES runners (id) ON DELETE SET NULL,
    state                   text NOT NULL CHECK (state IN ('unprovisioned', 'starting', 'running', 'stopping', 'stopped', 'failed')),
    owner_replica_id        uuid,
    channel_replica_id      uuid,
    lifecycle_lease_until   timestamptz, -- claim clock for start/stop
    boot_id                 uuid,
    container_ref           text,
    home_ref                text,
    bootstrap_token_hash    bytea CHECK (octet_length(bootstrap_token_hash) = 32), -- single use
    bootstrap_used_at       timestamptz,
    stop_reason             text CHECK (stop_reason IN ('idle', 'cap', 'evict', 'restart', 'delete', 'failure')),
    stop_requested_at       timestamptz,
    restart_requested_at    timestamptz,
    last_engaged_at         timestamptz,
    last_active_at          timestamptz,
    home_applied_generation bigint NOT NULL DEFAULT 0,
    listening_ports         integer[] NOT NULL DEFAULT '{}', -- churn column
    failure_code            text,
    failure_message         text,
    resource_profile        jsonb,
    created_at              timestamptz NOT NULL DEFAULT now(),
    updated_at              timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX sandboxes_runner_idx ON sandboxes (runner_id) WHERE runner_id IS NOT NULL;
CREATE UNIQUE INDEX sandboxes_bootstrap_token_uq ON sandboxes (bootstrap_token_hash) WHERE bootstrap_token_hash IS NOT NULL;

ALTER TABLE processes ADD CONSTRAINT processes_sandbox_id_fkey
    FOREIGN KEY (sandbox_id) REFERENCES sandboxes (id) ON DELETE CASCADE;

-- Ledger: no FKs (outlives what it describes).
CREATE TABLE runtime_intervals (
    id           uuid PRIMARY KEY,
    org_id       uuid NOT NULL,
    workspace_id uuid NOT NULL,
    agent_id     uuid NOT NULL,
    sandbox_id   uuid NOT NULL,
    boot_id      uuid NOT NULL UNIQUE,
    started_at   timestamptz NOT NULL, -- control-plane clock
    ended_at     timestamptz,
    end_reason   text,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now()
);
-- At most one open interval per sandbox.
CREATE UNIQUE INDEX runtime_intervals_one_open_uq ON runtime_intervals (sandbox_id) WHERE ended_at IS NULL;
CREATE INDEX runtime_intervals_agent_idx ON runtime_intervals (agent_id, started_at);

-- Ledger: no FKs. Rebuildable from runtime_intervals.
CREATE TABLE runtime_daily (
    org_id        uuid NOT NULL,
    agent_id      uuid NOT NULL,
    day           date NOT NULL, -- UTC
    awake_seconds bigint NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, agent_id, day)
);
