-- P4-SSH-2 (D-239, SEC-94, SEC-95; C-793, C-794): one row per authenticated SSH connection that a
-- terminator replica holds. The row is the API's decision record: the session open inserts it in one
-- transaction after re-verifying the certificate, the key, `can_ssh` and the caps; the terminator's
-- heartbeats keep `lease_until` current and the API answers them with revocations; the close is the
-- guarded transition `open -> closed` (`UPDATE ... WHERE state = 'open' RETURNING`), so exactly one
-- path (terminator close, heartbeat detection, stale-session sweep) writes the single
-- `agent.ssh.close` audit row. `client_address` is coarsened at insert (IPv4 /24, IPv6 /48) and is
-- audit metadata only; byte counts are totals, never content, never commands. `ssh_key_id` is set
-- null when the key row is deleted, which the next heartbeat revokes (`key_deleted`). The gateway
-- database role gets nothing. Closed rows are kept `SSH_SESSION_RETENTION` (30 days).
CREATE TABLE ssh_sessions (
    id                    uuid PRIMARY KEY,
    user_id               uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    ssh_key_id            uuid REFERENCES ssh_keys (id) ON DELETE SET NULL,
    key_fingerprint       text NOT NULL,
    cert_serial           bigint NOT NULL,
    org_id                uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    workspace_id          uuid NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    agent_id              uuid NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    -- The replica that holds the connection; no FK (replica rows are pruned).
    terminator_replica_id uuid NOT NULL,
    client_address        text,
    state                 text NOT NULL DEFAULT 'open' CHECK (state IN ('open', 'closed')),
    lease_until           timestamptz NOT NULL,
    last_activity_at      timestamptz,
    revoke_requested_at   timestamptz,
    revoke_reason         text,
    closed_at             timestamptz,
    close_reason          text,
    channels_total        int NOT NULL DEFAULT 0,
    bytes_in              bigint NOT NULL DEFAULT 0,
    bytes_out             bigint NOT NULL DEFAULT 0,
    created_at            timestamptz NOT NULL DEFAULT now(),
    updated_at            timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX ssh_sessions_user_open_idx ON ssh_sessions (user_id) WHERE state = 'open';
CREATE INDEX ssh_sessions_agent_open_idx ON ssh_sessions (agent_id) WHERE state = 'open';
CREATE INDEX ssh_sessions_lease_open_idx ON ssh_sessions (lease_until) WHERE state = 'open';
CREATE INDEX ssh_sessions_created_idx ON ssh_sessions (created_at);
