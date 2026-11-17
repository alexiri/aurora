-- P4-EVAL-1 (D-292(b), C-863): the agent's eval settings. The judge key is an LLM secret granted to
-- the agent; it is checked again at every judge call (P4-EVAL-3), so a deleted secret only nulls the
-- reference (the model id then has no meaning and is ignored). Existing agents get no judge and
-- the regression notice on.
ALTER TABLE agents
    ADD COLUMN eval_judge_secret_id uuid REFERENCES secrets (id) ON DELETE SET NULL,
    ADD COLUMN eval_judge_model_id text,
    ADD COLUMN eval_regression_notify boolean NOT NULL DEFAULT true;
