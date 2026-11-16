-- P5-FILES-1 (D-316, SEC-125; C-901): the Files page's SFTP sessions. The API is an SFTP client
-- of the sandbox image's `sftp-server`, transported over the runner's :4108 listener with a
-- single-use `auf_` grant bound to (agent, sandbox, boot, person) (`16 §W21`). The grant value is
-- never stored (hashed only); no foreign key except the person (the rows die young). A table of
-- its own, not rows in `terminal_stream_grants`: the `auf_` prefix routes redemption to this
-- table alone, so a files grant can only ever open an `sftp` exec. File contents are never stored
-- anywhere; this table holds the open's binding only. The gateway database role gets nothing.
CREATE TABLE file_stream_grants (
    id          uuid PRIMARY KEY,
    agent_id    uuid NOT NULL,
    sandbox_id  uuid NOT NULL,
    boot_id     uuid NOT NULL,
    -- The person whose request opened the session (the audit actor).
    granted_for uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- SHA-256 of the `auf_` grant; the value is never stored.
    grant_hash  text NOT NULL,
    -- now() + FILES_GRANT_TTL (30 s); pruned after 24 h.
    expires_at  timestamptz NOT NULL,
    -- Set by the single-use compare-and-set at redemption.
    consumed_at timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX file_stream_grants_grant_hash_uq ON file_stream_grants (grant_hash);
CREATE INDEX file_stream_grants_expires_idx ON file_stream_grants (expires_at);
