-- (P6-TPL-2, D-435, D-436, D-456, D-457, C-1106; 03 §agent templates) Agent templates:
-- sources from any https git repository (the skill library's address rules and sealed token),
-- the templates of a source, the ingested versions (ingest and approval, SEC-146) and the
-- organizations' switches. Additive; the gateway role gets nothing on these tables.

CREATE TABLE agent_template_sources (
    id                  uuid PRIMARY KEY,
    -- From the manifest's `repo.name`, cleaned.
    name                text NOT NULL,
    -- The stored, normalized https git address (C-1123 form).
    url                 text NOT NULL,
    ref                 text,
    -- The folder in the repository (may be the root, at most 4 segments).
    path                text NOT NULL DEFAULT '',
    -- The sealed read-only access token `{ username, token }` (purpose
    -- `template_source.token`, owner = the source id); both null or both set (C-1126's shape).
    token_ref           text,
    token_updated_at    timestamptz,
    homepage            text,
    contact             text,
    -- The manifest's `packages.url` template (`{id}`, `{version}`).
    packages_url        text NOT NULL,
    -- The last accepted manifest as parsed (the dashboard reads this, never the live source).
    manifest            jsonb NOT NULL,
    last_head_sha       text,
    update_interval     text NOT NULL DEFAULT '1d'
        CHECK (update_interval IN ('6h', '12h', '1d', '1w', '2w', '1mo', 'never')),
    status              text NOT NULL DEFAULT 'ok' CHECK (status IN ('ok', 'check_failed')),
    check_failed_at     timestamptz,
    -- Sanitized: never a token, never a URL with credentials.
    check_failed_reason text CHECK (char_length(check_failed_reason) <= 200),
    last_checked_at     timestamptz,
    next_check_at       timestamptz NOT NULL DEFAULT now(),
    check_lease_until   timestamptz,
    lease_owner         uuid,
    created_by          uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT agent_template_sources_token_ck
        CHECK ((token_ref IS NULL) = (token_updated_at IS NULL))
);
CREATE UNIQUE INDEX agent_template_sources_addr_uq
    ON agent_template_sources (url, coalesce(ref, ''), path);
CREATE INDEX agent_template_sources_next_check_idx ON agent_template_sources (next_check_at);

CREATE TABLE agent_templates (
    id                uuid PRIMARY KEY,
    -- The `<id>` of `template.yaml`; unique on the instance (one source holds it).
    template_id       text NOT NULL UNIQUE,
    source_id         uuid NOT NULL REFERENCES agent_template_sources (id) ON DELETE CASCADE,
    dir               text NOT NULL,
    -- From `template.yaml` at the head, cleaned.
    name              text NOT NULL,
    summary           text NOT NULL,
    head_version      text,
    -- `ok` | `needs_newer_aurora` | `invalid`.
    head_state        text NOT NULL DEFAULT 'ok'
        CHECK (head_state IN ('ok', 'needs_newer_aurora', 'invalid')),
    -- At most 20 C-1141 problems of the head's `template.yaml`.
    head_problems     jsonb,
    -- The admin's withdraw switch (withdrawn = not offered anywhere).
    offered           boolean NOT NULL DEFAULT true,
    removed_upstream  boolean NOT NULL DEFAULT false,
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX agent_templates_source_idx ON agent_templates (source_id);

CREATE TABLE agent_template_versions (
    id                uuid PRIMARY KEY,
    agent_template_id uuid NOT NULL REFERENCES agent_templates (id) ON DELETE CASCADE,
    version           text NOT NULL,
    -- `awaiting_package` | `pending_approval` | `approved` | `rejected` | `invalid`.
    status            text NOT NULL
        CHECK (status IN ('awaiting_package', 'pending_approval', 'approved', 'rejected', 'invalid')),
    -- A short code (`package_not_found`, `download_failed`, `source_mismatch`, …).
    status_reason     text,
    -- The head commit where `template.yaml` had this version.
    commit_sha        text NOT NULL,
    package_url       text NOT NULL,
    archive_sha256    text,
    size_bytes        integer,
    content_sha256    text,
    -- The parsed TemplateContent (files base64; the avatar re-encoded to a 256×256 PNG);
    -- null only for `awaiting_package` and `invalid`.
    content           jsonb,
    last_attempt_at   timestamptz,
    decided_by        uuid REFERENCES users (id) ON DELETE SET NULL,
    decided_at        timestamptz,
    created_at        timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT agent_template_versions_content_ck
        CHECK ((status IN ('awaiting_package', 'invalid')) = (content IS NULL))
);
CREATE UNIQUE INDEX agent_template_versions_uq
    ON agent_template_versions (agent_template_id, version);
CREATE INDEX agent_template_versions_status_idx
    ON agent_template_versions (agent_template_id, created_at DESC);

CREATE TABLE org_disabled_agent_templates (
    -- Keyed by the template id (not a FK), so a source removed and added again keeps the
    -- organization's choice.
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    template_id text NOT NULL,
    disabled_by uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, template_id)
);
