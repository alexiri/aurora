-- P5-REFINE-20 batch 5 item 4 (C-1101): the last test of an agent's grader is kept, so a failed test
-- is still shown after a reload. Written with the judge (a changed judge replaces it, a cleared one
-- nulls it); display data only, never the provider's text.
ALTER TABLE agents
    ADD COLUMN eval_judge_test_result text NULL CHECK (eval_judge_test_result IN ('ok', 'failed')),
    ADD COLUMN eval_judge_test_reason text NULL,
    ADD COLUMN eval_judge_test_ms bigint NULL,
    ADD COLUMN eval_judge_tested_at timestamptz NULL;
