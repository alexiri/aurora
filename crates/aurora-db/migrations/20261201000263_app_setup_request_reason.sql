-- P5-REFINE-8 (D-417, D-407, C-1081; SEC-135 amended): a request carries the asker's optional reason and the
-- decliner's optional message, and remembers which role closed it. Both texts are plain text, sanitized on write
-- by the API (control and invisible formatting characters removed), at most 300 characters, never given to an
-- agent. `closed_by_role` lets the asker read "An admin isn't setting up …" without ever learning who.
ALTER TABLE app_setup_request_askers
    ADD COLUMN reason text NULL
        CHECK (reason IS NULL OR char_length(reason) BETWEEN 1 AND 300);

ALTER TABLE app_setup_requests
    ADD COLUMN closed_message text NULL
        CHECK (closed_message IS NULL OR char_length(closed_message) BETWEEN 1 AND 300),
    ADD COLUMN closed_by_role text NULL
        CHECK (closed_by_role IN ('org_admin', 'instance_admin', 'system'));

-- Existing closed rows: a decline was always an organization owner's or admin's; a resolution is the system's.
UPDATE app_setup_requests SET closed_by_role = 'org_admin'
    WHERE status = 'declined' AND closed_by IS NOT NULL;
UPDATE app_setup_requests SET closed_by_role = 'system'
    WHERE status = 'resolved';

-- The asker's history (C-1081) finds the requests resolved for them by their `resolved` notice.
CREATE INDEX app_setup_request_notices_resolved
    ON app_setup_request_notices (user_id, created_at) WHERE kind = 'resolved';
