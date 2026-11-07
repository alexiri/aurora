-- 03 §1 Identity and tenancy.
-- updated_at has no trigger: the repository layer maintains it (03 §Conventions).

CREATE TABLE organizations (
    id                  uuid PRIMARY KEY,
    slug                text NOT NULL UNIQUE,
    name                text NOT NULL CHECK (char_length(name) <= 80),
    app_posture         text NOT NULL DEFAULT 'open' CHECK (app_posture IN ('open', 'restricted')),
    governance          jsonb NOT NULL DEFAULT '{}',
    governance_revision bigint NOT NULL DEFAULT 0,
    external_id         text NOT NULL UNIQUE,
    settings            jsonb NOT NULL DEFAULT '{}',
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE users (
    id                uuid PRIMARY KEY,
    email             citext NOT NULL UNIQUE,
    email_verified_at timestamptz,
    display_name      text NOT NULL CHECK (char_length(display_name) <= 80),
    password_hash     text,
    is_instance_admin boolean NOT NULL DEFAULT false,
    disabled_at       timestamptz,
    default_org_id    uuid REFERENCES organizations (id) ON DELETE SET NULL,
    last_login_at     timestamptz, -- churn column: written without touching updated_at
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE user_identities (
    id            uuid PRIMARY KEY,
    user_id       uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    provider_id   text NOT NULL,
    subject       text NOT NULL,
    email_at_link citext NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    UNIQUE (provider_id, subject)
);
CREATE INDEX user_identities_user_id_idx ON user_identities (user_id);

CREATE TABLE sessions (
    id           uuid PRIMARY KEY,
    user_id      uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    token_hash   bytea NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    ip           inet,
    user_agent   text CHECK (char_length(user_agent) <= 256),
    last_seen_at timestamptz NOT NULL DEFAULT now(), -- churn column
    expires_at   timestamptz NOT NULL,
    revoked_at   timestamptz,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX sessions_user_id_idx ON sessions (user_id);

CREATE TABLE email_tokens (
    id         uuid PRIMARY KEY,
    user_id    uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    purpose    text NOT NULL CHECK (purpose IN ('verify_email', 'reset_password')),
    token_hash bytea NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    expires_at timestamptz NOT NULL,
    used_at    timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX email_tokens_user_id_idx ON email_tokens (user_id);

CREATE TABLE org_members (
    org_id     uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    user_id    uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    role       text NOT NULL CHECK (role IN ('owner', 'admin', 'member')),
    status     text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'suspended')),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, user_id)
);
CREATE INDEX org_members_user_id_idx ON org_members (user_id);

CREATE TABLE invitations (
    id           uuid PRIMARY KEY,
    org_id       uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    email        citext NOT NULL,
    role         text NOT NULL CHECK (role IN ('admin', 'member')),
    workspace_ids uuid[] NOT NULL DEFAULT '{}',
    token_hash   bytea NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    status       text NOT NULL CHECK (status IN ('pending', 'accepted', 'cancelled', 'expired')),
    invited_by   uuid REFERENCES users (id) ON DELETE SET NULL,
    accepted_by  uuid REFERENCES users (id) ON DELETE SET NULL,
    expires_at   timestamptz NOT NULL DEFAULT now() + interval '7 days',
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX invitations_pending_email_uq ON invitations (org_id, email) WHERE status = 'pending';

CREATE TABLE workspaces (
    id                   uuid PRIMARY KEY,
    org_id               uuid NOT NULL REFERENCES organizations (id) ON DELETE RESTRICT,
    slug                 text NOT NULL,
    name                 text NOT NULL,
    timezone             text NOT NULL DEFAULT 'UTC',
    memory_language      text NOT NULL DEFAULT 'english',
    home_sync_generation bigint NOT NULL DEFAULT 0,
    created_at           timestamptz NOT NULL DEFAULT now(),
    updated_at           timestamptz NOT NULL DEFAULT now(),
    UNIQUE (org_id, slug)
);

CREATE TABLE workspace_access (
    workspace_id uuid NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    user_id      uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    role         text NOT NULL CHECK (role IN ('owner', 'member')),
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, user_id)
);
CREATE INDEX workspace_access_user_id_idx ON workspace_access (user_id);

CREATE TABLE api_keys (
    id             uuid PRIMARY KEY,
    org_id         uuid NOT NULL REFERENCES organizations (id) ON DELETE RESTRICT,
    workspace_id   uuid REFERENCES workspaces (id) ON DELETE CASCADE,
    user_id        uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    kind           text NOT NULL CHECK (kind IN ('user', 'service')),
    name           text NOT NULL,
    token_hash     bytea NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    display_prefix text NOT NULL,
    last_used_at   timestamptz, -- churn column, <=1 write/min
    expires_at     timestamptz,
    revoked_at     timestamptz,
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX api_keys_org_id_idx ON api_keys (org_id);
CREATE INDEX api_keys_user_id_idx ON api_keys (user_id);

CREATE TABLE cli_logins (
    id               uuid PRIMARY KEY,
    device_code_hash bytea NOT NULL UNIQUE CHECK (octet_length(device_code_hash) = 32),
    user_code        text NOT NULL UNIQUE,
    status           text NOT NULL CHECK (status IN ('pending', 'approved', 'denied', 'expired', 'consumed')),
    approved_by      uuid REFERENCES users (id) ON DELETE CASCADE,
    workspace_id     uuid REFERENCES workspaces (id) ON DELETE CASCADE,
    api_key_id       uuid REFERENCES api_keys (id) ON DELETE CASCADE,
    expires_at       timestamptz NOT NULL,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now()
);

-- Ledger: no foreign keys. Immutable, so no updated_at.
CREATE TABLE audit_log (
    id          uuid PRIMARY KEY,
    org_id      uuid,
    workspace_id uuid,
    actor_kind  text NOT NULL CHECK (actor_kind IN ('user', 'service', 'agent', 'system', 'instance_admin')),
    actor_id    uuid,
    actor_label text NOT NULL,
    action      text NOT NULL,
    target_kind text,
    target_id   text,
    outcome     text NOT NULL CHECK (outcome IN ('success', 'failure', 'denied')),
    source      text NOT NULL CHECK (source IN ('dashboard', 'api', 'cli', 'channel', 'system')),
    ip          inet,
    metadata    jsonb NOT NULL DEFAULT '{}',
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX audit_log_org_created_idx ON audit_log (org_id, created_at DESC);
CREATE INDEX audit_log_workspace_created_idx ON audit_log (workspace_id, created_at DESC);
