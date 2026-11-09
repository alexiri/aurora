-- (RFE-024, 03 §channel_ops) Self-contained `presence.deprovision` ops hold a sealed credential
-- copy in `payload.credentials_ref` until they finish. The expiry and pruning jobs look for
-- exactly these rows: still-running ops past the retry window, and finished ops whose copy has
-- not been discarded yet.
CREATE INDEX channel_ops_deprovision_idx ON channel_ops (kind, status)
 WHERE kind = 'presence.deprovision'
   AND (status IN ('pending', 'claimed') OR payload->>'credentials_ref' IS NOT NULL);
