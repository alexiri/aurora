-- P4-SSH-2 (D-239, SEC-92; C-794): the terminator replicas that registered with the API. The API
-- serves the distinct `host_public_key`s of live replicas (`last_seen_at` newer than
-- `SSH_SESSION_LEASE`) as the known-hosts entries of `GET /v1/ssh`; rows older than 24 h are pruned.
-- There is no token table: the single anchor token's hash is held in memory
-- (`AURORA_BOOTSTRAP_TERMINATOR_TOKEN`). The gateway database role gets nothing.
CREATE TABLE ssh_terminator_replicas (
    id                   uuid PRIMARY KEY,
    version              text NOT NULL,
    -- Canonical `ssh-ed25519 <base64>`.
    host_public_key      text NOT NULL,
    host_key_fingerprint text NOT NULL,
    sessions_open        int NOT NULL DEFAULT 0,
    last_seen_at         timestamptz NOT NULL DEFAULT now(),
    created_at           timestamptz NOT NULL DEFAULT now(),
    updated_at           timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX ssh_terminator_replicas_seen_idx ON ssh_terminator_replicas (last_seen_at);
