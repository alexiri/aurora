-- FIX-112: the sign-in link page shows the chat account being bound, and a person is prompted at most
-- once per 10 minutes. `platform_display_name` is what the platform called the person when the link
-- was requested (display only, never authorizes); the index serves the per-person throttle lookup.
ALTER TABLE link_requests ADD COLUMN platform_display_name text;
CREATE INDEX link_requests_person_idx ON link_requests (integration_id, platform_user_ref, created_at DESC);
