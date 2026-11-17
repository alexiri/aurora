-- P4-011B-1 (RFE-011b, D-133, D-264, D-265, SEC-74): approvals that park. A parked approval has
-- no holder: the request was refused at once and never sent; the row carries a salted
-- fingerprint of the identical request and, once approved, a single-use grant (`resume_state`).
-- Additive: every new column is nullable or has a default; no existing row changes meaning.
-- Taken after origin/master's newest migration (…000244); the block `000220..229` is reserved for
-- P4-011B. The gateway database role's grants (03 §approvals) are deployment-side, as for every
-- table: nothing to grant here.
ALTER TABLE approvals
    ADD COLUMN parked            boolean     NOT NULL DEFAULT false,
    ADD COLUMN fp_salt           bytea       NULL,
    ADD COLUMN fingerprint       bytea       NULL,
    ADD COLUMN resume_state      text        NULL,
    ADD COLUMN resume_expires_at timestamptz NULL,
    ADD COLUMN consumed_at       timestamptz NULL,
    ADD COLUMN resume_turn_id    uuid        NULL,
    ADD CONSTRAINT approvals_resume_state_ck
        CHECK (resume_state IS NULL OR resume_state IN ('ready', 'consumed', 'expired', 'revoked')),
    ADD CONSTRAINT approvals_park_fingerprint_ck
        CHECK ((parked = (fp_salt IS NOT NULL)) AND (parked = (fingerprint IS NOT NULL)));

-- Grant lookup (gateway, step 8), resume-window expiry (API), pending-per-agent limit and
-- de-duplication of identical parked requests (gateway).
CREATE INDEX approvals_ready_grant_idx ON approvals (agent_id, conversation_id)
    WHERE resume_state = 'ready';
CREATE INDEX approvals_ready_expiry_idx ON approvals (resume_expires_at)
    WHERE resume_state = 'ready';
CREATE INDEX approvals_parked_pending_idx ON approvals (agent_id, conversation_id)
    WHERE parked AND status = 'pending';
