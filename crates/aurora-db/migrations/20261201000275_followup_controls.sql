-- FIX-225 (D-469, C-1153..C-1155): queued messages (`followup` rows of `turns`) wait for a person.
--   * steer_requested_at: someone clicked Steer; due-work arm 7 steers only such rows (or door `system`).
--   * steer_error: the harness did not take the steer (`missed` | `refused`); the row then runs next.
--   * edited_at: last edit by the follow-up's author.
--   * status gains `cancelled` for `followup` rows only (never promoted).
-- Existing rows are untouched: the new columns are NULL and every existing status stays valid.
ALTER TABLE turns ADD COLUMN steer_requested_at timestamptz;
ALTER TABLE turns ADD COLUMN steer_error text;
ALTER TABLE turns ADD COLUMN edited_at timestamptz;
ALTER TABLE turns ADD CONSTRAINT turns_steer_error_ck CHECK (steer_error IN ('missed', 'refused'));

ALTER TABLE turns DROP CONSTRAINT turns_status_check;
ALTER TABLE turns ADD CONSTRAINT turns_status_check CHECK (status IN
    ('queued', 'dispatched', 'running', 'done', 'failed', 'aborted', 'joining', 'joined', 'promoted', 'cancelled'));
ALTER TABLE turns DROP CONSTRAINT turns_role_status_ck;
ALTER TABLE turns ADD CONSTRAINT turns_role_status_ck CHECK (
    (role = 'turn' AND status IN ('queued', 'dispatched', 'running', 'done', 'failed', 'aborted'))
    OR (role = 'followup' AND status IN ('joining', 'joined', 'promoted', 'cancelled'))
);
