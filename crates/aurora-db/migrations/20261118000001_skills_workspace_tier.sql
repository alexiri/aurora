-- P3-020 (RFE-020, D-64, 03 §Planned changes `skills` (workspace tier)): workspace-tier skills.
-- Additive: a nullable `workspace_id`. Tier = agent (`agent_id` set) | workspace (`workspace_id`
-- set) | org (both null). An agent skill never carries a workspace id.

ALTER TABLE skills
    ADD COLUMN workspace_id uuid REFERENCES workspaces (id) ON DELETE CASCADE,
    ADD CONSTRAINT skills_one_tier CHECK (workspace_id IS NULL OR agent_id IS NULL);

-- The org-tier uniqueness now excludes workspace skills (replacement keeps existing org skills
-- unique: every existing row has workspace_id NULL).
DROP INDEX skills_org_name_uq;
CREATE UNIQUE INDEX skills_org_name_uq ON skills (org_id, name)
    WHERE agent_id IS NULL AND workspace_id IS NULL;
CREATE UNIQUE INDEX skills_workspace_name_uq ON skills (workspace_id, name)
    WHERE workspace_id IS NOT NULL;
CREATE INDEX skills_workspace_idx ON skills (workspace_id) WHERE workspace_id IS NOT NULL;
