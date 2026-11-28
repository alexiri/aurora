-- (FIX-291, D-513) A skill's description may be as long as the open Agent Skills format
-- allows: 1,024 characters, still on one line. Widening only: every stored row passes.
ALTER TABLE skills DROP CONSTRAINT skills_description_check;
ALTER TABLE skills ADD CONSTRAINT skills_description_check
    CHECK (char_length(description) <= 1024 AND description !~ E'[\\r\\n]');
