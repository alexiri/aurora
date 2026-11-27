-- FIX-253 (lane 2, D-490(b)): the peer task `waiting` state. A task whose receiving agent
-- registered a wake (an armed `process_watch`, a one-time schedule created from the task
-- conversation, or a pending approval) is `waiting` — not cancelled at the end of the receiving
-- turn — until the wake resumes it (`open`) or an existing limit ends it. `waiting` holds no
-- worker and no sandbox: it is a state in the database.
ALTER TABLE peer_tasks DROP CONSTRAINT peer_tasks_status_check;
ALTER TABLE peer_tasks ADD CONSTRAINT peer_tasks_status_check
    CHECK (status IN ('queued', 'open', 'waiting', 'completed', 'cancelled', 'expired'));
