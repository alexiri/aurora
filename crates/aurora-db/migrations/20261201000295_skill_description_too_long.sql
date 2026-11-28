-- A skill whose description is over the format's maximum has its own reason code.
ALTER TABLE skill_library_updates
    DROP CONSTRAINT skill_library_updates_reason_code_check;
ALTER TABLE skill_library_updates
    ADD CONSTRAINT skill_library_updates_reason_code_check
        CHECK (reason_code IS NULL OR reason_code IN (
            'too_large', 'body_too_large', 'file_too_large', 'too_many_files',
            'unsupported_path', 'unsupported_entry', 'skill_md', 'description_too_long',
            'bad_name', 'unknown_size'));
