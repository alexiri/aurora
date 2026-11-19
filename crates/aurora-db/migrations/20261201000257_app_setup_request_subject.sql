-- P5-APIGAP-2 (D-399, C-1047): a setup request has a subject: an app (as before), a chat platform or an AI key.
-- `app_id` keeps the app id for `app`, holds the connector type (or '') for `chat_platform` and '' for `ai_key`.
-- Existing rows are apps. One open request per (organization, subject, app_id).
ALTER TABLE app_setup_requests
    ADD COLUMN subject text NOT NULL DEFAULT 'app'
        CHECK (subject IN ('app', 'chat_platform', 'ai_key'));

DROP INDEX app_setup_requests_open;
CREATE UNIQUE INDEX app_setup_requests_open
    ON app_setup_requests (org_id, subject, app_id) WHERE status = 'open';
