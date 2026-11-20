-- P5-REFINE-9 (D-415; C-1083, C-1084, SEC-143): the agent timeline reads each of its sources
-- newest first, bounded by agent and time (C-1083 "a source query that cannot be bounded by agent
-- and time is a defect"). Before this, turns, approvals, action approvals, peer tasks and access
-- requests had no (agent, time) index, a turn's transcript events were found by scanning its
-- conversation, and the request log was indexed by workspace only.
--
-- Additive only. Plain CREATE INDEX, not CONCURRENTLY: migrations run inside a transaction here.
-- `transcript_events` and `request_log` are the two large tables; the request-log indexes are
-- partial (a turn's rows by turn id; the rows without a turn by agent and time).

CREATE INDEX IF NOT EXISTS turns_agent_created_idx
    ON turns (agent_id, created_at DESC, id DESC) WHERE role = 'turn';

CREATE INDEX IF NOT EXISTS transcript_events_turn_idx
    ON transcript_events (turn_id, seq) WHERE turn_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS approvals_agent_created_idx
    ON approvals (agent_id, created_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS action_approvals_agent_created_idx
    ON action_approvals (agent_id, created_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS peer_tasks_from_created_idx
    ON peer_tasks (from_agent_id, created_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS peer_tasks_to_created_idx
    ON peer_tasks (to_agent_id, created_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS agent_access_requests_agent_created_idx
    ON agent_access_requests (agent_id, created_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS request_log_turn_idx
    ON request_log (turn_id) WHERE turn_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS request_log_agent_noturn_idx
    ON request_log (agent_id, ts DESC) WHERE turn_id IS NULL;
