-- P4-011B-1 (D-265(a), C-827): the request log names the approval a request was held, parked or
-- resumed under, and records a parked refusal as its own decision. Both statements run on the
-- partitioned parent; partitions inherit the columns and the constraint. `approval_id` has no
-- foreign key (ledger table, history survives).
ALTER TABLE request_log
    ADD COLUMN approval_id uuid NULL,
    ADD COLUMN resumed boolean NOT NULL DEFAULT false;

ALTER TABLE request_log DROP CONSTRAINT request_log_decision_check;
ALTER TABLE request_log ADD CONSTRAINT request_log_decision_check CHECK (decision IN (
    'allow', 'blocked', 'default_denied', 'egress_denied', 'governance_refused',
    'rate_limited', 'approval_denied', 'approval_expired', 'approved', 'guard_refused', 'error',
    'approval_parked'));
