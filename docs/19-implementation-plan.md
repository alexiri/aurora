# 19 — Implementation Plan

Phases and milestones. Each milestone lists scope, deliverables and
**acceptance criteria** (tests from `18`). Milestones are sized so several
implementation agents can work in parallel once the shared foundations (M0–M1)
exist; the spec fixes every interface between workstreams.

## Phase 0 — Core

### Dependency graph

```
M0 Foundations ─► M1 Identity & API ─► M2 Gateway, secrets, policy, BYO ─► M3 Plugin system
                                              │
                                              ├──────────────► M4 Hosted runtime ─► M5 Hosted features
                                              │                       │
                                              └──────────► M6 Channel core (needs M4 conversations)
                                                                      │
                                                     M7 Kubernetes, Helm, hardening (after M4; parallel to M5/M6)
```

### Workstreams (for parallel agents)

| Stream | Owns | Primary docs |
|--------|------|--------------|
| A — Platform/API/UI | `aurora-api` (identity, tenancy, routes), dashboard | 04, 05, 14 |
| B — Data plane | `aurora-gateway`, `aurora-policy`, `aurora-secrets`, `aurora-llm` | 06, 07, 08 |
| C — Plugins | `aurora-registry`, `aurora-summary`, hook host, SDK, CLI `plugin`, fixtures | 09 |
| D — Compute | runner, backends, supervisor, harness adapters, sandbox image | 11, 12 |
| E — Channels | channel core in the API, connector SDK, fake connector, conformance | 13 |

### M0 — Foundations
Scope: cargo workspace and repository layout (`02`); `aurora-core` (ids,
error envelope, sanitizers, limits registry, token generation/hashing);
`aurora-config`; `aurora-db` (pool, embedded migrations implementing **all**
tables of `03`, claim helpers, NOTIFY bus); `aurora-crypto` (inline envelope,
keyring, HKDF, ed25519); `aurora-protocol` skeleton with schema generation;
`aurora-testkit` (ephemeral Postgres, mock upstream, mock OAuth/OIDC servers);
service binaries with health/metrics/config validation; CI pipeline; compose
skeleton (postgres, migrate, api, gateway, hookhost, web, edge).
Acceptance:
- CI stages 1–2 green; `aurora-api migrate` on an empty database yields a schema
  that passes the `03` introspection diff.
- Envelope fixtures and sanitizer property tests pass.
- `podman compose up` starts all services healthy.

### M1 — Identity, tenancy, API skeleton, dashboard shell
Scope: `04` completely (email/password, verification, reset — removed again in phase 1, RFE-004 —, OIDC, sessions,
CSRF, API keys, CLI device login, invitations, roles, access law, permission
matrix, instance admins, bootstrap command, audit); org/workspace CRUD and
sign-up transaction including default policy generations and the **default
agent Daneel** (row only); OpenAPI generation; dashboard: auth pages, org and
workspace shells, members, invitations, API keys, account, admin users.
Acceptance: permission-matrix test; OIDC tests with mock IdP; Playwright
sign-up → create workspace (Daneel present) → invite → accept.

### M2 — Secrets, LLM keys, gateway, policy, approvals, BYO
Scope: `aurora-secrets` (`SecretVault`, registry, `inline`, test `memory`
store, reference mode plumbing, `secrets migrate|rekey`); secrets and LLM key
APIs and UI; `aurora-llm` with **LiteLLM** and **Anthropic**; gateway: CA
(`generate-ca`), MITM and open lanes, credential auth (agent tokens), full
pipeline for secrets and LLM keys, destination guard, guidance codes, caches +
NOTIFY invalidation, request log, internal port; `aurora-policy` (matchers,
evaluator, injection selection) and policy authoring (drafts, publish,
rollback, simulate, warnings) with UI; grants for secrets; approvals
(Postgres store, API, SSE, dashboard); BYO agents (token, container config,
context credentials), CLI `login`, `run`, `approvals watch`; **LLM and egress
governance** (`08 §11`: instance ceiling, org governance API and UI, key
admission and origin verification for LiteLLM and Anthropic, gateway
governance step, origin preflight and backstop, model allow-lists, the
uncredentialed default with allow-lists).
Acceptance:
- e2e-gateway: a BYO client calls a mock LiteLLM endpoint and a mock API
  through the gateway with injected keys; block, default-deny, rate limit and
  approval (approve/deny/expire) flows; guidance codes; SEC-03..08, 10–12, 28.
- Governance suites of `18`; SEC-35..39: a personal (foreign-organization)
  Anthropic key is refused at save and at first use; an agent-supplied key is
  refused under `sanctioned_only`; uncredentialed traffic is refused under
  `credentialed_only` except allow-listed hosts.
- Two gateway replicas share approvals and invalidations.

### M3 — App plugin system
Scope: `09` completely: package format, signing, trusted keys, validator,
ingestion sources, approval flow and admin UI, host claims, snapshot compiler
and activation, gateway snapshot loading and **all mechanisms**, connect flows
(`fields`, `probe`, `oauth2`, `hook`) with state handling, OAuth clients at
three scopes, connections UI and connect window, connection grants with tool
modes, effective views, OAuth refresh with locks and CAS, `token.mint`,
approval summaries (templates, decoders, generic), hook host + WIT + SDK,
`aurora plugin` CLI, `aurora-gateway check-snapshot`, all fixture packages.
Acceptance: every fixture package's tests; plugin suites of `18`; SEC-13..17,
27.

