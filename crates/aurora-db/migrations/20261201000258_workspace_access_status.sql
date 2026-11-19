-- P5-APIGAP-4 (D-399, C-1050, C-1051, SEC-138): a workspace owner suspends one person in one
-- workspace. The binding, its role and the person's data stay; a suspended binding grants
-- nothing. `joined_at` (C-1050) is the existing `created_at` of the row: no new column, no
-- backfill (every row already has an honest creation time).
ALTER TABLE workspace_access
    ADD COLUMN status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'suspended'));

-- The single SQL definition of the effective workspace role (docs/04 §Groups), now honouring the
-- workspace suspension: the direct row counts only while `active`, and a person whose direct row
-- is `suspended` derives nothing from their groups in that workspace either. The implicit owner
-- of an effective org owner/admin is unchanged (their access comes from the org role, which
-- outranks a workspace owner; such a person cannot be suspended in a workspace, C-1051).
-- Same columns as before, so `CREATE OR REPLACE` keeps every grant and dependent query.
CREATE OR REPLACE VIEW effective_workspace_access AS
SELECT x.workspace_id, x.org_id, x.user_id,
       (ARRAY['member', 'approver', 'owner'])[max(x.rank)] AS role
  FROM (
        -- implicit owner for effective org owners and admins
        SELECT w.id AS workspace_id, w.org_id, e.user_id, 3 AS rank
          FROM workspaces w
          JOIN effective_org_members e ON e.org_id = w.org_id AND e.role IN ('owner', 'admin')
        UNION ALL
        -- the direct binding, while not suspended
        SELECT wa.workspace_id, w.org_id, wa.user_id,
               array_position(ARRAY['member', 'approver', 'owner'], wa.role)
          FROM workspace_access wa
          JOIN workspaces w ON w.id = wa.workspace_id
          JOIN org_members m ON m.org_id = w.org_id AND m.user_id = wa.user_id
                            AND m.status = 'active'
         WHERE wa.status = 'active'
        UNION ALL
        -- the bindings of every group that contains the user (active members only), unless the
        -- user is suspended in that workspace
        SELECT b.workspace_id, b.org_id, gm.user_id,
               array_position(ARRAY['member', 'approver', 'owner'], b.role)
          FROM group_workspace_access b
          JOIN group_members gm ON gm.group_id = b.group_id AND gm.org_id = b.org_id
          JOIN org_members m ON m.org_id = gm.org_id AND m.user_id = gm.user_id
                            AND m.status = 'active'
         WHERE NOT EXISTS (SELECT 1 FROM workspace_access s
                            WHERE s.workspace_id = b.workspace_id AND s.user_id = gm.user_id
                              AND s.status = 'suspended')
       ) x
 GROUP BY x.workspace_id, x.org_id, x.user_id;
