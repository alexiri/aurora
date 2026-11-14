-- FIX-121 (owner D-178 / Q-R5-10, D-211): rotating a runner's token keeps the previous token
-- valid for a grace period so the operator can roll the runner; after `previous_token_expires_at`
-- the old token is refused. At most one previous token is kept (a second rotation inside the
-- grace replaces it). Migration 20261201000101 (the lane's allocation).

ALTER TABLE runners
    ADD COLUMN previous_token_hash bytea CHECK (previous_token_hash IS NULL OR octet_length(previous_token_hash) = 32),
    ADD COLUMN previous_token_expires_at timestamptz,
    ADD CONSTRAINT runners_previous_token_pair_ck
        CHECK ((previous_token_hash IS NULL) = (previous_token_expires_at IS NULL));

CREATE UNIQUE INDEX runners_previous_token_hash_key ON runners (previous_token_hash)
    WHERE previous_token_hash IS NOT NULL;
