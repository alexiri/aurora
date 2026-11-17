-- P4-011B-2 (D-265(b)/(c), C-828/C-829): the parked-approval reminder e-mail is its own idempotent
-- claim, and rule / ask timeouts may reach `approvals.park.max_timeout_secs` (up to 7 days) while
-- parking is on. Additive: existing claims become `initial`; the CHECKs only widen. The instance
-- maximum in force is still checked at authoring.
ALTER TABLE approval_notifications
    ADD COLUMN stage text NOT NULL DEFAULT 'initial',
    ADD CONSTRAINT approval_notifications_stage_ck CHECK (stage IN ('initial', 'reminder'));

ALTER TABLE approval_notifications DROP CONSTRAINT approval_notifications_pkey;
ALTER TABLE approval_notifications ADD PRIMARY KEY (approval_id, user_id, stage);

ALTER TABLE policy_rules DROP CONSTRAINT policy_rules_approval_timeout_ck;
ALTER TABLE policy_rules ADD CONSTRAINT policy_rules_approval_timeout_ck CHECK (
    approval_timeout_secs IS NULL
    OR (require_approval AND approval_timeout_secs >= 60 AND approval_timeout_secs <= 604800)
);

ALTER TABLE grants DROP CONSTRAINT grants_ask_timeout_ck;
ALTER TABLE grants ADD CONSTRAINT grants_ask_timeout_ck CHECK (
    ask_timeout_secs IS NULL
    OR (ask_timeout_secs >= 60 AND ask_timeout_secs <= 604800)
);
