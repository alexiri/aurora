-- 03 §3 addon sources (P5-ASRC, D-164): third-party app-package repositories.

CREATE TABLE addon_sources (
    id                  uuid PRIMARY KEY,
    name                text NOT NULL,
    url                 text NOT NULL UNIQUE,
    homepage            text,
    contact             text,
    publisher           text NOT NULL,
    signing_key         text NOT NULL,
    key_fingerprint     text NOT NULL,
    new_key_fingerprint text,
    manifest_digest     text NOT NULL,
    update_interval     text NOT NULL DEFAULT '1d'
        CHECK (update_interval IN ('6h', '12h', '1d', '1w', '2w', '1mo', 'never')),
    status              text NOT NULL DEFAULT 'ok'
        CHECK (status IN ('ok', 'check_failed', 'signing_key_changed')),
    check_failed_at     timestamptz,
    check_failed_reason text CHECK (char_length(check_failed_reason) <= 200),
    last_checked_at     timestamptz,
    next_check_at       timestamptz NOT NULL DEFAULT now(),
    check_lease_until   timestamptz,
    lease_owner         uuid,
    created_by          uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX addon_sources_next_check_idx
    ON addon_sources (next_check_at)
    WHERE status <> 'signing_key_changed';

-- Packages of an addon source point at their source row (09 §10 source `addon`).
ALTER TABLE plugin_packages ADD COLUMN source_id uuid REFERENCES addon_sources (id)
    ON DELETE SET NULL;
ALTER TABLE plugin_packages DROP CONSTRAINT plugin_packages_source_check;
ALTER TABLE plugin_packages
    ADD CONSTRAINT plugin_packages_source_check
    CHECK (source IN ('builtin', 'directory', 'upload', 'addon'));
CREATE INDEX plugin_packages_source_id_idx ON plugin_packages (source_id)
    WHERE source_id IS NOT NULL;
