-- 03 §2 Secrets, LLM keys, connections, OAuth clients.
-- secrets.proposed_by_agent_id -> agents is added by ALTER TABLE in 20261105000005_agents.sql
-- (agents is created in a later file).

CREATE TABLE secrets (
    id                   uuid PRIMARY KEY,
    org_id               uuid NOT NULL REFERENCES organizations (id) ON DELETE RESTRICT,
    workspace_id         uuid REFERENCES workspaces (id) ON DELETE CASCADE,
    name                 text NOT NULL,
    kind                 text NOT NULL CHECK (kind IN ('generic', 'llm')),
    llm_provider         text,
    llm_auth_mode        text,
    llm_endpoint_id      text,
    host_pattern         text NOT NULL,
    path_pattern         text,
    injection            jsonb NOT NULL,
    value_source         text NOT NULL CHECK (value_source IN ('managed', 'reference')),
    value_ref            text NOT NULL,
    status               text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'proposed', 'rejected')),
    origin               jsonb,
    value_version        integer NOT NULL DEFAULT 1,
    proposed_by_agent_id uuid, -- FK->agents (set null) added in 0005
    created_by           uuid REFERENCES users (id) ON DELETE SET NULL,
    last_used_at         timestamptz, -- churn column
    created_at           timestamptz NOT NULL DEFAULT now(),
    updated_at           timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT secrets_scope_name_uq UNIQUE NULLS NOT DISTINCT (org_id, workspace_id, name),
    CONSTRAINT secrets_llm_provider_ck CHECK ((kind = 'llm') = (llm_provider IS NOT NULL)),
    CONSTRAINT secrets_origin_llm_only_ck CHECK (origin IS NULL OR kind = 'llm')
);

-- Created before app_connections because app_connections references it.
CREATE TABLE app_oauth_clients (
    id           uuid PRIMARY KEY,
    org_id       uuid REFERENCES organizations (id) ON DELETE RESTRICT,
    workspace_id uuid REFERENCES workspaces (id) ON DELETE CASCADE,
    package_id   text NOT NULL,
    client_key   text NOT NULL,
    fields       jsonb NOT NULL,
    secret_ref   text NOT NULL,
    enabled      boolean NOT NULL DEFAULT true,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT app_oauth_clients_scope_uq UNIQUE NULLS NOT DISTINCT (org_id, workspace_id, package_id, client_key),
    CONSTRAINT app_oauth_clients_workspace_requires_org_ck CHECK (workspace_id IS NULL OR org_id IS NOT NULL)
);

CREATE TABLE app_connections (
    id                     uuid PRIMARY KEY,
    org_id                 uuid NOT NULL REFERENCES organizations (id) ON DELETE RESTRICT,
    workspace_id           uuid REFERENCES workspaces (id) ON DELETE CASCADE,
    app_id                 text NOT NULL,
    package_id             text NOT NULL,
    app_version            text NOT NULL,
    label                  text NOT NULL,
    status                 text NOT NULL CHECK (status IN ('active', 'reauth_required', 'disabled', 'orphaned')),
    status_reason          text CHECK (char_length(status_reason) <= 64),
    credentials_ref        text NOT NULL,
    credentials_version    bigint NOT NULL DEFAULT 1,
    credentials_expires_at timestamptz,
    scopes                 text[] NOT NULL DEFAULT '{}',
    metadata               jsonb NOT NULL DEFAULT '{}',
    tenant_host            text,
    oauth_client_id        uuid REFERENCES app_oauth_clients (id) ON DELETE RESTRICT,
    source_connection_id   uuid REFERENCES app_connections (id) ON DELETE CASCADE,
    connected_by           uuid REFERENCES users (id) ON DELETE SET NULL,
    reauth_required_at     timestamptz,
    last_used_at           timestamptz, -- churn column
    created_at             timestamptz NOT NULL DEFAULT now(),
    updated_at             timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX app_connections_scope_app_idx ON app_connections (org_id, workspace_id, app_id);
