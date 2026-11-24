-- FIX-234 (D-477, C-1160, SEC-153): an organization role administers workspaces, it does not
-- use them. Effective org owners and admins are no longer implicit owners of every workspace:
-- every workspace right comes only from access in one's own right, so effective_workspace_access
-- is now exactly own_workspace_access. Same columns, so the CREATE OR REPLACE keeps every grant
-- and every dependent query; own_access and its callers are unchanged.

CREATE OR REPLACE VIEW effective_workspace_access AS
SELECT workspace_id, org_id, user_id, role
  FROM own_workspace_access;

-- D-480 (C-1161): the person always owns their personal workspace. Before this migration the
-- org's admins were the (implicit) owners, and placement could bind the person as member or
-- approver, so after dropping the implicit role such a workspace would have had no owner at all.
-- For every workspace with a personal owner whose person is an active member of its org:
-- an existing binding of the person is raised to owner and set active (a person's own workspace
-- never suspends them), a missing binding is inserted as owner. Non-personal workspaces and
-- workspaces whose person is not an active member are untouched. Each changed row writes the
-- audit row `workspace_access.personal_owner` (actor `system`, `metadata.from`: the old role or
-- `none`).
WITH personal AS (
    SELECT w.id AS workspace_id, w.org_id, w.personal_owner_user_id AS user_id
      FROM workspaces w
      JOIN org_members m ON m.org_id = w.org_id
                        AND m.user_id = w.personal_owner_user_id
                        AND m.status = 'active'
     WHERE w.personal_owner_user_id IS NOT NULL
),
changed AS (
    SELECT wa.workspace_id, wa.user_id, wa.role AS from_role
      FROM workspace_access wa
      JOIN personal p ON p.workspace_id = wa.workspace_id AND p.user_id = wa.user_id
     WHERE wa.role <> 'owner' OR wa.status <> 'active'
     FOR UPDATE
),
raised AS (
    UPDATE workspace_access wa
       SET role = 'owner', status = 'active', updated_at = now()
      FROM changed c
     WHERE wa.workspace_id = c.workspace_id AND wa.user_id = c.user_id
),
inserted AS (
    INSERT INTO workspace_access (workspace_id, user_id, role)
    SELECT p.workspace_id, p.user_id, 'owner'
      FROM personal p
     WHERE NOT EXISTS (SELECT 1 FROM workspace_access wa
                        WHERE wa.workspace_id = p.workspace_id
                          AND wa.user_id = p.user_id)
    RETURNING workspace_id, user_id
)
INSERT INTO audit_log (id, org_id, workspace_id, actor_kind, actor_id, actor_label,
                       action, target_kind, target_id, outcome, source, metadata)
SELECT gen_random_uuid(), p.org_id, p.workspace_id, 'system', NULL::uuid, 'system',
       'workspace_access.personal_owner', 'user', p.user_id::text, 'success', 'system',
       jsonb_build_object('from', c.from_role)
  FROM changed c
  JOIN personal p ON p.workspace_id = c.workspace_id AND p.user_id = c.user_id
UNION ALL
SELECT gen_random_uuid(), p.org_id, p.workspace_id, 'system', NULL::uuid, 'system',
       'workspace_access.personal_owner', 'user', p.user_id::text, 'success', 'system',
       jsonb_build_object('from', 'none')
  FROM inserted i
  JOIN personal p ON p.workspace_id = i.workspace_id AND p.user_id = i.user_id;
