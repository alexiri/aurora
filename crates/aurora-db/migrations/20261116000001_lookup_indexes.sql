-- P2-FU-1 (d, FUP-M7): two lookups that scanned their table.
--   * the public avatar GET resolves `agents.avatar_public_key`;
--   * the orphan sweep and purge paths look attachments up by `blob_key`.
-- Additive and partial (null / purged keys are never looked up). Plain CREATE INDEX, not
-- CONCURRENTLY: migrations run inside a transaction here and both tables are small (agents:
-- one row per agent; attachments: bounded per org by the storage quota).
CREATE INDEX IF NOT EXISTS agents_avatar_public_key_idx
    ON agents (avatar_public_key) WHERE avatar_public_key IS NOT NULL;
CREATE INDEX IF NOT EXISTS attachments_blob_key_idx
    ON attachments (blob_key) WHERE blob_key <> '';
