-- P3-022 (RFE-022, D-65, C-340..C-343, SEC-57): plugin major upgrades.
-- `credential_keys` holds the NAMES (never values) of the fields present in the sealed credential
-- JSON, maintained wherever credentials are sealed (connect, reconnect, refresh, mint). Null =
-- unknown (legacy rows): the upgrade check then derives the names from the manifest of the row's
-- recorded `app_version`, else fails closed (09 §10). The gateway role's column-level UPDATE list
-- gains the column in `grants.rs`.
ALTER TABLE app_connections ADD COLUMN IF NOT EXISTS credential_keys text[];

-- C-342: `plugin_major_upgrade: <package> <old> -> <new>; missing: <names>` needs up to 300
-- characters; the phase-0 column limit was 64.
ALTER TABLE app_connections DROP CONSTRAINT IF EXISTS app_connections_status_reason_check;
ALTER TABLE app_connections
    ADD CONSTRAINT app_connections_status_reason_check CHECK (char_length(status_reason) <= 300);
