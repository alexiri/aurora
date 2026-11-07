# 18 — Testing Strategy

Every milestone in `19` lists acceptance tests from this document. A feature is
done when its tests exist, pass in CI, and the security invariants it touches
(`17`) have their proving tests.

## Layers

| Layer | Scope | Tooling | Runs |
|-------|-------|---------|------|
| **Unit** | pure logic per crate (matchers, evaluator, sanitizers, templates, envelopes, cron, state machines) | `cargo nextest`, `proptest`, `insta` | every PR |
| **Database** | repositories, claims, partial-unique races, migrations up from empty and from the previous release's schema | ephemeral Postgres from `aurora-testkit` (podman), one database per test | every PR |
| **Service** | one service in-process with real Postgres and mocks for its neighbours (API with fake runner/connector clients; gateway with mock upstreams and a stub hook host) | `aurora-testkit`, `wiremock` | every PR |
| **Contract** | OpenAPI document vs handlers; JSON Schemas of every wire type vs checked-in schemas; dashboard client regenerated without drift | schema diff | every PR |
| **Conformance** | harness adapters, connectors, plugin packages against their contracts | `tests/conformance/*` | every PR (fake implementations); env-gated for live ones |
| **End-to-end: gateway** | real gateway + API + hook host + Postgres + fixture plugins + mock upstreams/OAuth providers, black-box over the proxy port | `tests/e2e-gateway` | every PR |
| **End-to-end: hosted** | real API, gateway, hook host, runner (podman backend), sandbox image with the `fake` harness, fake connector | `tests/e2e-hosted` | every PR on Linux CI with podman; nightly on Kubernetes (kind with a NetworkPolicy CNI) |
| **Dashboard** | component tests; Playwright flows against the compose stack | vitest, Playwright | every PR |
| **Live** | `claude-code` adapter against a configured LiteLLM/Anthropic endpoint through the gateway; Mattermost connector against a Mattermost container (phase 1) | env-gated | nightly |
| **Performance** | gateway overhead and throughput; due-work poll cost with 1 000 agents | criterion, k6 | nightly |

## Required suites (non-exhaustive, by area)

### Policy and gateway
- Matcher property tests: host patterns (exact, single `*`, rejection of
  multiple `*`), path patterns (`*`, `**`, segment prefix), never panics.
- (phase 2, RFE-005) Speaker binding: corpus cases per `07 §Rule anatomy` — a rule
  naming Alice matches Alice's turns and not Bob's in the same workspace/agent;
  boot-token, BYO agent-token, BYO context, schedule/webhook/watch/peer/system
  turns never match `user` identities (allow rules select no credential); a
  speaker who lost `can_use` stops matching after invalidation; the context
  credential's `speaker_user_id` is set from the turn only for person speakers;
  simulate/effective with and without `speaker`; request-log rows carry
  `speaker_user_id`.
- Evaluator corpus (`crates/aurora-policy/tests/corpus/*.yaml`): request facts
  + rule sets → expected verdict and trace; covers scope combination, default
  carve-out, rate-limit accounting, condition polarity, GraphQL fail-closed,
  git operations, injection selection.
- Pipeline tests for every guidance code in `06`.
- Hold tests: approve, deny, expiry, client disconnect, SIGTERM
  (`gateway_restarting`), two gateway replicas with a decision written by the
  API.
- Refresh concurrency: two gateway processes, a mock provider that revokes on a
  second use of a refresh token; no revocation occurs (SEC-27).
- (phase 1, RFE-002) Proxy listener TLS: plaintext first bytes closed with no
  credential read (SEC-40); CA-issued leaf covers every SAN source of
  `06 §Proxy listener TLS` and is hot-renewed < 10 days before expiry without
  dropping tunnels; operator certificate mode (valid, expired, missing key →
  startup refusal); wrong server name and untrusted CA refused by the supervisor
  forwarder and by `aurora run`; ALPN is `http/1.1` only; a client matrix (curl
  with `--proxy-cacert`, Node `undici` ProxyAgent, Python `requests`/`urllib3` 2,
  Go, Chromium) completes `CONNECT` through the TLS listener.
