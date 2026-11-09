-- (P4-MCON-1, D-128, 03 §connectors / connector_hosts / connector_host_tokens / connector_host_status)
-- Managed connectors: added from the dashboard and run by a connector host. A managed connector
-- has no `aun_` token (token_hash NULL; the UNIQUE constraint stays, NULLs do not collide).

ALTER TABLE connectors
    ADD COLUMN managed      boolean NOT NULL DEFAULT false,
    ADD COLUMN type_id      text,
    ADD COLUMN settings     jsonb,
    ADD COLUMN secret_owner uuid NOT NULL DEFAULT gen_random_uuid(),
    ADD COLUMN secret_refs  jsonb NOT NULL DEFAULT '{}'::jsonb,
    -- field name -> RFC 3339 time the sealed value was last written (shown as `updated_at`)
    ADD COLUMN secret_updated_at jsonb NOT NULL DEFAULT '{}'::jsonb;

ALTER TABLE connectors ALTER COLUMN token_hash DROP NOT NULL;
ALTER TABLE connectors
    ADD CONSTRAINT connectors_token_or_managed CHECK (managed OR token_hash IS NOT NULL),
    ADD CONSTRAINT connectors_managed_shape CHECK (managed OR (type_id IS NULL AND settings IS NULL)),
    ADD CONSTRAINT connectors_settings_size CHECK (settings IS NULL OR pg_column_size(settings) <= 16384);

CREATE TABLE connector_hosts (
    instance_id  uuid PRIMARY KEY,
    version      text NOT NULL,
    types        jsonb NOT NULL,
    last_seen_at timestamptz NOT NULL -- churn column
);
CREATE INDEX connector_hosts_seen_idx ON connector_hosts (last_seen_at);

CREATE TABLE connector_host_tokens (
    id         uuid PRIMARY KEY,
    token_hash bytea NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE connector_host_status (
    connector_id  text NOT NULL REFERENCES connectors (id) ON DELETE CASCADE,
    host_instance uuid NOT NULL,
    state         text NOT NULL CHECK (state IN ('running', 'error')),
    message       text CHECK (char_length(message) <= 300),
    updated_at    timestamptz NOT NULL, -- churn column
    PRIMARY KEY (connector_id, host_instance)
);

-- Revision of the managed-connector assignments (C-456). Bumped with `nextval` in the same
-- transaction as every managed connector change; read as `last_value` (0 while never called).
CREATE SEQUENCE connector_assignments_rev;
