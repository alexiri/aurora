-- FIX-223 (D-466): agent avatars vanished about an hour after upload because the hourly orphan-blob
-- sweep counted only `attachments` as references. The sweep now also honours
-- `agents.avatar_blob_key`:
--   * an index serves its anti-join;
--   * agents whose avatar bytes the sweep already deleted lose the dangling pointer, so the web shows
--     the initial and offers Change picture (the bytes cannot be recovered; people re-upload).
-- Only pointers to a missing blob are cleared. `updated_at` is left alone and no `presence.set_icon`
-- is queued: chat platforms keep the icon they already downloaded. Idempotent: a second run finds
-- no dangling pointer.
CREATE INDEX IF NOT EXISTS agents_avatar_blob_key_idx
    ON agents (avatar_blob_key) WHERE avatar_blob_key IS NOT NULL;

UPDATE agents
   SET avatar_blob_key = NULL, avatar_public_key = NULL
 WHERE avatar_blob_key IS NOT NULL
   AND NOT EXISTS (SELECT 1 FROM blobs b WHERE b.key = agents.avatar_blob_key);