- Cache invalidation: every write listed in `05 §Gateway cache invalidation`
  changes the next request's outcome without waiting for TTL.

### Governance
- Governance validation: org documents below the instance minimum, scope and
  list subset rules, defaults for new orgs.
- Key admission at save: permitted/non-permitted endpoints and scopes; origin
  verified, foreign organization, foreign workspace, foreign team,
  unverifiable provider, unreachable endpoint with and without origin rules;
  generic secrets refused on governed LLM hosts.
- Governance changes re-evaluate stored origins and drop disallowed model
  overrides without provider calls.
- Gateway governance step: every refusal code of `06` pipeline step 6a; model allow-list
  with truncated and unparsable bodies (fail closed); open lane under both
  postures.
- Origin preflight and backstop against a mock Anthropic-style provider that
  returns configurable organization/workspace headers, including a
  reference-mode secret whose external value changes to a foreign key.
- (phase 1, RFE-003) Open lane: credential-capability matrix (`aub_` with and
  without `browser`, `aug_`, `auc_`, username `open` spoofing); posture matrix
  (`enabled`/`allowlist_only`/`disabled` set at instance/org/workspace,
  strictest wins, a workspace cannot loosen); `allowlist_only` per-level lists,
  allow-list entries with a `path` do not admit tunnels; CONNECT-time corpus
  (`crates/aurora-policy/tests/corpus/connect_time.yaml`): path/method/body-scoped
  block and require-approval rules refuse the tunnel, path-scoped allow rules do
  not admit it, credential targets never match, rate-limit counts once per
  tunnel; supervisor starts 3129 only with the capability (image with and
  without the label); simulate with `lane: "open"` agrees with the gateway.
- Uncredentialed default: org `credentialed_only` with allow-list; workspace
  `block` with its own allow-list; a workspace allow rule cannot widen the org
  (SEC-38, SEC-39).

### Plugins
- Validator: one `fixture-invalid-*` package per rule with the expected reason.
- Signature and trust: unsigned, wrong publisher, revoked key, dev override.
- Lifecycle: install → pending → approve → active; upgrade without widening
  auto-activates; widening waits; forced uninstall orphans connections;
  removed tools become inert; new tools arrive blocked.
- Host claims: conflict holds activation; winner/loser injection behaviour;
  reversal.
- Hooks: each hook point via `fixture-hooks`; egress denial; summarize with no
  egress; memory/time/output limits enforced (a hostile test module that loops,
  allocates and floods output).
- Every fixture package's own `tests/*.yaml` via `aurora plugin test`.

### Secrets
- (phase 2, RFE-007) Signing-key / internal-secret rings (SEC-46): parse and
  validation (legacy bare value = ring `v0`, missing active id, duplicate ids,
  > `KEYRING_MAX`), `v2.` action tokens and id-prefixed states verified by id,
  `v1.`/unprefixed by trial, unknown id refused, internal calls with and without
  `X-Aurora-Key-Id` across the add → activate → remove sequence in a
  mixed-version api/gateway/hookhost test, status endpoints expose ids only.
- Inline envelope fixtures (cross-version), AAD row-swap attack, keyring
  rotation with `secrets rekey`, `secrets migrate` to the `memory` test store,
  reference-mode secrets via the `memory` store, store-unavailable fail-closed.

### Delta bus (phase 1, RFE-016)
- `DeltaBus` conformance suite (`tests/conformance/delta_bus`), run against the
  `notify` implementation and an in-memory reference: fan-out to every replica,
  per-(conversation, turn) order, payload bound, at-most-once with loss tolerated
  (a dropped delta is repaired by the durable `text` event), resubscribe triggers
  resync, publish failure never fails turn persistence, unknown
  `events.delta_bus` id fails startup.
- Benchmark `scripts/perf/delta_bus.sh` measures the planning figures of
  `15 §Streaming fan-out ceiling` (N concurrent synthetic streams × R replicas) and
  records them in the phase-1 report; a regression guard fails when p99 publish
  latency at 100 streams exceeds 50 ms.

