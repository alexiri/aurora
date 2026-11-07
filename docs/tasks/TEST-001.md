# TEST-001: Schema introspection diff against `03` (architecture guard)

## Goal
CI guard (`18 §Architecture guards`): every table in `03` exists in migrations and vice versa, with the same columns.

## Linked requirements
- M0/03, `18 §Architecture guards` ("Every table in 03 exists in migrations and vice versa (schema introspection diff against a machine-readable extract of 03)"), `19 §M0` acceptance ("migrate on an empty database yields a schema that passes the 03 introspection diff").

## Context
- `docs/03-data-model.md`: tables are `### \`name\`` headings (some with suffix text like ` — *no FKs*`) followed by a markdown table `| Column | Type | Notes |`. Rows whose first cell is `unique`, `partial unique`, `index`, `check` or similar are constraints, not columns. `migrations` / `_sqlx_migrations` is managed by sqlx — exclude.
- `crates/aurora-db` exposes `MIGRATOR`; DB tests use `#[sqlx::test(migrator = "aurora_db::MIGRATOR")]` (fresh DB per test; `DATABASE_URL` in repo-root `.env`).

## Scope
### In scope
- `scripts/extract-schema.py` (Python 3 stdlib only): parse `docs/03-data-model.md` → `crates/aurora-db/tests/fixtures/spec_schema.json` `{ "tables": { "<table>": ["col", …] } }` (sorted). Checked in; the test fails if regenerating changes it (run the script from the test via `python3`, or have the Rust test parse the markdown directly — choose one approach, document it).
- `crates/aurora-db/tests/schema_matches_spec.rs`: on a migrated test DB, read `information_schema.columns` for schema `public` (excluding `_sqlx_migrations` and partitions of `request_log` — use `pg_class.relispartition`), compare table sets and column sets with the fixture; failure message lists missing/extra tables and columns precisely.
- Views created by grants are not tables — exclude views.

### Out of scope
- Comparing types/nullability (may be added later); fixing migrations (report mismatches to the orchestrator instead).

## Acceptance criteria
- [ ] `python3 scripts/extract-schema.py --check` exits 0 when fixture is current, non-zero when stale.
- [ ] `cargo test -p aurora-db --test schema_matches_spec` passes, or fails with an exact mismatch list that you report (do NOT edit migrations).

## Dependencies
- Blocked by: BE-005. Unblocks: OPS-002 (CI guard step).

## Suggested agent/model
haiku-tester (bounded test + small script). Escalate to rust-developer if markdown parsing proves ambiguous.
