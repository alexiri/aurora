-- P4-PRV-1 (D-144, D-145, SEC-80, C-540..C-549): port previews — shares, wake, on-demand start.
-- A preview share binds one TCP port of one hosted agent to a DNS label (`p` + 24 base32 chars).
-- Shares are only meaningful for hosted agents (service-enforced: BYO ⇒ `preview_not_supported`).

CREATE TABLE preview_shares (
    id            uuid PRIMARY KEY,
    org_id        uuid NOT NULL REFERENCES organizations (id) ON DELETE RESTRICT,
    workspace_id  uuid NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    agent_id      uuid NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    port          integer NOT NULL CHECK (port BETWEEN 1024 AND 65535),
    -- The hostname label: `p` + 24 base32 lowercase chars (120 bits of CSPRNG), immutable,
    -- never reused: a deleted share's label is never minted again.
    label         citext NOT NULL,
    visibility    text NOT NULL DEFAULT 'private' CHECK (visibility IN ('private')),
    state         text NOT NULL DEFAULT 'active' CHECK (state IN ('active', 'paused')),
    -- The on-demand start command. Never audited, never sent to the relay; the supervisor holds
    -- it in memory only (home sync part) and runs it inside the sandbox.
    start_command text CHECK (char_length(start_command) <= 1024 AND start_command NOT LIKE '%' || chr(10) || '%'),
    cwd           text CHECK (char_length(cwd) <= 200 AND cwd NOT LIKE '%..%'),
    last_start_at         timestamptz,
    last_start_exit_code  integer,
    last_start_tail       text,
    -- Timestamps of the attempts inside their windows (bounded arrays; see docs/11).
    start_attempts        jsonb NOT NULL DEFAULT '[]'::jsonb,
    wake_attempts         jsonb NOT NULL DEFAULT '[]'::jsonb,
    created_by_user_id    uuid REFERENCES users (id) ON DELETE SET NULL,
    created_by_label      text,
    -- Written at most once a minute per share (relay heartbeat, P4-PRV-2).
    last_accessed_at      timestamptz,
    created_at            timestamptz NOT NULL DEFAULT now(),
    updated_at            timestamptz NOT NULL DEFAULT now(),
    -- At most one share per (agent, port); labels are globally unique and never reused.
    UNIQUE (agent_id, port)
);

CREATE UNIQUE INDEX preview_shares_label_uq ON preview_shares (label);
CREATE INDEX preview_shares_org_idx ON preview_shares (org_id);
CREATE INDEX preview_shares_workspace_idx ON preview_shares (workspace_id);

-- Sandbox churn columns (docs/03 `sandboxes`): the preview wake demand and keep-awake clocks.
-- Written by the API (resolve/wake) and the relay plane (P4-PRV-2); read by the start arm and
-- the keep-awake predicate. `preview_awake_since` is reset at every start claim.
ALTER TABLE sandboxes
    ADD COLUMN preview_wake_at             timestamptz,
    ADD COLUMN preview_start_requested_at  timestamptz,
    ADD COLUMN preview_active_until        timestamptz,
    ADD COLUMN preview_awake_since         timestamptz;
