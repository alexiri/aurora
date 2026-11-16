# P5-UX-2: Connecting, backend: setup status, connect with access, who can fix it (owner, 2026-11-15)

## Goal

The owner's walkthroughs found that connecting an agent to anything was a maze:
- GitHub's recommended method asked a member for an App ID and a private key.
- Linear and Notion failed with "No usable OAuth client is configured" only after the whole form.
- A wrong token answered "The connection probe failed.", with no reason.
- Making the account and giving the agent access were two separate acts, so it was easy to leave with
  neither done.
- Nothing said who could fix a missing piece.

This task gives the server what the journey (P5-UX-3), the setup page (P5-UX-9) and every "who can fix
this" sentence need:
- one **setup status** per app, which says which ways to connect work here, what is missing and who can
  fix it;
- **connect with access**: the account and the agents' access in one request, including across an OAuth
  sign-in;
- the same for a **website or an AI key**;
- refusals that say **why** and **who can help**.

## Linked requirements

- **Design** (D-354): the canvas “Aurora — agents, accounts and access”, https://claude.ai/artifact/3UwmUgrhh6MLCsfqdhTFmQ, boards none of its own (backend); its data drives Agent › Give Daneel an app and Workspace › Apps & accounts › Linear. Read each board's note; where a board and the spec disagree, the spec wins.
- D-321, D-322, D-330, D-344, D-349; SEC-130.
- Contracts **C-931, C-946 (API), C-955, C-956, C-957, C-958, C-981 (server), C-988**.
- Normative: `14 §Agents, accounts and access` §3 and §4, `14 §Information architecture` "Who can fix
  this", `09 §3.4` (journey keys) and `§11` (OAuth client resolution), `05` (apps, secrets, org and
  workspace rows), `17` SEC-130.
- Builds on FIX-153 (agent page by id), FIX-154 (connection made inside the grant,
  `grant_connection_in_tx`). Unchanged: C-410, C-504, C-740, C-741, C-842, I-29.

## Wave and order

- **Wave 0**, in the Rust lane after FIX-159 (both edit `crates/aurora-registry`).
- P5-UX-13 (registry lints, builtin packages) starts after this task.
- Consumers: P5-UX-4 (access levels), P5-UX-3 (journey), P5-UX-9 (setup page), P5-UX-1 (`WhoCan`
  component), P5-UX-6 and P5-UX-7.

## Context

- **Connect service:** `services/aurora-api/src/domains/connect/{routes,service,flow,page}.rs`.
  - The OAuth flow object is sealed and bound to the user, sent as the `state` parameter plus a cookie
    and opened by `Flow::open` (`service.rs` ~1260). The callback is `run_callback`.
  - `oauth_client_required` and `instance_oauth_client_required` are raised at `service.rs` ~226–239,
    `connect_probe_failed` at ~51. The probe runner is `connect/probe.rs`.
- **OAuth client resolution:** `crates/aurora-db/src/repo/connections.rs` ~366 (workspace, then
  organization, then instance; enabled only; `instance_id IS NOT DISTINCT FROM`). The setup status
  **must call the same query**.
- **Apps:** `GET /v1/apps/{app}` and `AppDetail.oauth_clients[]` with `redirect_uri` built from
  `config.external_url` (`domains/apps/service.rs` ~258–295).
- **Grants:** `domains/grants/{routes,service}.rs` (`grant_connection_in_tx`, the C-504 bulk checks).
- **Secrets:** `domains/secrets/` (generic and LLM create).
- **Role helpers:** `require_admin`, `require_owner`, the `can_own` and `can_approve` checks
  (`src/http/auth.rs` ~171 and the callers).
- **External id:** `organizations.external_id`, in `GET /v1/orgs/{org}` for any member
  (`tenancy/routes.rs` ~156–161). An app needs it when `permissions.context` includes `org_external_id`.
