-- FIX-253 (security review, high): a `waiting` peer task occupies the one-open slot of its
-- ordered pair exactly like an `open` one. The partial index below counted only `open`, so while
-- a task was `waiting` a second task of the same pair could open beside it — and when the first
-- task's wake resumed it (`waiting` → `open`), the resume violated this index: the wake's
-- transaction failed and the waiting task became a zombie until its expiry. Safe on a populated
-- table: `waiting` is new in this lane (20261201000286), so no existing row carries it.
DROP INDEX peer_tasks_one_open_uq;
CREATE UNIQUE INDEX peer_tasks_one_open_uq ON peer_tasks (from_agent_id, to_agent_id)
    WHERE status IN ('open', 'waiting');
