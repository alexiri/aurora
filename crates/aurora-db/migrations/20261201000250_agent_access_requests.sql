-- P5-UX-17 (D-348, C-984, SEC-132): an agent's request for access, as a record.
--
-- `request_app_connection` and `propose_custom_connection` create or refresh one open row per agent and
-- app (or per agent and proposed website) next to the conversation notice they already write. A row gives
-- no access; it is resolved (`given`) in the transaction of a real access change, dismissed by a person
-- who can own the workspace, or expired after `ACCESS_REQUEST_TTL`. The gateway role gets nothing on it.
CREATE TABLE agent_access_requests (
    id              uuid PRIMARY KEY,
    org_id          uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    workspace_id    uuid NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    agent_id        uuid NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    conversation_id uuid NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    turn_id         uuid NULL REFERENCES turns (id) ON DELETE SET NULL,
    kind            text NOT NULL CHECK (kind IN ('app', 'website')),
    app_id          text NULL,
    secret_id       uuid NULL REFERENCES secrets (id) ON DELETE CASCADE,
    reason          text NOT NULL,
    status          text NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'given', 'dismissed', 'expired')),
    resolved_at     timestamptz NULL,
    resolved_by     uuid NULL REFERENCES users (id) ON DELETE SET NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT agent_access_requests_target CHECK (
        (kind = 'app' AND app_id IS NOT NULL AND secret_id IS NULL)
        OR (kind = 'website' AND secret_id IS NOT NULL)
    )
);

CREATE UNIQUE INDEX agent_access_requests_open_app
    ON agent_access_requests (agent_id, app_id) WHERE status = 'open' AND kind = 'app';
CREATE UNIQUE INDEX agent_access_requests_open_website
    ON agent_access_requests (agent_id, secret_id) WHERE status = 'open' AND kind = 'website';
CREATE INDEX agent_access_requests_workspace_open
    ON agent_access_requests (workspace_id, created_at DESC) WHERE status = 'open';
