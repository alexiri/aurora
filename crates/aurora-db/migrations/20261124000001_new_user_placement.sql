-- P4-ONB-1 (D-140, SEC-79): new-user placement.
--
-- `instance_placement` is the single row the instance admin saves in the dashboard (no row =
-- "unsaved": the instance behaves as before, `personal_org` under `open`).
-- `workspaces.personal_owner_user_id` binds a personal workspace to its user; sign-in finds the
-- workspace only by this id, never by name or slug (SEC-79).

CREATE TABLE instance_placement (
    singleton           boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    mode                text    NOT NULL CHECK (mode IN ('shared_org', 'personal_org', 'none')),
    default_org_id      uuid    REFERENCES organizations (id) ON DELETE SET NULL,
    default_org_role    text    NOT NULL DEFAULT 'member' CHECK (default_org_role IN ('member', 'admin')),
    shared_workspaces   jsonb   NOT NULL DEFAULT '[]',
    personal_enabled    boolean NOT NULL DEFAULT false,
    personal_role       text    NOT NULL DEFAULT 'owner' CHECK (personal_role IN ('owner', 'approver', 'member')),
    name_template       text    NOT NULL DEFAULT '{preferred_username}|{email_local}',
    starter_grants      jsonb   NOT NULL DEFAULT '[]',
    updated_by          uuid    REFERENCES users (id) ON DELETE SET NULL,
    updated_at          timestamptz NOT NULL DEFAULT now()
);

ALTER TABLE workspaces
    ADD COLUMN personal_owner_user_id uuid REFERENCES users (id) ON DELETE SET NULL;

CREATE UNIQUE INDEX workspaces_personal_owner_key
    ON workspaces (org_id, personal_owner_user_id)
    WHERE personal_owner_user_id IS NOT NULL;
