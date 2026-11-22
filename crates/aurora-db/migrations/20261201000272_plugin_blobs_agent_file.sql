-- (phase 5, P5-APPSKILL-1, C-1132, D-453) The agent guide is replaced by skills: a package blob that
-- an agent can read is now `agent_file` (the operations file, every `SKILL.md`, every skill file).
-- Rows and check constraint move together, in this one transaction. Content is still read by
-- digest only, whatever the row's kind (C-642).
ALTER TABLE plugin_blobs DROP CONSTRAINT plugin_blobs_kind_check;
UPDATE plugin_blobs SET kind = 'agent_file' WHERE kind = 'agent_guide';
ALTER TABLE plugin_blobs ADD CONSTRAINT plugin_blobs_kind_check
    CHECK (kind IN ('hooks_wasm', 'icon', 'icon_dark', 'setup_guide', 'archive', 'agent_file'));
