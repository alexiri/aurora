-- P5-SCHED-ONCE (D-421, D-422, C-1092, C-1095): one-time schedules and the reminder push.
--
-- A schedule has either a 5-field `cron` or a `run_at` instant (exactly one). A one-time schedule
-- fires once and is then Done (`completed_at`); the daily retention pass deletes Done rows after
-- `SCHEDULE_DONE_RETENTION`. A due one-time schedule of a paused agent waits
-- (`last_outcome = 'waiting_paused'`). `schedule_runs.answer_pushed_at` is the claim of the
-- `schedule_answer` Web Push. Existing rows all have a cron, so both checks hold for them.
-- `eval_schedules` is not touched.
ALTER TABLE schedules
    ALTER COLUMN cron DROP NOT NULL,
    ADD COLUMN run_at timestamptz NULL,
    ADD COLUMN completed_at timestamptz NULL,
    ADD CONSTRAINT schedules_cron_xor_run_at_ck CHECK ((cron IS NULL) <> (run_at IS NULL)),
    ADD CONSTRAINT schedules_completed_once_ck CHECK (completed_at IS NULL OR run_at IS NOT NULL);

ALTER TABLE schedules DROP CONSTRAINT schedules_last_outcome_check;
ALTER TABLE schedules ADD CONSTRAINT schedules_last_outcome_check
    CHECK (last_outcome IN ('ok', 'failed', 'skipped_busy', 'gated_quiet', 'skipped_frozen',
                            'waiting_paused'));

CREATE INDEX schedules_completed_idx ON schedules (completed_at) WHERE completed_at IS NOT NULL;

ALTER TABLE schedule_runs ADD COLUMN answer_pushed_at timestamptz NULL;
