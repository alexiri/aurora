-- FIX-096 (REVIEW-5 SKILL-3, D-178 Q-R5-3): browse and add accept only the library ref's current
-- head and heads this table recorded; `head_epoch` makes a refresh's head write compare-and-set
-- (an add that clears `last_head_sha` bumps it, so an older pass cannot write its head back).
-- Migration 20261201000100 (the lane's allocation; origin/master's newest was ...000081).

ALTER TABLE skill_libraries ADD COLUMN head_epoch integer NOT NULL DEFAULT 0;

CREATE TABLE skill_library_heads (
    library_id uuid NOT NULL REFERENCES skill_libraries (id) ON DELETE CASCADE,
    commit_sha text NOT NULL,
    seen_at timestamptz NOT NULL DEFAULT now(),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (library_id, commit_sha)
);

CREATE INDEX skill_library_heads_seen_idx ON skill_library_heads (library_id, seen_at DESC);
