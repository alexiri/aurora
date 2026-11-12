-- P4-TERM (D-161, SEC-87; C-616..C-625): the web terminal. A terminal session is a PTY in the
-- sandbox transported over the preview-stream machinery (`16 §W19` on `16 §W18`): the API mints
-- a single-use `auy_` grant bound to (agent, sandbox, boot), redeems it at the runner's :4108
-- listener and proxies the leg-1 WebSocket itself. The grant value is never stored (hashed
-- only, like `preview_stream_grants`); no foreign keys (the rows die young); terminal bytes are
-- never stored anywhere — this table holds the open's metadata only. The gateway database role
-- gets nothing on this table.

-- Single-use web-terminal grants (`auy_`, ≤ 30 s, redeemed only by the API at the runner's
-- `/preview-streams/open` with `kind: "terminal"`): one compare-and-set consumes them.
CREATE TABLE terminal_stream_grants (
    id          uuid PRIMARY KEY,
    -- The terminal's agent; the workspace authority (`can_use`) was checked at mint.
    agent_id    uuid NOT NULL,
    -- The bound sandbox and boot; a stale boot is refused at redemption.
    sandbox_id  uuid NOT NULL,
    boot_id     uuid NOT NULL,
    -- The user whose dashboard session opened the tab (the audit actor).
    granted_by  uuid NOT NULL,
    -- SHA-256 of the `auy_` grant; the value is never stored.
    grant_hash  text NOT NULL,
    -- now() + TERMINAL_GRANT_TTL (30 s); pruned after 24 h.
    expires_at  timestamptz NOT NULL,
    -- Set by the single-use compare-and-set at redemption.
    consumed_at timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX terminal_stream_grants_grant_hash_uq ON terminal_stream_grants (grant_hash);
CREATE INDEX terminal_stream_grants_expires_idx ON terminal_stream_grants (expires_at);

-- The terminal keep-awake clock and its per-boot cap clock (docs/03 `sandboxes`, P4-TERM
-- C-620): written by the runner's keep-awake stamps while a session carries bytes; read by the
-- start arm and the keep-awake predicate. `terminal_awake_since` is reset at every start claim
-- (the same shape as the preview columns above).
ALTER TABLE sandboxes
    ADD COLUMN terminal_active_until       timestamptz,
    ADD COLUMN terminal_awake_since        timestamptz,
    -- (P4-TERM, C-621) the wake demand: a terminal open on a parked sandbox, due work for
    -- `TERMINAL_WAKE_WINDOW` (the same shape as `preview_wake_at`).
    ADD COLUMN terminal_wake_at            timestamptz;
