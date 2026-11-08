# P2-INST: Self-hosted app instances (phase 2, D-74)

## Goal
Let one app package serve both its SaaS host and any number of self-hosted deployments (the owner runs its own GitLab). Admins register an **app instance** (a host for one app); connections and BYOC OAuth clients bind to it; the gateway injects an instance-bound connection only on that exact host. Then make the GitLab built-in package instance-bound.

## Linked requirements
- Decision **D-74** (`docs/90-decisions.md`).
- `docs/09-app-plugins.md` §3.4 "Instance-bound hosts: self-hosted apps", §6 (validation rules for `{instance}`), §7 (registered hosts are claims).
- `docs/06-gateway.md` §MITM lane: connect-time resolution, step 2 (instance lookup) and the `host_mismatch` row.
- `docs/05-control-plane-api.md` §App instances and the `connect` / BYOC lines; **`docs/API_CONTRACTS.md` §Phase 2 — Self-hosted app instances (C-240..C-246)** — the wire contract; implement it exactly.
- `docs/03-data-model.md` §Planned changes: `app_instances`, `app_connections.instance_id`, `app_oauth_clients.instance_id` — make them real migrations (additive), move the rows into the machine-checked tables, regenerate the fixture (`python3 scripts/extract-schema.py`).
- `docs/17-security.md` SEC-59 (new) and the amended SEC-04: add the proving tests and the `docs/tests/security-coverage.md` row.

## Scope
1. **Manifest and registry**: `instances` block and the `{instance}` placeholder in the manifest model, linter rules (`09 §6`), snapshot compilation (instance-bound host entries kept separate from the static host index; `default_host` compiled as an ordinary static claim), `aurora plugin validate|test` support (fixtures may set an instance host).
2. **API**: the `app_instances` resource and routes (C-240..C-242) with the validation and conflict checks (static claims from the active snapshot, governed LLM hosts, other instances across visible scopes), the optional probe through the destination guard, audits in the same transaction, invalidation; `connect` and BYOC changes (C-243, C-244); `GET /v1/apps/{app}` `instances` (C-245); the permission-matrix rows.
3. **Gateway**: per-org instance list (cached, `org` invalidation scope), resolution step 2 extension, exact-host injection, `host_mismatch` for an instance-bound connection elsewhere (C-246); hook egress and probe resolve `{instance}` to the connection's instance host.
4. **Dashboard**: an "Instances" admin section for instance-bound apps (org settings for owners/admins, `/admin` for instance-wide), the instance picker in the connect dialog, the instance shown on connection rows. Follow `docs/14 §Visual design` (tokens, shared primitives and page patterns, both themes, 390 px).
5. **GitLab package**: after P2-CAT-0 merges, add `instances` (`label: GitLab instance`, `default_host: gitlab.com`, probe `GET /api/v4/version`) and switch its hosts/tools/OAuth URLs to `{instance}`; fixtures for both gitlab.com and a registered instance host; update its guide (registering a self-managed instance, creating the OAuth application on it, `gateway.allowed_private` for a private address).
6. **Tests**: everything SEC-59 lists; e2e: a scenario in `scripts/e2e/gateway.sh` with a fake self-hosted app on a test host (an instance-bound fixture package under `plugins/fixtures/`) proving injection on the instance host only.

## Constraints / file ownership
- You own the manifest/registry changes for `instances`, the `app_instances` migration and repo, the API routes above, the gateway resolution change, the dashboard pieces, the GitLab manifest change, the new fixture package, tests and docs rows. Contract numbers: only C-240..C-249 (C-240..C-246 are already written; add C-247.. only for genuinely new wire details and say so).
- **Starts after P2-CAT-0 merges** (it creates `plugins/builtin/gitlab` and the built-in scan).
- P2-006 (freeze) may run in parallel and touches gateway refusal: keep the gateway change confined to resolution step 2 and injection matching.
- Never weaken SEC-04: the exact-host rule must hold for every path (request, probe, hooks, OAuth token exchange).

## Acceptance criteria
- [ ] C-240..C-246 implemented exactly; OpenAPI drift test and permission matrix (0 inconclusive) pass.
- [ ] SEC-59 proving tests pass and are listed in `docs/tests/security-coverage.md`.
- [ ] GitLab works against gitlab.com (default) and a registered instance in fixtures.
- [ ] fmt, clippy `-D warnings`, `cargo test` for touched crates, `cargo deny`, guards, web typecheck/lint/vitest/build, and `scripts/e2e/gateway.sh` pass.

## Suggested agent/model
Sonnet, own worktree.
