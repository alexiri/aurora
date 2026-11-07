-- RFE-021 (phase 2, docs/03, D-58): workspace-anchored schedules and webhooks. `creator` keeps the
-- phase-0 rule (the creator's access is the fire-time authority); `workspace` means "an active
-- owner exists". Additive: every existing row stays `creator`.
ALTER TABLE schedules ADD COLUMN anchor text NOT NULL DEFAULT 'creator'
    CHECK (anchor IN ('creator','workspace'));
ALTER TABLE webhooks ADD COLUMN anchor text NOT NULL DEFAULT 'creator'
    CHECK (anchor IN ('creator','workspace'));
