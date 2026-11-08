-- P1-005 (RFE-004, D-46): OIDC-only sign-in.
-- Expand step only: `users.password_hash` and `email_tokens` are no longer read or written by the
-- core; both are dropped by a later contract migration (docs/15 §Upgrades).

-- Last successful OIDC re-authentication of a session (docs/04 §OIDC "Re-authentication").
ALTER TABLE sessions ADD COLUMN reauth_at timestamptz;

-- No local credential exists any more. Irreversible for this column's data by design (D-46); the
-- column itself stays until the contract migration so a rolling upgrade's old replicas still work.
UPDATE users SET password_hash = NULL WHERE password_hash IS NOT NULL;
