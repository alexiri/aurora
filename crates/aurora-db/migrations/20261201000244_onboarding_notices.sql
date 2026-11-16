-- P5-IA-1 (C-987): one-time notices a person has dismissed ("Pages have new names").
-- Everyone who exists now has used the old dashboard and still sees the notice; accounts created
-- after this migration never do, because a new row (and the answer for a person without a row)
-- starts with `renames-2026-11` already seen.

ALTER TABLE user_onboarding
    ADD COLUMN notices_seen text[] NOT NULL DEFAULT '{}'
        CONSTRAINT user_onboarding_notices_seen_max CHECK (cardinality(notices_seen) <= 50);

-- A person who has no row yet existed before this migration: give them one that has not seen it.
INSERT INTO user_onboarding (user_id)
SELECT id FROM users
ON CONFLICT DO NOTHING;

ALTER TABLE user_onboarding
    ALTER COLUMN notices_seen SET DEFAULT ARRAY['renames-2026-11']::text[];