### Freeze (phase 2, RFE-006)
- Effects matrix per level (agent, workspace, org): gateway `frozen` for MITM,
  open lane and WebSocket; live tunnel closed ≤ 5 s after freezing; held request
  answered `frozen` and approval `abandoned`; API 423 on messages/attachments/
  run-now; queued turn aborted `frozen`, running turn aborted; due-work arms skip
  (two pollers), sandbox parked `stop_reason = frozen` with home kept and not
  reconciled away; webhook 410 + `paused` delivery; schedule `skipped_frozen`
  without disabling; connector reply "paused" rate-limited by
  `FREEZE_REPLY_INTERVAL`; BYO endpoints 423.
- Authority: who may freeze/unfreeze at each level, `freeze_authority_insufficient`
  when lifting a higher freeze, instance-admin freeze sees no org data, idempotent
  repeat, audit rows in the same transaction (RFE-008), unfreeze resumes service.

### Hosted agents
- Due-work engine against the fake runner backend: every arm, claim races with
  concurrent pollers, ownership fencing, replica death and takeover.
- Turn lifecycle: queued → done; follow-up joined / promoted; abort; send-now;
  never-started revival; stall; ceiling warning and failure; restart.
- Home sync: parts packing at worst-case encodings, crash mid-generation,
  manifest pruning, memory harvest round-trip with checksum verification.
- Memory search with `english`, `german` and `simple` languages; language
  change re-render.
- Schedules: coalescing missed fires, gate quiet/wake/fail, authorization
  disable, failure disable.
- Security probes inside a real sandbox (SEC-01, 02, 09, 26, 34).

### Channels
- Core with the fake connector: every scenario of the connector conformance
  suite plus routing matrix (dedicated vs shared, DM default, bindings, listen
  modes), speaker authorization and reach, verified-email linking vs link flow,
  mirror rules (door-based), approval cards end-to-end,
  (phase 1, RFE-001) auto-link opt-in: default config never links on an asserted
  email (sign-in link flow with `platform_email_hint` pre-fill), enabling
  requires `email_verified` (422 `autolink_unsupported`), enabled path links only
  verified users that are active org members, both paths audited, confirm by a
  non-member refused (`not_org_member`), messaging tools with
  platform resolution and `proactive_messaging` modes, op error code handling,
  ownership failover between two fake connector replicas.

### Conversation data lifecycle (phase 2, RFE-010)
- Delete: authorization matrix (direct user, owner, member, key of each) per
  conversation kind; `conversation_active` refusal; full cascade (turns, events,
  buffers, attachments, blobs, thread links, credentials) in one transaction with
  the critical audit row; approvals keep rows with the reference nulled;
  schedule/webhook run conversation recreated on next fire; session purge
  delivered live, via home sync after a stopped sandbox boots, and before any turn.
- Redaction: payload replaced per event type keeping ids; `turns.text` and answer
  buffers scrubbed; idempotent and irreversible; SSE `event.redacted`; authority
  (turn author, direct user, `can_own`, nobody else); `reset_session` bumps the
  epoch and queues a purge; audit row has no content.
- Retention: window validation (bounds, instance ceiling), daily sweep deletes only
  idle conversations without pending approvals through the delete path, batching,
  system audit row per batch, disabled by default.
- Export: JSON schema `aurora.conversation.v1` (contract test), redacted events as
  markers, zip with attachments, `export_too_large`, authorization incl. an org
  admin exporting a user's direct conversation.

### Attachment storage (phase 2, RFE-015)
- Quota: upload, connector `file.fetch` (attachment `failed` `quota_exceeded` +
  notice) and agent `send_file` (`attachment_quota_exceeded` tool error) refused at
  the limit and accepted below it, concurrent uploads cannot overshoot by more than
  one file, 80 %/100 % notices once each, instance ceiling on the org setting, `0`
  unlimited.
