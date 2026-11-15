# P5-UX-2: Connect plan and connect-with-grant, backend (owner walkthrough, 2026-11-15)

## Goal

The owner walked through a first sign-in and found that connecting an agent to anything is "entirely too
complicated". The specific problems were these:

- GitHub's recommended method asked a member for an App ID and a `.pem` private key.
- Linear and Notion failed with "No usable OAuth client is configured" only after the whole form.
- A wrong token answered "The connection probe failed.", with no reason.
- A connection and its grant were two separate acts, so it was easy to leave with neither done.

This task gives the server what the connect journey (P5-UX-3) needs:

- a **connect plan** that says which methods are ready for this person and who unblocks the rest;
- manifest keys for **audience**, **limits** and **scope presets**;
- **connect with a grant** in one request, including across an OAuth sign-in;
- refusals that say **why**.

## Linked requirements

- D-337, D-321, D-322; SEC-130.
- Contracts **C-955, C-956, C-957, C-958, C-959**.
- Normative: `14 §First run and connecting` §1 (what the dashboard needs); `09 §3.4` (journey keys) and
  `§11` (OAuth client resolution); `05` apps rows; `17` SEC-130.
- Builds on D-306 / FIX-153 (link builder `access_url`, agent page by id) and FIX-154 (connection
  inside the grant, `grant_connection_in_tx`). Unchanged: C-410, C-504, C-740, C-741, C-842, I-29.

## Placement in the new structure (D-342)

- **Dashboard paths.** Every path this task emits follows the C-960 routes of
  `14 §Information architecture`: the `Unlock.admin_path`, the gateway's `connect_url` and
  `connect_url_workspace`, and the journey deep link. One server-side path table mirrors
  `web/src/lib/routes.ts`.
- **Wording.** `Unlock` sentences use the vocabulary: "organization admins", "Administration", "OAuth
  client".

## Context

- **Connect service:** `services/aurora-api/src/domains/connect/{routes,service,flow,page}.rs`.
  - The OAuth flow object is sealed and user-bound, sent as the `state` parameter plus a cookie and
    opened by `Flow::open` (`service.rs` ~1260).
  - The callback is `run_callback`.
  - `oauth_client_required` and `instance_oauth_client_required` are raised at `service.rs` ~226–239,
    and `connect_probe_failed` at ~51.
- **OAuth client resolution:** `crates/aurora-db/src/repo/connections.rs` ~366, ordered workspace, org,
  instance, enabled only, `instance_id IS NOT DISTINCT FROM`. The plan **must call the same query**; do
  not write a second resolver.
- **Grants:** `domains/grants/{routes,service}.rs`.
  - `grant_connection_in_tx` and `write_connection_grant` come from FIX-154.
  - The bulk grant checks are C-504.
- **Gateway link builder:** C-891, in the gateway (FIX-153).
- **`request_app_connection`:** `domains/tools/apps.rs` ~250.
- **Registry snapshot and manifest model:** `crates/aurora-registry` (`manifest/model.rs`, `snapshot.rs`);
  linter at `09 §6`.

## Scope

1. **Manifest keys (C-956).** Add to the manifest model, the snapshot and the linter:
   - `connect[].audience` (`person`|`admin`, default `person`);
   - `connect[].fields[].limit` (bool; refused on a required field);
   - `connect[].scope_presets` (`read`/`write` lists; each a method scope, and `read` only `access: read`).

   Old packages stay valid (a golden test over every builtin and fixture). Package edits:
   - `plugins/builtin/github/plugin.yaml`: the `github_app` hook method gets `audience: admin`;
   - `plugins/builtin/linear/plugin.yaml`: `scope_presets.write: [read, write]`.

   Bump each package's patch version the way the repo does for package changes, and run
   `check-builtin.sh`.
2. **Connect plan (C-955).** Add `GET /v1/apps/{app}/connect-plan?workspace_id=` in `domains/apps` (or
   `domains/connect`, wherever `GET /v1/apps/{app}` lives). It needs `can_own` of the workspace.
   Compute, per C-955:
   - `available` with `unlock`, using the posture check the connect service already uses;
   - each method's `ready` / `blocked { reason, unlock }`;
   - `suggested_method`, `presets`, `limits` and `existing`.

   The `Unlock.people` are the display names of up to three active org owners/admins, owners first, then
   by name. Never e-mails, never instance admins. `admin_path` for `register_oauth_client`:
   - the org's Apps & services › OAuth clients tab
     (`/o/<org>/apps?tab=oauth-clients&package=<pkg>&client_key=<key>`, C-960) when `who = org_admin`;
   - the workspace's Apps & services › OAuth clients tab (`…/apps?tab=oauth-clients&…`) when
     `caller_can` and the caller is a workspace owner who would register a workspace client;
   - Administration › Apps & services › OAuth clients (`/admin/apps?tab=oauth-clients&…`) for
     instance-bound apps (`who = instance_admin`).

   Paths come from one server-side table that mirrors C-960. The gateway link builder (C-891) uses the
   same paths.

   Add it to the OpenAPI document and regenerate `web/openapi.json` and `schema.d.ts`.