- **Registry:** `crates/aurora-registry` (`manifest/model.rs`, `snapshot.rs`); linter at `09 §6`.

## Scope

1. **Manifest keys (C-956).** Add `connect[].audience` (`person`|`admin`, default `person`) and
   `connect[].scope_presets` to the manifest model, the snapshot and the linter. Old packages stay valid
   (a golden test over every builtin and fixture). Set the two values the tests need: the `github_app`
   method of `plugins/builtin/github` is `audience: admin`; `plugins/builtin/linear` gets
   `scope_presets.write: [read, write]`. Bump each package's patch version and run `check-builtin.sh`.
2. **`WhoCan` on the server (C-981).**
   - One type and one builder: action, role, up to three display names (owners first, then by name;
     never e-mails; never instance admins), a path from the table of C-960 paths when the caller can do
     it themselves. Put that table in `crates/aurora-core`, one function per page, so that the gateway
     uses the same one later (C-959, P5-UX-17). The role phrases are the dashboard's (`14 §Who can fix
     this`); the server sends the role, not the phrase.
   - The shared role helpers put it in every `403 forbidden` (`details.who_can`). Do this in the helpers,
     not route by route.
   - `GET /v1/workspaces/{ws}` and `GET /v1/orgs/{org}` gain `contacts` (up to three names each), readable
     by every member of that level.
3. **Setup status and plan (C-931, C-955, C-946).**
   - One function, `setup_status(ctx, app, view)` in `domains/apps/setup.rs`, where `view` is a workspace
     or an organization. It computes availability, each method's readiness with `needs` and `who_can`, the
     suggested method, OAuth clients by level (refs only; the instance as set up or not), self-hosted
     apps, values to copy and what the caller can do.
   - Routes `GET /v1/apps/{app}/setup` and `GET /v1/apps/{app}/connect-plan` (a projection of the same
     function).
   - `used_by_methods` on client refs and on `AppDetail.oauth_clients[]`.
   - The setup status does not list the app's accounts: the setup page reads them from the accounts list
     (C-917, P5-UX-4) filtered by app.
4. **Access levels (the shared expansion).** In `domains/grants`, one function expands a level (`read`,
   `ask`, `act`) into a grant from the app's tools in the snapshot, and one derives the connection's
   `access` from a set of levels. P5-UX-4 reuses both for C-980.
5. **Connect with access (C-957, SEC-130).** Extend `ConnectRequest` with `grant: { agent_ids, level }`.
   - Before any write or redirect: check the agents like C-504; refuse `act` on a `custom_only` app.
   - `connected`: the connection and the access in one transaction (reuse FIX-154's `write_in_tx` and
     `grant_connection_in_tx`); the connection's `access` derived from the levels; audit
     `via: "connect_journey"`; the answer adds `grants`.
   - `redirect`: keep the intent only inside the sealed `Flow`; at the callback, after the exchange and
     in the connection's transaction, re-authorize and write the access. On failure, commit the
     connection without access and report `grant_failed { code }` in the callback's `BroadcastChannel`
     message, next to `granted_agent_ids`.
   - A reconnect with `grant` → 422 `grant_not_allowed_on_reconnect`. Without `grant`, behaviour is
     unchanged.
6. **A website or an AI key with access (C-988).** `POST /v1/workspaces/{ws}/secrets` accepts
   `grant: { agent_ids }`, checked like C-504 and written in the secret's transaction.
7. **Refusals (C-958).** `connect_probe_failed` gets `details.cause` and `details.host`, classified in
   the probe runner (401/403 → `rejected`; DNS, connect, TLS or timeout → `unreachable`; anything else →
   `unexpected`). `oauth_client_required` and `instance_oauth_client_required` carry `details.who_can`.
8. **OpenAPI.** Regenerate the document, `web/openapi.json` and `schema.d.ts`.

## Tests (each seen failing first)

- **Linter:** a `scope_presets.read` entry that is a write scope or not a method scope → error; every
  builtin and fixture still validates.
- **Setup status** (`api/it/app_setup.rs`):
  - GitHub with no OAuth clients: `github_app` is `admin`; `oauth` and `oauth_app` need a client, with
    `who_can` naming the org admins; the suggested method is `token`.
  - With an organization client for `oauth`: suggested `oauth`. With a workspace client: ready in that
    workspace only.
  - An instance client: `instance.set_up = true`, no id.
  - Linear with no client: suggested `null`, with `who_can`.
  - `aws-keys` returns `values.external_id`; `linear` does not.
  - GitLab without a registered server: the instance-bound methods need `instance`.
  - A restricted organization without the app: `available: false`.
  - The plan and the setup status agree for every method.
  - No client secret, client id or e-mail appears in any answer (a scan of the JSON). Instance admins
    are never named.
- **`WhoCan`:** a non-owner's 403 carries role and names; `contacts` is readable by a member and holds at
  most three names.
- **Connect with access:**
  - `fields`: account and access in one transaction; a forced failure of the access part rolls back both.
  - Each level expands to the expected allowed and asked tools; the connection's `access` follows.
  - `act` on `custom_only` → 422, nothing written.
  - OAuth (the fake provider of the existing connect tests): access applied at the callback; the caller
    demoted, or the agent deleted, between start and callback → the account is kept, no access,
    `grant_failed`; tampered state → `oauth_state_invalid`, nothing written; the state parameter and the
    cookie contain no agent id in clear.
  - Reconnect with `grant` → 422. Without `grant`, the existing tests stay green.
- **Secrets with access:** one transaction; refusals as C-504.
- **Probe:** 401 → `rejected`; an unroutable host → `unreachable` with `host`; 500 → `unexpected`.
- `permission_matrix` rows for the new routes; OpenAPI drift check green; a `security-coverage.md` row
  for SEC-130.

## Constraints / file ownership

- Owns: `crates/aurora-registry` (manifest keys, linter); `domains/{connect,apps}`; the level expansion
  in `domains/grants`; `domains/secrets` (the `grant` field); the `WhoCan` type and the role helpers;
  the shared table of dashboard paths in `crates/aurora-core`; `plugins/builtin/{github,linear}/plugin.yaml`
  (the two keys only); OpenAPI and the generated web types.
- No dashboard work.
- Rust conventions: `cargo fmt --check`, clippy `-D warnings`, targeted tests per step; full suites at the
  batch end.

## Acceptance criteria

1. Every test above passes; test counts reported.
2. One function serves the setup status and the plan.
3. Every refused route carries `who_can`.
4. An account and its access are never left half-made by a successful request.

## Suggested agent/model and reason

Sonnet (`rust-developer`). Several domains, each part bounded. The OAuth re-authorization needs care, and
the reviewer should read SEC-130 against the callback code.

## Completion report

Worker 13 (lane `p5-agent`), 2026-11-16. The task is split into parts, each its own commit `P5-UX-2: …`; a next worker picks up at the first part not marked done.

| Part | Scope items | Content | Status |
|---|---|---|---|
| (a) | 1 | Manifest keys `connect[].audience` and `scope_presets` (model, snapshot, linter, golden test), the two builtin packages, the shared table of dashboard paths in `aurora-core` | done (commit below) |
| (b) | 2, 4 | `WhoCan` type and builder, role helpers put it in every 403, `contacts` on workspace and org, the level expansion in `domains/grants` | open |
| (c) | 3, 7 (probe part) | `setup_status` in `domains/apps/setup.rs`, `/setup` and `/connect-plan`, `used_by_methods`, probe `cause` and `host`, `who_can` on the two client refusals | open |
| (d) | 5, 6, 8 | Connect with access (fields and OAuth, SEC-130), secrets with `grant`, OpenAPI and web types, `permission_matrix`, `security-coverage.md` | open |