- Retention sweep: bytes purged and row kept (`purged_at`), content 410, delivered
  sandbox copies untouched; orphan blobs removed; counter reconciled after injected
  drift; breakdown endpoints (`/v1/orgs/{org}/storage`, `/v1/admin/storage`) agree
  with the table sums and are authorization-checked.

### Identity
- Permission matrix test generated from `04 §Permission matrix` (every route ×
  role), suspended members, workspace key confinement, CSRF, OIDC against a
  mock IdP (discovery, PKCE, nonce, email-verified rule, JIT).
- (phase 2, RFE-012) Approver role (SEC-28): `can_approve` hierarchy
  (`can_own ⊂ can_approve ⊂ can_use`), every approvals route × {member, approver,
  owner, org admin} (`approver_required`), channel click / `approve <code>` by a
  member refused with the ephemeral and the card still pending, action approvals
  and agent-link decisions, invitation and JIT bindings with `approver`, role
  change takes effect on the next request and invalidates cached decisions,
  migration leaves existing members as `member`.
- (phase 1, RFE-009) Direct-conversation visibility (SEC-43): each
  conversation-returning/naming route × {the direct user, another member, a
  workspace owner, an org admin, workspace/org keys of each} — 404 vs 200,
  `direct_conversation_forbidden` for owner writes; the direct user who lost
  workspace access sees nothing; approvals naming a direct conversation hidden
  from other members in list, long-poll, SSE and batch (and undecidable → 404);
  channel DM card clicked by a non-participant refused; activity rows omitted;
  `conversation.owner_view` audit; platform DM and dashboard chat remain one
  conversation.
- (phase 1, RFE-004) OIDC-only: the removed password/verify/reset routes return
  404; sign-up semantics per `signup_mode` (`open` creates user+org,
  `invite_only` refuses without invitation/admin e-mail and creates no row,
  `sso_jit` via the provider `jit`); invitation accepted through a first OIDC
  sign-in; `bootstrap-admin` pre-provisioning bound by first sign-in;
  `auth.instance_admins` flag via OIDC; unverified e-mail, disabled user,
  re-authentication (`reauth=1`, `reauth_required` on account deletion);
  startup refusals (no provider, dev IdP on a public URL, `require_email_verified
  = false`); the dev IdP and `aurora_testkit::oidc` `sign_in` helper used by the
  Playwright flows; API keys and CLI device login unchanged.

- (phase 2, RFE-008) Atomic audit (SEC-32): the audit coverage test enumerates
  `SECURITY_CRITICAL_ACTIONS` against the route catalogue (every critical route
  produces its row with `critical = true` in the same transaction); a
  fault-injection hook makes the audit insert fail and every critical action
  must return 503 `audit_unavailable` with **no** state change (policy publish,
  grant put, role change, approval decide via dashboard/API/channel, plugin
  approve, freeze, governance update); best-effort actions still succeed with the
  hook on (`aurora_audit_dropped_total` increments); a repository-signature test
  (critical mutations take the audit entry) guards against future bypass.

### Architecture guards (CI scripts)
- No app/platform names in core paths (SEC-33).
- Dependency graph: `aurora-gateway` and `aurora-api` do not depend on
  `wasmtime`; `aurora-policy` does not depend on `aurora-db`.
- Edge configuration covers all route prefixes (`15`).
- Every table in `03` exists in migrations and vice versa (schema introspection
  diff against a machine-readable extract of `03`).

## Performance budgets (gateway)

- Added latency for a cached, allowed, bodyless request with injection:
  p50 < 1 ms, p99 < 5 ms (excluding upstream).
- 2 000 requests/s per gateway core on cached paths.
- Connect-time resolution (cache miss): p99 < 25 ms with 1 000 rules and 500
  connections in scope (excluding secret-store latency).

## CI pipeline

1. `fmt`, `clippy -D warnings`, `cargo deny` (licenses, advisories), pnpm lint
   and type-check.
2. Unit + database + service + contract tests.
3. Build images (multi-arch on main; amd64 on PRs).
4. e2e-gateway, e2e-hosted (podman), dashboard Playwright on compose.
5. Architecture guards.
6. Nightly: Kubernetes e2e on kind, live suites, performance.
