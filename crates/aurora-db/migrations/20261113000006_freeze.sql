-- P2-006 (RFE-006, D-50): freeze an agent, a workspace or an organization. Additive: every existing
-- row is unfrozen (all four columns null). A CHECK keeps the four columns all null or all set.
ALTER TABLE agents
    ADD COLUMN frozen_at timestamptz NULL,
    ADD COLUMN frozen_by_user_id uuid NULL REFERENCES users (id) ON DELETE SET NULL,
    ADD COLUMN frozen_reason text NULL,
    ADD COLUMN frozen_authority text NULL,
    ADD CONSTRAINT agents_frozen_ck CHECK (
        (frozen_at IS NULL AND frozen_reason IS NULL AND frozen_authority IS NULL AND frozen_by_user_id IS NULL)
        OR (frozen_at IS NOT NULL AND frozen_authority IN ('workspace_owner', 'org_admin', 'instance_admin')
            AND char_length(frozen_reason) BETWEEN 1 AND 500)
        -- frozen_by_user_id is set null when the freezer is deleted
    );

ALTER TABLE workspaces
    ADD COLUMN frozen_at timestamptz NULL,
    ADD COLUMN frozen_by_user_id uuid NULL REFERENCES users (id) ON DELETE SET NULL,
    ADD COLUMN frozen_reason text NULL,
    ADD COLUMN frozen_authority text NULL,
    ADD CONSTRAINT workspaces_frozen_ck CHECK (
        (frozen_at IS NULL AND frozen_reason IS NULL AND frozen_authority IS NULL AND frozen_by_user_id IS NULL)
        OR (frozen_at IS NOT NULL AND frozen_authority IN ('workspace_owner', 'org_admin', 'instance_admin')
            AND char_length(frozen_reason) BETWEEN 1 AND 500)
    );

ALTER TABLE organizations
    ADD COLUMN frozen_at timestamptz NULL,
    ADD COLUMN frozen_by_user_id uuid NULL REFERENCES users (id) ON DELETE SET NULL,
    ADD COLUMN frozen_reason text NULL,
    ADD COLUMN frozen_authority text NULL,
    ADD CONSTRAINT organizations_frozen_ck CHECK (
        (frozen_at IS NULL AND frozen_reason IS NULL AND frozen_authority IS NULL AND frozen_by_user_id IS NULL)
        OR (frozen_at IS NOT NULL AND frozen_authority IN ('workspace_owner', 'org_admin', 'instance_admin')
            AND char_length(frozen_reason) BETWEEN 1 AND 500)
    );

ALTER TABLE sandboxes DROP CONSTRAINT sandboxes_stop_reason_check;
ALTER TABLE sandboxes ADD CONSTRAINT sandboxes_stop_reason_check
    CHECK (stop_reason IN ('idle', 'cap', 'evict', 'restart', 'delete', 'failure', 'frozen'));

ALTER TABLE schedules DROP CONSTRAINT schedules_last_outcome_check;
ALTER TABLE schedules ADD CONSTRAINT schedules_last_outcome_check
    CHECK (last_outcome IN ('ok', 'failed', 'skipped_busy', 'gated_quiet', 'skipped_frozen'));