### M4 — Hosted runtime
Scope: runner (registration, replicas, ownership, channel server, reconcile)
with `fake` and `podman` backends; sandbox image; supervisor (boot document,
non-dumpable, loopback proxies, WS transport with sequencing, harness
interface, `fake` adapter, MCP bridge, home sync, instruction rendering);
due-work engine (start/stop/dispatch/steer/abort/send-now/recovery arms);
conversations, turns, follow-ups, transcript persistence, SSE; context
credentials for hosted turns and boot tokens; model selection; tool registry
skeleton; dashboard chat and computer tabs.
Acceptance: e2e-hosted — chat with Daneel (fake harness) whose HTTP calls go
through the gateway with its context credential (attributed in the request log
and on an approval); idle park and wake; restart; follow-up promotion; never-
started revival; security probes SEC-01, 02, 09, 24–26, 34.

### M5 — Hosted features
Scope: memory (projection, harvest, search, recall, **memory language**),
skills (agent and org), schedules with gates, webhooks, processes and watches,
attachments in/out, full platform-tool catalogue (`12`), action approvals,
agent links and peer tasks, app-turn cap, `claude-code` adapter with live
conformance, dashboard tabs (memory, skills, schedules, webhooks, peers).
Acceptance: hosted suites of `18`; live conformance passes against a
configured LiteLLM endpoint.

### M6 — Channel core
Scope: `13` completely: connector registration and manifest approval, config
feed with integration ownership, inbox and ops with leases and error codes,
correlated ops, routing, linking, reach, bindings, commands, mirror, cards and
action tokens, presence lifecycle, messaging tools; `aurora-connector-sdk`;
fake connector; conformance suite; dashboard channel pages and admin
connectors.
Acceptance: connector conformance suite green against the fake connector;
channel suites of `18`; SEC-18..20; e2e-hosted scenario: a fake-platform user
DMs the shared bot, gets an answer from Daneel, approves a held request from a
card.

### M7 — Kubernetes, Helm, hardening
Scope: `kubernetes` runner backend; Helm chart per `15` (jobs, policies,
namespaces, RBAC, routing); nightly kind e2e; performance budgets; upgrade
test (previous tag → current with running sandboxes); operator documentation
(install, configure LLM endpoints, add a runner, approve plugins, backups).
Acceptance: e2e-hosted passes on kind with NetworkPolicy enforcement; SEC-02
probes pass on Kubernetes; performance budgets met; upgrade test green.

**Phase 0 exit**: all milestones accepted; every SEC invariant has a passing
proving test; the compose stack and the Helm chart install from scratch with
documented steps.

## Phase 1 — Mattermost connector

Scope: `20`. Order: `shared` mode (single bot, bindings, DM defaults), then
`dedicated` mode (bots API provisioning).
Design-review items (`80`): RFE-001 (verified-email auto-link opt-in per
integration, default off), RFE-002 (TLS-only gateway proxy listener, internal
and external), RFE-003 (open-lane posture, CONNECT-time rule semantics,
supervisor 3129 listener only with the browser capability), RFE-004 (OIDC-only
sign-in), RFE-009 (direct conversations visible to their user and workspace
owners only), RFE-016 (`DeltaBus` seam). Acceptance: connector conformance
suite against a Mattermost container in CI; live scenario: DM, channel
mention + thread follow-up, approval click, unlinked user link flow, bot
removal handling.

## Phase 2 — App catalogue and database lane

- Built-in packages under `plugins/builtin/`, each with fixtures, setup guide
  and icons, signed by the project key. Suggested first set (covers every
  mechanism): GitLab, GitHub (OAuth) and GitHub App (hooks), Jira and
  Confluence (shared OAuth client), a Google Workspace package (Gmail,
  Calendar, Drive with MIME summaries via hook), Microsoft 365 package,
  AWS keys and AWS role (SigV4 + mint), Sentry, Notion, Linear, PostgreSQL.
- Gateway database lane (`21 §Database lane`).
- Design-review items (`80`): RFE-005 (policy `user` identities bind to the
  turn's speaker), RFE-006 (freeze agent/workspace/org), RFE-007 (keyring-style
  lists for the signing key and internal secret), RFE-008 (atomic audit for the
  security-critical set), RFE-010 (conversation delete/archive, transcript
  redaction, retention, export), RFE-011a (stall exemption, per-rule approval
  timeouts, email to owners when no surface is live, SSE browser
  notification), RFE-012 (`approver` workspace role), RFE-015 (attachment
  retention and quota), RFE-017 (management API rate limiting), RFE-021
  (automation creator reassignment, offboarding view, workspace-anchored
  automations), RFE-025 (BYO posture, source CIDR list, all-agents-rule
  warning, required context credentials).
- Phases 1 and 2 may run in parallel once phase 0 is merged to `master`.

## Phase 3 — External secret stores

HashiCorp Vault store (`21 §HashiCorp Vault`), managed and reference modes,
migration tooling exercised against a Vault dev server in CI.
Design-review items (`80`): RFE-013 (optional agent-token maximum age),
RFE-014 (gateway behaviour during Postgres outages), RFE-018 (idempotency keys
for message creation), RFE-019 (neutral default brief, public base image),
RFE-020 (workspace-tier skills), RFE-022 (plugin major upgrades →
`reauth_required`), RFE-023 (document the LLM-host default-rule carve-out),
RFE-024 (self-contained deprovision ops).

## Phase 4+ — Later

SSH front door, port previews, evals, git-backed skill library, groups and
SCIM, shared rate-limit counters, S3 blob store, additional LLM providers,
additional connectors (Slack decision pending). See `21`.
Design-review item RFE-011b (approval park-and-resume, Web Push).
Phases 3 and 4 start once phases 1 and 2 are done.
