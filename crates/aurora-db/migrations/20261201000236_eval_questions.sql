-- P4-EVAL-1 (D-289, C-861, SEC-116): the questions an agent is evaluated with. A question is one of
-- three kinds; the fields of the other kinds stay null (the CHECKs). The expected answer is
-- agent-audience data: it is never copied to audit, logs or the sandbox. No foreign key to runs: a
-- run snapshots the question and its expected answer in `eval_results` (P4-EVAL-2). The gateway
-- database role gets nothing on this table.
CREATE TABLE eval_questions (
    id              uuid PRIMARY KEY,
    agent_id        uuid NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    workspace_id    uuid NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    position        int NOT NULL CHECK (position >= 1),
    question        text NOT NULL CHECK (char_length(question) >= 1),
    expected_kind   text NOT NULL CHECK (expected_kind IN ('numeric', 'text', 'judged')),
    expected_number numeric,
    tolerance       numeric CHECK (tolerance IS NULL OR tolerance >= 0),
    expected_text   text,
    criteria        text,
    enabled         boolean NOT NULL DEFAULT true,
    version         int NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_by      uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    -- Deferrable so a move or a compaction shifts several positions in one statement.
    CONSTRAINT eval_questions_position_key UNIQUE (agent_id, position) DEFERRABLE INITIALLY IMMEDIATE,
    CONSTRAINT eval_questions_kind_fields CHECK (
        (expected_kind = 'numeric' AND expected_number IS NOT NULL
            AND expected_text IS NULL AND criteria IS NULL)
        OR (expected_kind = 'text' AND expected_text IS NOT NULL
            AND expected_number IS NULL AND tolerance IS NULL AND criteria IS NULL)
        OR (expected_kind = 'judged' AND criteria IS NOT NULL
            AND expected_number IS NULL AND tolerance IS NULL AND expected_text IS NULL)
    )
);
CREATE INDEX eval_questions_workspace_idx ON eval_questions (workspace_id);
