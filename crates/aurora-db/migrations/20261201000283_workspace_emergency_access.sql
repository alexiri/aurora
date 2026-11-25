-- FIX-236 (D-478, C-1164, SEC-154): emergency read access for organization owners — break-glass.
-- An org owner opens time-limited, read-only access to ONE workspace of their organization, with
-- a reason, so legal and incident work can read conversations after FIX-234 (D-477) took the
-- implicit role away. One row per (workspace, user) while active (the partial unique index):
-- a second open while one is active is 409 `emergency_access_active`. Active means
-- `ended_at IS NULL AND expires_at > now()`, always evaluated at use — never cached, and never
-- only by the sweep. Rows are kept 400 days for the history views, then the nightly sweep
-- deletes them; their audit rows stay under the audit's own retention. The gateway role gets
-- nothing on this table.
CREATE TABLE workspace_emergency_access (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    workspace_id uuid NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    user_id     uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- Why the access was opened; sanitized at the route, 10–500 characters.
    reason      text NOT NULL CHECK (char_length(reason) BETWEEN 10 AND 500),
    opened_at   timestamptz NOT NULL DEFAULT now(),
    expires_at  timestamptz NOT NULL,
    ended_at    timestamptz NULL,
    ended_by    uuid NULL REFERENCES users (id) ON DELETE SET NULL,
    -- `ended` = ended early by a person; `expired` = the sweep closed it; `access_lost` = the
    -- opener stopped being an active org owner (role lowered, suspended, removed) and the change
    -- ended the row in its own transaction.
    end_reason  text NULL CHECK (end_reason IN ('ended', 'expired', 'access_lost')),
    CONSTRAINT workspace_emergency_access_end_pair CHECK (
        (ended_at IS NULL AND end_reason IS NULL)
        OR (ended_at IS NOT NULL AND end_reason IS NOT NULL)
    ),
    CONSTRAINT workspace_emergency_access_window CHECK (expires_at > opened_at)
);
-- One active grant per (workspace, user): the open route relies on this to serialize two
-- owners opening at once (the loser of the race gets 409).
CREATE UNIQUE INDEX workspace_emergency_access_active_uq
    ON workspace_emergency_access (workspace_id, user_id)
    WHERE ended_at IS NULL;
CREATE INDEX workspace_emergency_access_org_idx ON workspace_emergency_access (org_id, opened_at DESC);
CREATE INDEX workspace_emergency_access_ws_idx ON workspace_emergency_access (workspace_id, opened_at DESC);
-- The 400-day retention sweep and the expiry job.
CREATE INDEX workspace_emergency_access_opened_idx ON workspace_emergency_access (opened_at);
CREATE INDEX workspace_emergency_access_active_idx ON workspace_emergency_access (expires_at)
    WHERE ended_at IS NULL;
