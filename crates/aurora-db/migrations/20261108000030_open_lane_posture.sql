-- P1-003 (RFE-003): open-lane posture per workspace policy generation, and credential
-- capabilities. Additive; existing rows take the defaults (`enabled`, no capabilities).
ALTER TABLE policy_generations
    ADD COLUMN open_lane_posture text NOT NULL DEFAULT 'enabled'
        CONSTRAINT policy_generations_open_lane_posture_ck CHECK (open_lane_posture IN ('enabled', 'allowlist_only', 'disabled')),
    ADD CONSTRAINT policy_generations_open_lane_scope_ck CHECK (workspace_id IS NOT NULL OR open_lane_posture = 'enabled');

ALTER TABLE gateway_credentials
    ADD COLUMN capabilities text[] NOT NULL DEFAULT '{}';
