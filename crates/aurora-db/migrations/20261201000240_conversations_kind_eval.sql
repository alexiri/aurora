-- P4-EVAL-2 (C-860): conversations gain the kind `eval` (one per question of one eval run,
-- external_ref `eval:<run_id>:<position>`). The unique (agent_id, kind, external_ref) index is
-- unchanged.
ALTER TABLE conversations DROP CONSTRAINT conversations_kind_check;
ALTER TABLE conversations
    ADD CONSTRAINT conversations_kind_check
    CHECK (kind IN ('direct', 'group', 'schedule', 'webhook', 'watch', 'peer', 'api', 'eval'));

-- (C-868, SEC-114) The gateway refuses every write of an eval turn; the request log records it as
-- its own decision, which the run's `refused_writes` counts. Partitions inherit the constraint.
ALTER TABLE request_log DROP CONSTRAINT request_log_decision_check;
ALTER TABLE request_log ADD CONSTRAINT request_log_decision_check CHECK (decision IN (
    'allow', 'blocked', 'default_denied', 'egress_denied', 'governance_refused',
    'rate_limited', 'approval_denied', 'approval_expired', 'approved', 'guard_refused', 'error',
    'approval_parked', 'eval_read_only'));
