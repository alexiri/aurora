-- P4-EVAL-2 (D-290, D-291, C-864, C-867, SEC-116): eval runs and their per-question results. A run
-- snapshots the question, its expected answer and the configuration fingerprint at start;
-- results are agent-audience data and are never copied to audit, logs or the sandbox. The
-- schedule foreign key is added by 20261201000238 (the two tables reference each other). The
-- gateway database role gets nothing on these tables.
CREATE TABLE eval_runs (
    id                   uuid PRIMARY KEY,
    agent_id             uuid NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    workspace_id         uuid NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    trigger              text NOT NULL CHECK (trigger IN ('manual', 'schedule')),
    schedule_id          uuid,
    started_by_user_id   uuid REFERENCES users (id) ON DELETE SET NULL,
    status               text NOT NULL DEFAULT 'queued'
                         CHECK (status IN ('queued', 'running', 'done', 'failed', 'cancelled')),
    question_set_version text NOT NULL,
    fingerprint          jsonb NOT NULL,
    fingerprint_hash     text NOT NULL,
    questions_total      int NOT NULL DEFAULT 0,
    questions_passed     int NOT NULL DEFAULT 0,
    questions_failed     int NOT NULL DEFAULT 0,
    questions_errored    int NOT NULL DEFAULT 0,
    questions_not_scored int NOT NULL DEFAULT 0,
    refused_writes       int NOT NULL DEFAULT 0,
    judge_usage          jsonb,
    regression           jsonb,
    error_code           text,
    cancel_requested_at  timestamptz,
    lease_until          timestamptz,
    created_at           timestamptz NOT NULL DEFAULT now(),
    started_at           timestamptz,
    finished_at          timestamptz,
    updated_at           timestamptz NOT NULL DEFAULT now()
);
-- One active run per agent (EVAL_RUNS_ACTIVE_PER_AGENT = 1).
CREATE UNIQUE INDEX eval_runs_active_key ON eval_runs (agent_id) WHERE status IN ('queued', 'running');
CREATE INDEX eval_runs_agent_idx ON eval_runs (agent_id, created_at DESC, id DESC);
CREATE INDEX eval_runs_workspace_idx ON eval_runs (workspace_id);
CREATE INDEX eval_runs_finished_idx ON eval_runs (finished_at) WHERE finished_at IS NOT NULL;

CREATE TABLE eval_results (
    id                uuid PRIMARY KEY,
    run_id            uuid NOT NULL REFERENCES eval_runs (id) ON DELETE CASCADE,
    position          int NOT NULL CHECK (position >= 1),
    question_id       uuid REFERENCES eval_questions (id) ON DELETE SET NULL,
    question_text     text NOT NULL,
    expected_kind     text NOT NULL CHECK (expected_kind IN ('numeric', 'text', 'judged')),
    expected_snapshot jsonb NOT NULL,
    conversation_id   uuid REFERENCES conversations (id) ON DELETE SET NULL,
    turn_id           uuid,
    outcome           text NOT NULL DEFAULT 'not_scored'
                      CHECK (outcome IN ('pass', 'fail', 'error', 'not_scored')),
    reason_code       text,
    extracted_value   text,
    answer_text       text,
    judge_verdict     text CHECK (judge_verdict IS NULL OR judge_verdict IN ('pass', 'fail')),
    judge_rationale   text,
    refused_writes    int NOT NULL DEFAULT 0,
    -- The platform tools' share of refused_writes; the rest is the gateway's `eval_read_only`
    -- request-log rows, reconciled when the run finishes (the log is flushed in batches).
    tool_refusals     int NOT NULL DEFAULT 0,
    started_at        timestamptz,
    finished_at       timestamptz,
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT eval_results_position_key UNIQUE (run_id, position)
);
CREATE INDEX eval_results_conversation_idx ON eval_results (conversation_id) WHERE conversation_id IS NOT NULL;
CREATE INDEX eval_results_turn_idx ON eval_results (turn_id) WHERE turn_id IS NOT NULL;
