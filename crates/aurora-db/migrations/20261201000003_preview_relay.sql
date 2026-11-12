-- P4-PRV-2 (D-146, D-147, SEC-81, SEC-82, SEC-85; C-550..C-559): preview access and the relay
-- plane. The browser credential (`auv_`, cookie `__Host-aurora_preview`) lives hashed in
-- `preview_sessions`; grants are single-use ledgers; the relay authenticates with `aul_`
-- (`relay_tokens`) and registers replicas (`relay_replicas`). The gateway database role gets
-- nothing on any of these tables (the gateway has no part in previews).

-- The preview credential: bound to one share and one dashboard session; deleted by cascade when
-- the share, the user, the dashboard session or the agent/workspace/org goes (`03 §preview_sessions`).
CREATE TABLE preview_sessions (
    id             uuid PRIMARY KEY,
    -- SHA-256 of the `auv_` value; the value is returned once to the relay and never stored.
    token_hash     bytea NOT NULL,
    -- First 12 characters, for support.
    display_prefix text NOT NULL,
    share_id       uuid NOT NULL REFERENCES preview_shares (id) ON DELETE CASCADE,
    user_id        uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- The dashboard session that authorized it: signing out or revoking it deletes this row.
    session_id     uuid NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    created_at     timestamptz NOT NULL DEFAULT now(),
    -- Advanced at most once a minute; idle past PREVIEW_SESSION_IDLE is dead.
    last_seen_at   timestamptz NOT NULL DEFAULT now(),
    -- created_at + PREVIEW_SESSION_TTL, capped by the dashboard session's own absolute expiry.
    expires_at     timestamptz NOT NULL
);
CREATE UNIQUE INDEX preview_sessions_token_hash_uq ON preview_sessions (token_hash);
CREATE INDEX preview_sessions_share_idx ON preview_sessions (share_id);
CREATE INDEX preview_sessions_session_idx ON preview_sessions (session_id);
CREATE INDEX preview_sessions_user_idx ON preview_sessions (user_id);
CREATE INDEX preview_sessions_expires_idx ON preview_sessions (expires_at);

-- Single-use ledger of browser authorization grants: `jti` insert-on-conflict claims a sealed
-- grant exactly once (a replayed grant fails with 400 `grant_invalid`, docs/04 §Preview access).
CREATE TABLE preview_grant_uses (
    jti        uuid PRIMARY KEY,
    -- Grant expiry + 60 s; pruned by the preview housekeeping job.
    expires_at timestamptz NOT NULL
);
CREATE INDEX preview_grant_uses_expires_idx ON preview_grant_uses (expires_at);

-- Single-use stream grants (`aux_`, ≤ 30 s, redeemed only by a runner at
-- `/v1/runner/preview-streams/open`): one compare-and-set consumes them. The binding (share,
-- sandbox, boot, port) is copied at mint time; no foreign keys (the rows die young).
CREATE TABLE preview_stream_grants (
    token_hash  bytea PRIMARY KEY,
    share_id    uuid NOT NULL,
    sandbox_id  uuid NOT NULL,
    boot_id     uuid NOT NULL,
    port        integer NOT NULL,
    -- The preview session the grant was minted for (accounting only).
    session_hash bytea NOT NULL,
    -- Grant expiry + 60 s; pruned by the preview housekeeping job.
    expires_at  timestamptz NOT NULL,
    consumed_at timestamptz
);
CREATE INDEX preview_stream_grants_expires_idx ON preview_stream_grants (expires_at);

-- The relay's anchor token `aul_…`, from `AURORA_BOOTSTRAP_RELAY_TOKEN` (same shape as
-- `connector_host_tokens`); the chart's secrets hook generates it into the instance Secret.
CREATE TABLE relay_tokens (
    id         uuid PRIMARY KEY,
    token_hash bytea NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX relay_tokens_token_hash_uq ON relay_tokens (token_hash);

-- One row per registered relay replica (docs/14 Admin → Overview "Previews" line). The
-- heartbeat (`POST /v1/relay/report`) refreshes `last_seen_at`; rows older than 24 h are pruned.
CREATE TABLE relay_replicas (
    id            uuid PRIMARY KEY,
    token_id      uuid NOT NULL REFERENCES relay_tokens (id) ON DELETE CASCADE,
    version       text NOT NULL,
    streams_open  integer NOT NULL DEFAULT 0,
    last_seen_at  timestamptz NOT NULL DEFAULT now(),
    created_at    timestamptz NOT NULL DEFAULT now()
);
