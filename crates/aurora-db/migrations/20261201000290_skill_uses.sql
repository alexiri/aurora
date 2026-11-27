-- P5-SKILLS-1 (C-1175, D-505, SEC-158): every time an agent loads a skill. Written by the
-- `skill_get` tool and by the supervisor's report of the harness's own skill loading; one row per
-- turn and skill at most. Kept 90 days (the daily retention pass prunes older rows).
CREATE TABLE skill_uses (
    skill_id        uuid        NOT NULL REFERENCES skills (id) ON DELETE CASCADE,
    agent_id        uuid        NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    -- A deleted conversation keeps the use (it counts) and drops the reference.
    conversation_id uuid        NULL REFERENCES conversations (id) ON DELETE SET NULL,
    turn_id         uuid        NULL REFERENCES turns (id) ON DELETE SET NULL,
    at              timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX skill_uses_turn_skill_uq ON skill_uses (turn_id, skill_id)
    WHERE turn_id IS NOT NULL;
-- The agent's view (this week, last use) and a skill's "used by N agents".
CREATE INDEX skill_uses_agent_skill_idx ON skill_uses (agent_id, skill_id, at DESC);
CREATE INDEX skill_uses_skill_at_idx ON skill_uses (skill_id, at);
CREATE INDEX skill_uses_conversation_idx ON skill_uses (conversation_id)
    WHERE conversation_id IS NOT NULL;
CREATE INDEX skill_uses_at_idx ON skill_uses (at);
