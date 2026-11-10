-- P4-IDP-1 (D-139, SEC-78): roles derived from IdP claims. Claim-sourced assignments are
-- tracked on the rows they produce, so the sign-in sync can add and remove exactly those and
-- never touch a role another source gave (docs/03 §Planned changes).
--
-- `role`      effective role (the strongest of the sources), what every reader already uses.
-- `idp_role`  the role the IdP claims give (NULL: no claim-sourced role on this row).
-- `idp_source` `idp:<provider>:<value>` of the claim value that gave `idp_role`.
-- `base_role` the role the other sources gave before the claim raised it (NULL: the claim
--             created the row, so removing the claim removes the row).

ALTER TABLE org_members
    ADD COLUMN idp_role   text CHECK (idp_role IN ('owner', 'admin', 'member')),
    ADD COLUMN idp_source text,
    ADD COLUMN base_role  text CHECK (base_role IN ('owner', 'admin', 'member')),
    ADD CONSTRAINT org_members_idp_chk CHECK ((idp_role IS NULL) = (idp_source IS NULL));

ALTER TABLE workspace_access
    ADD COLUMN idp_role   text CHECK (idp_role IN ('owner', 'approver', 'member')),
    ADD COLUMN idp_source text,
    ADD COLUMN base_role  text CHECK (base_role IN ('owner', 'approver', 'member')),
    ADD CONSTRAINT workspace_access_idp_chk CHECK ((idp_role IS NULL) = (idp_source IS NULL));

-- Instance admin granted by a claim (NULL: manual / e-mail list / bootstrap-admin).
ALTER TABLE users ADD COLUMN idp_admin_source text;

-- The claim debugger: the values of the last sign-in and what they mapped to.
CREATE TABLE user_idp_roles (
    user_id      uuid PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    provider_id  text NOT NULL,
    claim_values text[] NOT NULL DEFAULT '{}',
    granted      jsonb NOT NULL DEFAULT '[]',
    synced_at    timestamptz NOT NULL DEFAULT now()
);
