# P4-MCON-4: Connector-host registration honours `plugins.allow_unsigned` (development parity)

## Problem (P4-MCON-2 finding)
`POST /v1/connector-host/register` (C-455) verifies every type's manifest signature and drops a type whose signature is empty or invalid. Release images carry signatures made with the project key (the `connector-host-signed` Containerfile stage; CI's `release` environment holds `AURORA_PROJECT_SIGNING_KEY`). Locally built images (kind-dev, `first-run.sh`, `kind.sh` without the test key) carry none, so a development install lists **no** connector types, although `plugins.allow_unsigned` already accepts unsigned self-hosted connector manifests (`docs/operator/plugins-and-connectors.md`). `13 §3.1` assumed development relies on `plugins.allow_unsigned`; the API never implemented that for the host path.

## Decision (coordinator, D-142)
Parity with the self-hosted path: with `plugins.allow_unsigned = true` (development only, warned at startup), a host type with an **empty** signature is accepted into the catalogue and marked `signed: false`; a **present but invalid** signature, or `package.id ≠ type_id`, or an invalid manifest is still rejected. With the flag off (the default, and every production install), nothing changes: unsigned types are rejected with `reason: "signature"`. Managed connectors of an unsigned type behave like any other, and the dashboard shows "Unsigned (development)" on the type card and the connector.

## Scope
1. API (`services/aurora-api/src/domains/connectors/host.rs` and the shared verification): the rule above; the catalogue item (C-450 `ConnectorType`) gains `signed: bool`; a warning log once per host registration that lists the unsigned types (no secrets).
2. Contract: amend C-450/C-455 in `docs/API_CONTRACTS.md` and `13 §3.1` (one sentence each).
3. Web: the type card and the connector card show an "Unsigned (development)" badge when `signed` is false (shared `Badge`, tokens only).
4. Docs: `docs/operator/install-kind-dev.md` and `plugins-and-connectors.md`: replace the "lists no types" caveat with the new behaviour.
5. OpenAPI and `schema.d.ts` regenerated.

## Acceptance
- API tests in `tests/it/connectors_managed.rs`: unsigned type rejected with the flag off; accepted and `signed: false` with it on; invalid signature rejected even with the flag on; id mismatch rejected with the flag on.
- Vitest for the badge.
- fmt, clippy `-D warnings`, `cargo test -p aurora-api --test it connectors`, web typecheck/lint/test/build, OpenAPI drift test, guards.

## Constraints
- Never accept a present-but-invalid signature. Never weaken the default (flag off).
- Change files only with the Edit/Write tools; never python, sed, awk, perl, `cat >>` or heredocs. No `scripts/perf/*`.
