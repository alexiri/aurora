-- FIX-025 (D-158, C-580..C-589): the last start attempt of a sandbox, and recent start failures
-- per runner for instance admins. Additive; existing rows keep NULL / '[]'.

-- The current (or last) start attempt: when it was claimed, its latest phase and detail as the
-- runner reported them (`sandbox.status starting|held|running`), the pod's warning events, and
-- when the runner last reported progress. Reset by every new boot.
ALTER TABLE sandboxes
    ADD COLUMN start_attempt_at  timestamptz,
    ADD COLUMN start_phase       text,
    ADD COLUMN start_detail      text,
    ADD COLUMN start_events      jsonb NOT NULL DEFAULT '[]'::jsonb,
    ADD COLUMN start_progress_at timestamptz,
    ADD COLUMN running_at        timestamptz;

-- Recent start failures (runner reports and API deadlines). Bounded: rows older than 30 days are
-- pruned on insert. Reasons never contain secrets (runner and API sanitise them).
CREATE TABLE sandbox_start_failures (
    id           uuid PRIMARY KEY,
    runner_id    uuid REFERENCES runners (id) ON DELETE CASCADE,
    sandbox_id   uuid NOT NULL REFERENCES sandboxes (id) ON DELETE CASCADE,
    agent_id     uuid NOT NULL,
    boot_id      uuid,
    code         text NOT NULL,
    message      text NOT NULL,
    phase        text,
    events       jsonb NOT NULL DEFAULT '[]'::jsonb,
    elapsed_secs integer,
    created_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX sandbox_start_failures_runner_idx ON sandbox_start_failures (runner_id, created_at DESC);
CREATE INDEX sandbox_start_failures_created_idx ON sandbox_start_failures (created_at);
