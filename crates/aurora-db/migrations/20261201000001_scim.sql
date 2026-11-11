-- P4-GRP-2 (D-132, D-135, SEC-72, SEC-73): SCIM 2.0 provisioning (`/scim/v2`).
-- SCIM is a link layer: `scim_users` ties one organization to a global `users` row (its own SCIM
-- id, not the Aurora user id, which would leak across orgs), `scim_tokens` holds the hashed
-- `aud_` provisioning tokens of one org. The gateway role gets nothing on these tables; token
-- liveness is read on every SCIM request (no cache).

-- The IdP's correlation id for a SCIM group (04 §SCIM: never used to find, link or
-- authenticate anything).
ALTER TABLE groups ADD COLUMN external_id text CHECK (char_length(external_id) <= 256);
-- Only a `source = scim` group carries one.
ALTER TABLE groups ADD CONSTRAINT groups_external_id_source_ck
    CHECK (source = 'scim' OR external_id IS NULL);
CREATE UNIQUE INDEX groups_org_external_id_uq ON groups (org_id, external_id)
    WHERE external_id IS NOT NULL;

CREATE TABLE scim_tokens (
    id             uuid PRIMARY KEY,
    org_id         uuid NOT NULL REFERENCES organizations (id) ON DELETE RESTRICT,
    name           text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80),
    token_hash     bytea NOT NULL UNIQUE,
    display_prefix text NOT NULL CHECK (char_length(display_prefix) <= 12),
    created_by     uuid REFERENCES users (id) ON DELETE SET NULL,
    expires_at     timestamptz NOT NULL,
    revoked_at     timestamptz,
    -- churn column: written at most once a minute, without touching created_at
    last_used_at   timestamptz,
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX scim_tokens_org_id_idx ON scim_tokens (org_id);

CREATE TABLE scim_users (
    id                 uuid PRIMARY KEY,
    org_id             uuid NOT NULL REFERENCES organizations (id) ON DELETE RESTRICT,
    user_id            uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    external_id        text CHECK (char_length(external_id) <= 256),
    -- the e-mail as asserted by the IdP; the domain was checked against the org's
    -- `email_domains` when the link was written (04 §SCIM)
    user_name          citext NOT NULL,
    display_name       text CHECK (char_length(display_name) <= 120),
    given_name         text CHECK (char_length(given_name) <= 120),
    family_name        text CHECK (char_length(family_name) <= 120),
    state              text NOT NULL DEFAULT 'active'
                       CHECK (state IN ('active', 'suspended', 'removed')),
    -- true only when SCIM made the org_members row `suspended`: a human suspension is never
    -- lifted by the IdP (04 §SCIM Reactivation)
    suspended_by_scim  boolean NOT NULL DEFAULT false,
    removed_at         timestamptz,
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now(),
    UNIQUE (org_id, user_id)
);
CREATE UNIQUE INDEX scim_users_org_external_id_uq ON scim_users (org_id, external_id)
    WHERE external_id IS NOT NULL;
CREATE INDEX scim_users_user_id_idx ON scim_users (user_id);

-- The gateway role gets nothing on the SCIM tables (03 §scim_tokens): no grants here.
