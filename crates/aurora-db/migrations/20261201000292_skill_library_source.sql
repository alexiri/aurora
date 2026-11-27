-- A skill library knows its default branch's name, and counts every change of what it reads, so
-- that a pass over the old source can write nothing after it.
ALTER TABLE skill_libraries
    ADD COLUMN default_branch text NULL
        CHECK (default_branch IS NULL OR char_length(default_branch) BETWEEN 1 AND 200),
    ADD COLUMN source_epoch integer NOT NULL DEFAULT 0;

-- The code of why an `invalid` proposal cannot be applied.
ALTER TABLE skill_library_updates
    ADD COLUMN reason_code text NULL
        CHECK (reason_code IS NULL OR reason_code IN (
            'too_large', 'body_too_large', 'file_too_large', 'too_many_files',
            'unsupported_path', 'unsupported_entry', 'skill_md', 'bad_name', 'unknown_size'));

-- Open proposals may come from a source the library no longer follows (a change of branch, tag
-- or folder did not drop them), and the invalid ones have no code. They are only offers: they are
-- all dropped and every library is compared again from its source as it is now. Rejections stay
-- remembered. The stored counts came from a partial read, so they are counted again too.
UPDATE skill_library_updates SET status = 'superseded', decided_at = now()
 WHERE status = 'pending';
UPDATE skill_libraries
   SET last_head_sha = NULL, head_epoch = head_epoch + 1, skills_offered = NULL,
       skills_unavailable = NULL, next_refresh_at = now(), updated_at = now();
