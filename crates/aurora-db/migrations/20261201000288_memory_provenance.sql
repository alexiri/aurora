-- P5-MEMORY-1 (C-1169, C-1170, D-497, SEC-157): where a memory came from and when the agent
-- last used it.
--
-- `added_by` / `changed_by` are stored actors: ids only, never names or e-mail addresses (names
-- are resolved at read time). Shape: {"kind": "person"|"api_key"|"agent"|"template"|"import"|
-- "unknown", "user_id"?, "api_key_id"?, "conversation_id"?, "template"?: {"name","version"}}.
-- Dangling ids are expected (a deleted conversation or key), so there are no foreign keys.
-- `last_used_marks` maps a conversation id to the time of the last recorded use there; entries
-- older than an hour are dropped at the next write. It backs the throttle: at most one write per
-- memory, conversation and hour.
ALTER TABLE memories
    ADD COLUMN added_by jsonb NOT NULL DEFAULT '{"kind": "unknown"}'::jsonb,
    ADD COLUMN changed_by jsonb NOT NULL DEFAULT '{"kind": "unknown"}'::jsonb,
    ADD COLUMN last_used_at timestamptz NULL,
    ADD COLUMN last_used_conversation_id uuid NULL,
    ADD COLUMN last_used_marks jsonb NOT NULL DEFAULT '{}'::jsonb;

-- The API key a revision was written through; null for every older revision.
ALTER TABLE memory_revisions
    ADD COLUMN author_api_key_id uuid NULL;

-- Back-fill from the kept revisions. The adder is known only when revision 1 is still kept (only
-- the newest 50 are); the newest revision is always kept, so the last changer is known whenever
-- the memory has a revision at all. Everything else stays `unknown`.
CREATE FUNCTION memory_revision_actor(r memory_revisions) RETURNS jsonb
LANGUAGE sql STABLE AS $$
    SELECT CASE
        WHEN r.author_kind = 'agent' THEN jsonb_strip_nulls(jsonb_build_object(
            'kind', 'agent',
            'conversation_id',
            CASE WHEN r.provenance ->> 'source' = 'tool'
                 THEN r.provenance ->> 'conversation_id' END))
        WHEN r.provenance ->> 'source' = 'template' THEN jsonb_strip_nulls(jsonb_build_object(
            'kind', 'template',
            'user_id', r.author_user_id,
            'template', jsonb_build_object(
                'name', COALESCE(r.provenance ->> 'template', ''),
                'version', COALESCE(r.provenance ->> 'template_version', ''))))
        WHEN r.provenance ->> 'source' = 'api' THEN jsonb_strip_nulls(jsonb_build_object(
            'kind', 'api_key', 'user_id', r.author_user_id))
        WHEN r.author_user_id IS NOT NULL THEN
            jsonb_build_object('kind', 'person', 'user_id', r.author_user_id)
        ELSE '{"kind": "unknown"}'::jsonb
    END
$$;

UPDATE memories m SET added_by = memory_revision_actor(r)
  FROM memory_revisions r
 WHERE r.memory_id = m.id AND r.revision = 1;

UPDATE memories m SET changed_by = memory_revision_actor(r)
  FROM memory_revisions r
 WHERE r.memory_id = m.id AND r.revision = m.revision;

DROP FUNCTION memory_revision_actor(memory_revisions);
