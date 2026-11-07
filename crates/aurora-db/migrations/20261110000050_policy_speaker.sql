-- P2-005 (RFE-005, D-49): policy `user` identities bind to the turn's speaker. Additive and
-- data-preserving: existing credentials and request-log rows simply carry no speaker (so a
-- phase-0 `user` rule stops matching their traffic, the intended narrowing).
-- The person who authored the turn of a hosted context credential (null for every other
-- credential). No FK: revocation on user delete is handled by the resolver's can_use check.
ALTER TABLE gateway_credentials ADD COLUMN speaker_user_id uuid NULL;
-- The request identity's speaker; no FK (ledger table).
ALTER TABLE request_log ADD COLUMN speaker_user_id uuid NULL;
