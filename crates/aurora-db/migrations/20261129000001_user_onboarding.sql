-- P4-ONB-2 (D-141): per-user onboarding progress, stored server-side so it follows the person
-- across browsers. Only what cannot be computed lives here; guide step completion is derived from
-- real state (model readiness, grants, conversations) and is never stored.

CREATE TABLE user_onboarding (
    user_id           uuid PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    welcome_seen      boolean     NOT NULL DEFAULT false,
    -- Workspaces whose getting-started guide the user dismissed.
    guide_dismissed   uuid[]      NOT NULL DEFAULT '{}',
    tour_completed_at timestamptz,
    updated_at        timestamptz NOT NULL DEFAULT now(),
    CHECK (cardinality(guide_dismissed) <= 200)
);

-- People who already use the instance are not welcomed as newcomers; they can replay the tour and
-- the guide from the account page.
INSERT INTO user_onboarding (user_id, welcome_seen)
SELECT id, true FROM users
ON CONFLICT DO NOTHING;
