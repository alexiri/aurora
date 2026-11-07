-- RFE-001 (phase 1, docs/03): the platform-asserted email of the person, kept only to pre-fill
-- the sign-in (`login_hint`) and the confirm page. It never authorizes anything.
ALTER TABLE link_requests ADD COLUMN platform_email_hint citext;
