-- P3-013 (RFE-013, D-60, C-316): token use tracking and the stale / expiring views.
-- `last_used_at` is a churn column (<= 1 write per minute per credential, written by the gateway
-- and by the API on the agent-token routes); `expires_at` already exists. The gateway role's
-- `UPDATE (last_used_at)` grant is applied with the other role grants (`grants.rs`).
ALTER TABLE gateway_credentials ADD COLUMN IF NOT EXISTS last_used_at timestamptz;

-- The agent-token lists (C-313) scan the live `agent` credentials ordered newest first.
CREATE INDEX IF NOT EXISTS gateway_credentials_live_agent_idx
    ON gateway_credentials (created_at DESC, id DESC)
    WHERE kind = 'agent' AND revoked_at IS NULL;
