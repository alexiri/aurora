-- Self-hosted app instances (D-74, docs/03 §app_instances). Additive: one new table and two
-- nullable columns. The BYOC uniqueness widens to one client per instance.

CREATE TABLE app_instances (
    id         uuid PRIMARY KEY,
    org_id     uuid REFERENCES organizations (id) ON DELETE CASCADE,
    package_id text NOT NULL,
    app_id     text NOT NULL,
    host       text NOT NULL,
    label      text NOT NULL CHECK (char_length(label) BETWEEN 1 AND 120),
    status     text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'disabled')),
    created_by uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT app_instances_scope_uq UNIQUE NULLS NOT DISTINCT (org_id, app_id, host),
    -- Defence in depth for SEC-59: a lower-case DNS name, never an IP literal, port, path or
    -- userinfo. The API validates the same shape (and more) before insert.
    CONSTRAINT app_instances_host_ck CHECK (
        host ~ '^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$'
        AND host !~ '^[0-9.]+$'
        AND char_length(host) <= 253
    )
);
CREATE INDEX app_instances_host_idx ON app_instances (host);
CREATE INDEX app_instances_org_idx ON app_instances (org_id);

ALTER TABLE app_connections
    ADD COLUMN instance_id uuid REFERENCES app_instances (id) ON DELETE RESTRICT;
CREATE INDEX app_connections_instance_idx ON app_connections (instance_id) WHERE instance_id IS NOT NULL;

ALTER TABLE app_oauth_clients
    ADD COLUMN instance_id uuid REFERENCES app_instances (id) ON DELETE CASCADE;
ALTER TABLE app_oauth_clients DROP CONSTRAINT app_oauth_clients_scope_uq;
ALTER TABLE app_oauth_clients ADD CONSTRAINT app_oauth_clients_scope_uq
    UNIQUE NULLS NOT DISTINCT (org_id, workspace_id, package_id, client_key, instance_id);
