-- P5-UX-5 (D-325, D-328, SEC-128, C-919, C-920): a workspace owner gives a person access by e-mail before that person
-- is a member of the organization. The row waits; it is applied at a sign-in whose verified e-mail equals it, and only
-- once the person is an active member of the workspace's organization. It never makes anyone a member of the
-- organization and never carries the `owner` role. The gateway role gets nothing on this table.
CREATE TABLE workspace_pending_access (
    id               uuid PRIMARY KEY,
    workspace_id     uuid NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    -- The workspace's organization, denormalized for the sign-in lookup.
    org_id           uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    -- Normalized like `invitations.email` (trimmed, lower case); compared for equality only.
    email_normalized text NOT NULL,
    role             text NOT NULL CHECK (role IN ('member', 'approver')),
    invited_by       uuid REFERENCES users (id) ON DELETE SET NULL,
    expires_at       timestamptz NOT NULL,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT workspace_pending_access_email_uq UNIQUE (workspace_id, email_normalized)
);
CREATE INDEX workspace_pending_access_signin_idx ON workspace_pending_access (org_id, email_normalized);
CREATE INDEX workspace_pending_access_expires_idx ON workspace_pending_access (expires_at);
