-- FIX-230 (D-473, C-1157): the single definition of a person's access to a workspace IN THEIR OWN
-- RIGHT: an active direct binding (manual or IdP-claimed) or an active group binding, while the
-- person is an active org member and not suspended in that workspace. The implicit owner role of
-- an effective org owner/admin is NOT here (it stays in effective_workspace_access). Chat offers
-- and auto-picks agents from this view; authorization keeps reading effective_workspace_access.
-- The two branches are today's branches 2 and 3 of effective_workspace_access, verbatim.
CREATE VIEW own_workspace_access AS
SELECT x.workspace_id, x.org_id, x.user_id,
       (ARRAY['member', 'approver', 'owner'])[max(x.rank)] AS role
  FROM (
        -- the direct binding, while not suspended
        SELECT wa.workspace_id, w.org_id, wa.user_id,
               array_position(ARRAY['member', 'approver', 'owner'], wa.role) AS rank
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

-- Same output as before: the implicit owner of an effective org owner/admin, plus the own access.
-- Same columns, so CREATE OR REPLACE keeps every grant and dependent query.
CREATE OR REPLACE VIEW effective_workspace_access AS
SELECT x.workspace_id, x.org_id, x.user_id,
       (ARRAY['member', 'approver', 'owner'])[max(x.rank)] AS role
  FROM (
        -- implicit owner for effective org owners and admins
        SELECT w.id AS workspace_id, w.org_id, e.user_id, 3 AS rank
          FROM workspaces w
          JOIN effective_org_members e ON e.org_id = w.org_id AND e.role IN ('owner', 'admin')
        UNION ALL
        SELECT o.workspace_id, o.org_id, o.user_id,
               array_position(ARRAY['member', 'approver', 'owner'], o.role)
          FROM own_workspace_access o
       ) x
 GROUP BY x.workspace_id, x.org_id, x.user_id;
