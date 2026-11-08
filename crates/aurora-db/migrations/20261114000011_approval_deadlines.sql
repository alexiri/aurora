-- P2-011A (RFE-011a, D-56): per-rule / per-ask-grant approval timeouts, the stall clock restart
-- (`approvals.resolved_at`), approval watchers (live delivery surface) and notification claims.
-- Additive: every new column is nullable or has a default; no existing row changes meaning.
ALTER TABLE policy_rules
    ADD COLUMN approval_timeout_secs integer NULL,
    ADD CONSTRAINT policy_rules_approval_timeout_ck CHECK (
        approval_timeout_secs IS NULL
        OR (require_approval AND approval_timeout_secs >= 60 AND approval_timeout_secs <= 14400)
    );

ALTER TABLE grants
    ADD COLUMN ask_timeout_secs integer NULL,
    ADD CONSTRAINT grants_ask_timeout_ck CHECK (
        ask_timeout_secs IS NULL
        OR (ask_timeout_secs >= 60 AND ask_timeout_secs <= 14400)
    );

-- The stall clock restarts here. Backfill terminal rows so the restart is defined for history.
ALTER TABLE approvals ADD COLUMN resolved_at timestamptz NULL;
UPDATE approvals SET resolved_at = COALESCE(decided_at, deadline) WHERE status <> 'pending';

-- The stall rule looks up the approvals of a turn (pending exemption, latest resolution).
CREATE INDEX approvals_turn_idx ON approvals (turn_id) WHERE turn_id IS NOT NULL;

ALTER TABLE users ADD COLUMN approval_email_enabled boolean NOT NULL DEFAULT true;

CREATE TABLE approval_watchers (
    replica_id uuid NOT NULL,
    user_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    workspace_id uuid NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    last_seen_at timestamptz NOT NULL,
    PRIMARY KEY (replica_id, user_id, workspace_id)
);
CREATE INDEX approval_watchers_ws_idx ON approval_watchers (workspace_id, last_seen_at);

CREATE TABLE approval_notifications (
    approval_id uuid NOT NULL,
    user_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    kind text NOT NULL CHECK (kind IN ('gateway', 'action')),
    sent_at timestamptz NOT NULL DEFAULT now(),
    -- how the claim was served: its own mail, folded into a batch mail, the "N more waiting" digest
    -- mail, or folded silently into an earlier digest (hourly cap, D-77)
    delivery text NOT NULL DEFAULT 'mail' CHECK (delivery IN ('mail', 'batch', 'digest_mail', 'digest')),
    PRIMARY KEY (approval_id, user_id)
);
CREATE INDEX approval_notifications_user_idx ON approval_notifications (user_id, sent_at);
