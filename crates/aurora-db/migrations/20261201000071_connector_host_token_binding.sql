-- FIX-120 (D-189, SEC-69/SEC-70): one connector-host token per host, bound to the connector types it
-- announced. `type_ids IS NULL` marks the shared bootstrap token (every existing row): it keeps
-- working until the instance has a live bound token (docs/15 §Upgrades). A bound token can only
-- register, poll and report for its `type_ids`, and owns the host instances it registered.
ALTER TABLE connector_host_tokens
    ADD COLUMN name         text CHECK (name IS NULL OR char_length(name) BETWEEN 1 AND 80),
    ADD COLUMN prefix       text,
    ADD COLUMN type_ids     text[] CHECK (type_ids IS NULL OR cardinality(type_ids) BETWEEN 1 AND 64),
    ADD COLUMN created_by   uuid REFERENCES users (id) ON DELETE SET NULL,
    ADD COLUMN last_used_at timestamptz,
    ADD COLUMN revoked_at   timestamptz;

-- Names of bound tokens are unique among the live ones.
CREATE UNIQUE INDEX connector_host_tokens_name_idx
    ON connector_host_tokens (name) WHERE name IS NOT NULL AND revoked_at IS NULL;

-- The token that registered a host instance; another token may not register, poll or report as it.
ALTER TABLE connector_hosts
    ADD COLUMN token_id uuid REFERENCES connector_host_tokens (id) ON DELETE SET NULL;
