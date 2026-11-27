-- P5-SKILLS-1 (C-1173, C-1174, C-1176, D-498, D-499, SEC-158): a skill's display name; turning an
-- inherited skill off for one agent; and what a library offered at its last read.

-- The display name beside the folder name (`name`, immutable). Null: show the folder name made
-- readable. Library and app skills have none. No default, no backfill: adding a nullable column
-- takes no table rewrite.
ALTER TABLE skills ADD COLUMN title text NULL
    CHECK (title IS NULL OR (char_length(title) BETWEEN 1 AND 80 AND title !~ '[[:cntrl:]]'));

-- A workspace or organization skill that an agent does not use. Off only: a row never turns on
-- what the skill's owners switched off. The row goes with the agent or the skill; the skill is
-- treated as not enabled for that agent only.
CREATE TABLE agent_skills_off (
    agent_id uuid        NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    skill_id uuid        NOT NULL REFERENCES skills (id) ON DELETE CASCADE,
    set_by   uuid        NULL REFERENCES users (id) ON DELETE SET NULL,
    set_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (agent_id, skill_id)
);
CREATE INDEX agent_skills_off_skill_idx ON agent_skills_off (skill_id);

-- How many valid and invalid skill folders the library held at its last read (create, address
-- change, browse or refresh). Null until a library has been read since this column exists.
ALTER TABLE skill_libraries
    ADD COLUMN skills_offered     integer NULL CHECK (skills_offered >= 0),
    ADD COLUMN skills_unavailable integer NULL CHECK (skills_unavailable >= 0);
