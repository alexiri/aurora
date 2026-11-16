-- P4-SSH-2 (D-239(f), SEC-95; C-794, C-796): single-use SSH channel grants. The terminator asks the
-- API for one per channel; the runner redeems it on :4108 (`kind: "ssh"`). A table of its own, not
-- rows in `terminal_stream_grants` or `file_stream_grants`: the `auz_` prefix routes redemption to
-- this table alone, so a terminal or files grant can never open an SSH channel or the reverse, and
-- the binding includes the session whose liveness redemption re-checks (open and lease-current).
-- The grant value is never stored (hashed only). The gateway database role gets nothing.
CREATE TABLE ssh_stream_grants (
    id          uuid PRIMARY KEY,
    session_id  uuid NOT NULL REFERENCES ssh_sessions (id) ON DELETE CASCADE,
    agent_id    uuid NOT NULL,
    sandbox_id  uuid NOT NULL,
    boot_id     uuid NOT NULL,
    -- SHA-256 of the `auz_` grant; the value is never stored.
    grant_hash  text NOT NULL,
    -- The channel mode the grant was minted for (`shell`, `exec`, `sftp`), kept for the audit
    -- trail of channel counts. Not content.
    mode        text NOT NULL CHECK (mode IN ('shell', 'exec', 'sftp')),
    -- now() + SSH_GRANT_TTL (30 s); pruned after 24 h.
    expires_at  timestamptz NOT NULL,
    -- Set by the single-use compare-and-set at redemption.
    consumed_at timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX ssh_stream_grants_grant_hash_uq ON ssh_stream_grants (grant_hash);
CREATE INDEX ssh_stream_grants_expires_idx ON ssh_stream_grants (expires_at);
CREATE INDEX ssh_stream_grants_session_idx ON ssh_stream_grants (session_id);
