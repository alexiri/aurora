-- FIX-253 (fixes2 G): a schedule created from a delegated task's peer turn carries the task's
-- `ends_at` (the peer task's `expires_at`), and the fire path skips and disables such a schedule
-- once the task is closed or its time is up (`disabled_reason = 'task_ended'`,
-- `last_outcome = 'skipped_task_closed'`) instead of running it in its own thread. Existing rows
-- keep `ends_at` NULL: only peer-created schedules are bounded by a task.
ALTER TABLE schedules ADD COLUMN ends_at timestamptz NULL;

ALTER TABLE schedules DROP CONSTRAINT schedules_disabled_reason_check;
ALTER TABLE schedules ADD CONSTRAINT schedules_disabled_reason_check
    CHECK (disabled_reason IN ('user', 'authorization', 'failures', 'task_ended'));

ALTER TABLE schedules DROP CONSTRAINT schedules_last_outcome_check;
ALTER TABLE schedules ADD CONSTRAINT schedules_last_outcome_check
    CHECK (last_outcome IN ('ok', 'failed', 'skipped_busy', 'gated_quiet', 'skipped_frozen',
                            'waiting_paused', 'skipped_task_closed'));
