# P2-DBL-1 and P2-DBL-2: Gateway database lane (phase 2)

## Goal
Agents use PostgreSQL databases without holding a database password, under the same policy, approvals, rate limits and audit as HTTP (`21 §Phase 2 — Database lane`). The seams already exist: the manifest `database` section, connection credentials, the tool catalogue and the policy engine.

## Linked requirements
- `docs/21-later-phases.md` §Phase 2 — Database lane: **the specification**. Implement every bullet exactly as written.
- `docs/06-gateway.md` (proxy port, guarded dial, CA, refusal guidance), `docs/07-policy-approvals.md` (rules, approvals, summaries), `docs/09-app-plugins.md` §3 (`database` section of the manifest), `docs/08-secrets-credentials.md` (connection credentials), `docs/11` / `docs/12` (home sync files for P2-DBL-2), `docs/17-security.md` (add the invariant below), `docs/18-testing.md`.

## P2-DBL-1 — the wire lane (gateway)
- **Sniffing** on the proxy port: a Postgres `SSLRequest` (or `StartupMessage`) as the first bytes enters the database lane. This must coexist with the TLS proxy listener (P1-004), the PROXY protocol (P2-025: the PROXY header comes first) and the open lane.
- **TLS** with a leaf for `<connection_id>.db.aurora.internal` from the gateway CA.
- **Login**: user = connection id, password = the agent's gateway credential (cleartext over TLS only). Resolve the credential and check that the connection is selected for the agent (grant). All admission checks that HTTP gets apply too: BYO posture and source CIDRs (P2-025), freeze (P2-006, if merged), the speaker for `user` rules (P2-005). Then open the real server connection through the guarded dial with the stored credentials (TLS per `sslmode`, optional `ca_pem`) and relay the authentication outcome.
- **Classification** with the `pg_query` crate, off the async runtime: bounded memory, 1 MiB statement cap, classes `read` / `write` / `ddl` / `admin` / `transaction`, a pure-function allow-list, strictest-of for multi-statement strings, and the **always refused** list. Unparsable → refused.
- **Policy** through the synthetic request `POST https://<connection_id>.db.aurora.internal/<class>`, so grants, rules, approvals (the summary shows the statement, clamped, with literals redacted), rate limits and `request_log` work unchanged. A refusal is an `ErrorResponse` with SQLSTATE `42501` and the guidance message.
- **Protocol state**: track prepared statements and portals so the server stays consistent after a refusal. Cancel requests use gateway-generated keys mapped to server keys that never leave the gateway.
- **Engine-agnostic core** (policy mapping, approval, audit, dial) separated from the Postgres wire adapter.
- **Dependencies**: `pg_query` builds libpg_query (C). Check that `cargo deny` accepts its license and that the release image and CI build it. Report the compile-time cost, and keep it out of crates that don't need it.
- **Tests**: unit tests for classification (a corpus file like `crates/aurora-policy/tests/corpus/*.yaml`, covering every class, every always-refused form and multi-statement strings); gateway integration tests against a real Postgres (the testkit DB) for login, grant refusal, read allowed, write needing approval, DDL refused, cancel mapping, and state after a refused prepared statement; one e2e scenario in `scripts/e2e/gateway.sh`.
- **Security**: add the invariant to `docs/17` (next free SEC number): an agent never receives a database password, and every statement is classified and policy-checked before it reaches the server. Add its proving tests to `docs/tests/security-coverage.md`.

## P2-DBL-2 — sandbox files and the PostgreSQL package (after DBL-1 merges)
- Home sync writes `PGSERVICEFILE`, `PGPASSFILE` and `PGSSLROOTCERT` per granted database connection, so `psql service=<label>` works with no secret on the command line. The password file holds the gateway credential, not the database password.
- A `postgresql` built-in package under `plugins/builtin/` (follows `P2-CAT-0`; tools `read`, `write`, `ddl`, `admin` mapped to the class paths; guide and fixtures).
- Dashboard: what the connection page needs to show for a database connection (the service name, how to connect), using the stage-1 UI primitives.
- e2e: `hosted.sh` gains a scenario where a hosted agent runs `psql service=<label> -c 'select 1'` and a write needs approval.

## Constraints / file ownership
- DBL-1 owns the gateway database lane (new modules under `services/aurora-gateway/src/db/` or similar), the classification crate or module, its tests, the `docs/06` and `docs/17` additions, contracts **C-220..C-229** and the security-coverage row. Touch the proxy listener only where the sniffing needs it.
- DBL-2 owns the home-sync file generation (API/runner/supervisor), the `postgresql` package, the dashboard pieces, contracts **C-230..C-239**.
- Migrations additive only; ask through Orca `ask` before anything destructive or before changing the manifest contract.

## Acceptance criteria
- [ ] Every bullet of `21 §Phase 2 — Database lane` is implemented or reported explicitly.
- [ ] Classification corpus, gateway integration tests and the e2e scenario pass.
- [ ] fmt, clippy `-D warnings`, `cargo test` for touched crates, `cargo deny check`, guards and the OpenAPI drift test pass.

## Suggested agent/model
Sonnet, own worktree, one lane at a time (DBL-1, then DBL-2).
