-- RFE-012 (D-55): third workspace role `approver`. Additive; existing rows are unchanged
-- (plain members lose approval rights at upgrade, no silent privilege).
ALTER TABLE workspace_access DROP CONSTRAINT workspace_access_role_check;
ALTER TABLE workspace_access
    ADD CONSTRAINT workspace_access_role_check CHECK (role IN ('owner', 'approver', 'member'));

ALTER TABLE invitations ADD COLUMN approver_workspace_ids uuid[] NOT NULL DEFAULT '{}';
