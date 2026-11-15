-- FIX-082 (REVIEW-5, D-175): SCIM prepares a membership for an existing account, only that
-- person's own verified sign-in activates it. `pending` counts against the seat cap, can be
-- placed in SCIM groups and confers nothing: every effective-access view, key, grant,
-- group-derived access and gateway identity already filters `status = 'active'`.
-- No existing row changes.
ALTER TABLE org_members DROP CONSTRAINT org_members_status_check;
ALTER TABLE org_members
    ADD CONSTRAINT org_members_status_check CHECK (status IN ('active', 'suspended', 'pending'));
