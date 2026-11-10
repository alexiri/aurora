-- P4-GRP-1 (D-130, D-131, SEC-71): directory groups (manual; SCIM fills the same tables later).
-- A group is a flat, org-scoped set of that org's members. It can hold workspace bindings and an
-- optional org-role mapping (`admin`/`member`, never `owner`). It never creates, reactivates or
-- removes an org membership: the composite foreign keys below make that structural.

-- Same-org composite foreign keys need a unique index on (id, org_id).
ALTER TABLE workspaces ADD CONSTRAINT workspaces_id_org_uq UNIQUE (id, org_id);

CREATE TABLE groups (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE RESTRICT,
    name        citext NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80),
    description text CHECK (char_length(description) <= 280),
    source      text NOT NULL DEFAULT 'manual' CHECK (source IN ('manual', 'scim', 'idp')),
    -- (D-156) IdP-linked groups: one provider and one claim value, set together iff source = idp.
    idp_provider text,
    idp_value   text CHECK (char_length(idp_value) <= 256),
    CONSTRAINT groups_idp_link_chk CHECK ((source = 'idp') = (idp_provider IS NOT NULL AND idp_value IS NOT NULL)
                                          AND ((idp_provider IS NULL) = (idp_value IS NULL))),
    -- The group -> org-role mapping. `owner` is not a legal value (SEC-71).
    org_role    text CHECK (org_role IN ('admin', 'member')),
    revision    bigint NOT NULL DEFAULT 1,
    created_by  uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    -- `group_members.source` must equal the group's source (and the group's source is immutable
    -- while it has members: the referencing foreign key blocks the update).
    UNIQUE (id, org_id, source),
    UNIQUE (org_id, name)
);

CREATE UNIQUE INDEX groups_idp_link_uq ON groups (org_id, idp_provider, idp_value)
    WHERE idp_provider IS NOT NULL;

CREATE TABLE group_members (
    group_id   uuid NOT NULL,
    org_id     uuid NOT NULL,
    user_id    uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    source     text NOT NULL DEFAULT 'manual' CHECK (source IN ('manual', 'scim', 'idp')),
    -- (D-156) idp rows: the sign-in that last confirmed the membership.
    synced_at  timestamptz,
    added_by  uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (group_id, user_id),
    FOREIGN KEY (group_id, org_id, source) REFERENCES groups (id, org_id, source) ON DELETE CASCADE,
    -- A member must already be a member of the group's org; leaving the org leaves every group.
    FOREIGN KEY (org_id, user_id) REFERENCES org_members (org_id, user_id) ON DELETE CASCADE
);
-- "Which groups is this user in" (the access law, the gateway).
CREATE INDEX group_members_org_user_idx ON group_members (org_id, user_id);
CREATE INDEX group_members_user_idx ON group_members (user_id);

CREATE TABLE group_workspace_access (
    group_id     uuid NOT NULL,
    workspace_id uuid NOT NULL,
    org_id       uuid NOT NULL,
    role         text NOT NULL CHECK (role IN ('owner', 'approver', 'member')),
    granted_by   uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (group_id, workspace_id),
    FOREIGN KEY (group_id, org_id) REFERENCES groups (id, org_id) ON DELETE CASCADE,
    -- A group binds only workspaces of its own org.
    FOREIGN KEY (workspace_id, org_id) REFERENCES workspaces (id, org_id) ON DELETE CASCADE
);
-- "Who has access to this workspace".
CREATE INDEX group_workspace_access_ws_idx ON group_workspace_access (workspace_id);

-- The single SQL definition of the effective roles (docs/04 §Groups, D-130(b)): the strongest of
-- the direct and group-derived values, never a sum, only for an *active* membership. Every
-- query that authorizes by role joins these views (through `aurora-db::authz`); nothing else
-- reads `workspace_access` or the group tables to decide access. With no groups they return
-- exactly the phase 0-3 answers. Views are for the API role only (the gateway role reads the
-- group tables, not these views).

-- Effective org role of every active member.
CREATE VIEW effective_org_members AS
SELECT m.org_id, m.user_id,
       (ARRAY['member', 'admin', 'owner'])[GREATEST(
           array_position(ARRAY['member', 'admin', 'owner'], m.role),
           COALESCE((SELECT max(array_position(ARRAY['member', 'admin', 'owner'], g.org_role))
                       FROM group_members gm
                       JOIN groups g ON g.id = gm.group_id
                      WHERE gm.org_id = m.org_id AND gm.user_id = m.user_id
                        AND g.org_role IS NOT NULL), 0))] AS role
  FROM org_members m
 WHERE m.status = 'active';

-- Effective workspace role of every user who can use the workspace (a row exists iff `can_use`).
CREATE VIEW effective_workspace_access AS
SELECT x.workspace_id, x.org_id, x.user_id,
       (ARRAY['member', 'approver', 'owner'])[max(x.rank)] AS role
  FROM (
        -- implicit owner for effective org owners and admins
        SELECT w.id AS workspace_id, w.org_id, e.user_id, 3 AS rank
          FROM workspaces w
          JOIN effective_org_members e ON e.org_id = w.org_id AND e.role IN ('owner', 'admin')
        UNION ALL
        -- the direct binding
        SELECT wa.workspace_id, w.org_id, wa.user_id,
               array_position(ARRAY['member', 'approver', 'owner'], wa.role)
          FROM workspace_access wa
          JOIN workspaces w ON w.id = wa.workspace_id
          JOIN org_members m ON m.org_id = w.org_id AND m.user_id = wa.user_id
                            AND m.status = 'active'
        UNION ALL
        -- the bindings of every group that contains the user (active members only)
        SELECT b.workspace_id, b.org_id, gm.user_id,
               array_position(ARRAY['member', 'approver', 'owner'], b.role)
          FROM group_workspace_access b
          JOIN group_members gm ON gm.group_id = b.group_id AND gm.org_id = b.org_id
          JOIN org_members m ON m.org_id = gm.org_id AND m.user_id = gm.user_id
                            AND m.status = 'active'
       ) x
 GROUP BY x.workspace_id, x.org_id, x.user_id;
