# CONTRACT-001: Drop `users.password_hash` and `email_tokens` (first phase-3 task)

## Decision (owner, Q9, D-78, 2026-11-08)
The contract step of RFE-004's expand → migrate → contract: drop the column `users.password_hash` and the table `email_tokens`. They have been unused since phase 1 (OIDC-only sign-in, P1-005). The phase-1 migration `20261110000001_oidc_only_sign_in.sql` cleared the hashes and stopped every read and write.

The owner said this specific drop needs **no further confirmation**: there is no user data to lose.

## Scope
1. **One migration** (next timestamp after `20261116000001`): `ALTER TABLE users DROP COLUMN password_hash;` and `DROP TABLE email_tokens;`, plus its index if it isn't dropped with the table.
   - The migration header names the owner's decision.
   - It is **not** additive. Note that in the header, and that `upgrade.sh` covers the path.
2. **`docs/03`:**
   - remove `password_hash` from the `users` table and remove the whole `email_tokens` table section;
   - delete both rows from §Planned changes.
   - `scripts/extract-schema.py --check` and the aurora-db `schema_matches_spec` test must stay green. Update `crates/aurora-db/tests/fixtures/spec_schema.json` with the repo's generator, not by hand.
3. **Code and tests that still name them:**
   - `services/aurora-api/tests/it/admin.rs:114` (`password_hash IS NULL` in a select);
   - `services/aurora-api/tests/it/auth.rs:72` (counts non-null hashes, a phase-1 erase check that loses its meaning).
   Remove or rewrite these assertions honestly: the guarantee becomes "the column doesn't exist", which the schema test proves. Don't leave a test that can't fail.
   - Grep the whole repo (Rust, SQL, scripts, Helm, docs) for both names; only history (old migrations, decision records, `docs/90`) may keep them.
4. **`docs/15 §Upgrades`:** one line that the contract step for RFE-004 shipped in this release, and that a deployment must have run a phase-1+ release first. `upgrade.sh` from the phase-2 exit commit proves the path.

## Constraints
- **Owns:** the new migration, `docs/03` (the two tables and §Planned changes), the spec-schema fixture, the two api test files' lines, the `docs/15` line.
- No other schema change.
- No decision number needed; this is D-78.

## Acceptance criteria
- [ ] `cargo test -p aurora-db` (incl. `schema_matches_spec`), `cargo test -p aurora-api -- --test-threads=8`.
- [ ] `scripts/extract-schema.py --check`.
- [ ] `UPGRADE_STRICT=1 bash scripts/e2e/upgrade.sh` green, from the phase-0 base it uses (it must apply the drop).
- [ ] fmt, clippy `-D warnings`, `bash scripts/guards/run-all.sh`.
- [ ] `grep -rn "password_hash\|email_tokens"` shows only history.
