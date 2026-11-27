-- FIX-253 (security review, high): a `waiting` peer task occupies the one-open slot of its
-- ordered pair exactly like an `open` one. The partial index below counted only `open`, so while
-- a task was `waiting` a second task of the same pair could open beside it — and when the first
-- task's wake resumed it (`waiting` → `open`), the resume violated this index: the wake's
-- transaction failed and the waiting task became a zombie until its expiry. Safe on a populated
-- table: `waiting` is new in this lane (20261201000286), so no existing row carries it.
DROP INDEX peer_tasks_one_open_uq;
CREATE UNIQUE INDEX peer_tasks_one_open_uq ON peer_tasks (from_agent_id, to_agent_id)
    WHERE status IN ('open', 'waiting');
-- FIX-253 (security review, medium 3): how many times the task was woken. A task is woken a
-- bounded number of times without a report, so an agent cannot chain waits until its expiry.
ALTER TABLE peer_tasks ADD COLUMN wakes_used integer NOT NULL DEFAULT 0 CHECK (wakes_used >= 0);
-- FIX-253 (security review, medium 5): what the idle harness child spends after a turn is added
-- to the usage ledger. This is the number of the last idle usage event counted for the turn, so a
-- redelivered report counts nothing twice. No CHECK: `turns` is large, and adding one scans it
-- under an exclusive lock (migrations run in a transaction, so NOT VALID would not avoid that).
-- The only writer moves the number forward from 0.
ALTER TABLE turns ADD COLUMN idle_usage_n bigint NOT NULL DEFAULT 0;
