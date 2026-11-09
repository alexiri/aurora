-- CONTRACT-001: the contract step of RFE-004's expand -> migrate -> contract (owner decision Q9,
-- D-78, 2026-11-08): drop `users.password_hash` and the `email_tokens` table (its index
-- `email_tokens_user_id_idx` goes with the table).
--
-- NOT additive: this migration destroys data. It is approved because there is none to lose:
-- sign-in is OIDC-only since phase 1 (P1-005), `20261110000001_oidc_only_sign_in.sql` cleared
-- every hash and stopped every read and write. A deployment must have run a phase-1+ release
-- before this one (15 §Upgrades). `scripts/e2e/upgrade.sh` covers the path.
ALTER TABLE users DROP COLUMN password_hash;
DROP TABLE email_tokens;
