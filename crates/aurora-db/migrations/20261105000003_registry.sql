-- 03 §3 Plugin registry.

CREATE TABLE trusted_keys (
    id         uuid PRIMARY KEY,
    publisher  text NOT NULL,
    name       text NOT NULL,
    public_key text NOT NULL UNIQUE,
    added_by   uuid REFERENCES users (id) ON DELETE SET NULL,
    revoked_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE plugin_packages (
    id            uuid PRIMARY KEY,
    package_id    text NOT NULL,
    version       text NOT NULL,
    contract      integer NOT NULL,
    publisher     text NOT NULL,
    digest        text NOT NULL,
    signer_key_id text NOT NULL,
    manifest      jsonb NOT NULL,
    permissions   jsonb NOT NULL,
    source        text NOT NULL CHECK (source IN ('builtin', 'directory', 'upload')),
    status        text NOT NULL CHECK (status IN ('pending_approval', 'active', 'superseded', 'rejected', 'disabled', 'invalid')),
    status_reason text CHECK (char_length(status_reason) <= 64),
    approved_by   uuid REFERENCES users (id) ON DELETE SET NULL,
    approved_at   timestamptz,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    UNIQUE (package_id, version, digest)
);
CREATE UNIQUE INDEX plugin_packages_one_active_uq ON plugin_packages (package_id) WHERE status = 'active';

-- Content-addressed and immutable: no updated_at.
CREATE TABLE plugin_blobs (
    digest     text PRIMARY KEY,
    kind       text NOT NULL CHECK (kind IN ('hooks_wasm', 'icon', 'icon_dark', 'setup_guide', 'archive')),
    content    bytea NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE host_claim_decisions (
    id                uuid PRIMARY KEY,
    claim_key         text NOT NULL UNIQUE,
    claim_fingerprint text NOT NULL,
    winner_package_id text NOT NULL,
    winner_app_id     text NOT NULL,
    decided_by        uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now()
);

-- Immutable snapshots: no updated_at.
CREATE TABLE registry_snapshots (
    id           uuid PRIMARY KEY,
    contract     integer NOT NULL,
    content_hash text NOT NULL UNIQUE,
    snapshot     jsonb NOT NULL,
    compiled_by  text NOT NULL,
    activated_at timestamptz NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX registry_snapshots_active_idx ON registry_snapshots (contract DESC, activated_at DESC);

-- 03 originally declared PK (org_id, app_id), contradicting 07 §App
-- availability which needs (org, app, NULL) and (org, app, workspace) rows.
-- Resolved as spec fix S-01 (docs/90-decisions.md): unique over all three.
CREATE TABLE org_app_availability (
    org_id       uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    app_id       text NOT NULL,
    workspace_id uuid REFERENCES workspaces (id) ON DELETE CASCADE,
    created_at   timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT org_app_availability_uniq UNIQUE NULLS NOT DISTINCT (org_id, app_id, workspace_id)
);
