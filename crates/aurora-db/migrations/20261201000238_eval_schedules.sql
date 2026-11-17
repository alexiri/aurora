-- P4-EVAL-2 (D-293, C-870): eval schedules. They fire through the schedules engine's due-work
-- arm (same lease, anchor and fire-time authority) but start an eval run instead of a turn.
-- The gateway database role gets nothing on this table.
CREATE TABLE eval_schedules (
    id                   uuid PRIMARY KEY,
    agent_id             uuid NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    name                 text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 200),
    cron                 text NOT NULL,
    timezone             text NOT NULL,
    enabled              boolean NOT NULL DEFAULT true,
    disabled_reason      text CHECK (disabled_reason IN ('user', 'authorization', 'failures')),
    consecutive_failures int NOT NULL DEFAULT 0,
    creator_user_id      uuid REFERENCES users (id) ON DELETE SET NULL,
    anchor               text NOT NULL DEFAULT 'creator' CHECK (anchor IN ('creator', 'workspace')),
    next_fire_at         timestamptz,
    fire_lease_until     timestamptz,
    last_fire_at         timestamptz,
    last_outcome         text CHECK (last_outcome IN ('ok', 'failed', 'skipped_busy', 'skipped_frozen')),
    last_run_id          uuid REFERENCES eval_runs (id) ON DELETE SET NULL,
    created_at           timestamptz NOT NULL DEFAULT now(),
    updated_at           timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX eval_schedules_agent_idx ON eval_schedules (agent_id);
CREATE INDEX eval_schedules_due_idx ON eval_schedules (next_fire_at) WHERE enabled AND next_fire_at IS NOT NULL;
CREATE INDEX eval_schedules_creator_idx ON eval_schedules (creator_user_id) WHERE creator_user_id IS NOT NULL;

ALTER TABLE eval_runs
    ADD CONSTRAINT eval_runs_schedule_fk FOREIGN KEY (schedule_id) REFERENCES eval_schedules (id) ON DELETE SET NULL;
CREATE INDEX eval_runs_schedule_idx ON eval_runs (schedule_id) WHERE schedule_id IS NOT NULL;