3. **Connect with a grant (C-957, SEC-130).** Extend `ConnectRequest` with `grant`.
   - **Before any write or redirect:** validate the agents like C-504 and refuse `full` on `custom_only`.
     Expand the preset server-side from the snapshot's tools (`read`, `ask_writes`, `full`). Put the
     expansion in `domains/grants` so P5-UX-4 and the tool path can reuse it.
   - **`connected`:** create the connection and the grants in one transaction. Reuse FIX-154's
     `write_in_tx` / `grant_connection_in_tx`. Audit `via: "connect_journey"`. The answer adds `grants`.
   - **`redirect`:**
     - Add `grant` to the sealed `Flow` (agent ids and preset only).
     - At the callback, after the exchange and inside the connection's transaction, re-authorize (the
       caller's `can_own`, agents visible and in the workspace, `custom_only`) and write the grants.
     - On failure, commit the connection without grants, and put `grant_failed { code }` in the callback
       page's `BroadcastChannel` message (C-842) next to the new `granted_agent_ids`.
   - **Reconnect** ignores `grant` (422 `grant_not_allowed_on_reconnect` if sent).
4. **Refusals (C-958).**
   - `connect_probe_failed` gets `details.cause` and `details.host`. Classify in the probe runner:
     401/403 → `rejected`; DNS, connect, TLS or timeout → `unreachable`; anything else → `unexpected`.
   - `oauth_client_required` and `instance_oauth_client_required` get `details.unlock` from the same plan
     function.
   - Dashboard-reachable `forbidden` refusals get `details.required_role`. Do this in the shared helpers
     (`require_admin`, `require_owner`, the `can_own` / `can_approve` checks), not route by route.
5. **Links (C-959).**
   - Add `connect_journey_url(agent, app)` to the gateway builder.
   - Use it for `connect_url` wherever the refusal knows the agent; keep the workspace URL in `details`
     as `connect_url_workspace`.
   - `request_app_connection` links to it. A credential-free app keeps `access_url` (FIX-154).
   - Update the tests that assert these URLs.

## Constraints / file ownership

- Owns: `crates/aurora-registry` (manifest keys, linter); `services/aurora-api/src/domains/{connect,
  apps}` and the grant-preset expansion in `domains/grants`; the forbidden-details helper; the gateway link
  builder and its messages (`services/aurora-gateway`, messages only); `domains/tools/apps.rs` (link
  only); `plugins/builtin/{github,linear}/plugin.yaml`; OpenAPI and the generated web types.
- No dashboard work. That is P5-UX-3, plus P5-UX-1 for `required_role` rendering.
- Not beside P5-GUIDE-1/3 (registry and manifest model) or P5-ICS (registry): run before or after them in
  the same lane.
- Rust conventions as in `02`: `cargo fmt --check`, clippy `-D warnings`, targeted tests per step. Full
  suites at the batch end.

## Acceptance criteria (each test seen failing first)

1. Linter:
   - `limit` on a required field → error;
   - a `scope_presets.read` entry that is a write scope or not a method scope → error;
   - every builtin and fixture still validates (golden list).
2. Connect plan ITs (`api/it/connect_plan.rs`):
   - **GitHub with no OAuth clients:** `github_app` is `audience: admin`; `oauth` and `oauth_app` are
     blocked `oauth_client_missing` with `who: org_admin` and the org admins' names; the suggested
     method is `token`.
   - **The same with an org client for `oauth`:** suggested `oauth`.
   - **The same with a workspace client:** `oauth` is ready.
   - **Linear with no client:** suggested `null`, with `unlock`.
   - **Restricted posture without the app:** `available: false`, `enable_app`.
   - **An instance-bound app with no instance:** `instance_missing`, `who: instance_admin`, no names.
   - A non-owner gets 403 with `required_role: workspace_owner`.
   - `existing` lists an active connection with its granted agents.
   - Instance admins are never named; e-mails appear nowhere in the answer.
3. Connect with grant ITs:
   - **`fields`:** connection plus grants in one transaction; a forced grant failure rolls back both.
   - **Presets:** each expands to the expected `allow` / `ask` lists from the snapshot.
   - **`full` on `custom_only`:** 422, nothing written.
   - **OAuth (fake provider, the existing connect ITs' harness):** the grant is applied at the callback.
   - **Caller demoted between start and callback:** the connection is kept, there is no grant, and
     `grant_failed` is set.
   - **Agent deleted between start and callback:** the same.
   - **Tampered state:** `oauth_state_invalid`, no connection, no grant.
   - The state parameter and the cookie, base64-decoded, contain no agent id in clear.
   - Reconnect with `grant` → 422.
   - Without `grant`, behaviour is unchanged (the existing tests stay green).
4. Probe ITs: a fake upstream answering 401 gives `cause: rejected`; an unroutable host gives
   `unreachable` with `host`; a 500 gives `unexpected` with `upstream_status`.
5. Gateway: the refusals for an agent without the app carry the journey URL. `request_app_connection`'s
   notice links to the journey for an app that needs sign-in, and to `access_url` for a credential-free
   app.
6. `permission_matrix` rows for the new route; OpenAPI drift check green; `docs/tests/security-coverage.md`
   row for SEC-130.
7. `cargo fmt --check`, clippy, the targeted suites green; report the test counts.

## Dependencies

- FIX-153 and FIX-154 merged (they are).
- No migration.

## Suggested agent/model and reason

Sonnet (`rust-developer`). It touches several crates but each part is bounded. The OAuth re-authorization
needs care, and the reviewer should read SEC-130 against the callback code.
