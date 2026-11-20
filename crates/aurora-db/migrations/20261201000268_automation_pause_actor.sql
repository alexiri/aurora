-- P5-REFINE-14 send-back (C-1096, C-1098): who paused a schedule or webhook, and the name an
-- automation's run threads had before the last rename. Both are display data only.
ALTER TABLE schedules
    ADD COLUMN disabled_by uuid NULL REFERENCES users(id) ON DELETE SET NULL;
ALTER TABLE webhooks
    ADD COLUMN disabled_by uuid NULL REFERENCES users(id) ON DELETE SET NULL;
ALTER TABLE conversations
    ADD COLUMN previous_title text NULL;
