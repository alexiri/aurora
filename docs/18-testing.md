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
- (phase 2, RFE-011a) Deadlines and stall exemption: effective timeout = minimum
  of set timeouts across scopes, default when unset, authoring bounds
  (`approval_timeout_out_of_range`), startup refusal when the instance maximum
  reaches the turn ceiling; a turn with a pending 20-minute approval is **not**
  `turn_stalled` and is after the approval resolves plus `TURN_STALL`; ceiling still
  fails it; action approvals are not exempt.
- (phase 2, RFE-011a) Notifications: e-mail only after `APPROVAL_NOTIFY_DELAY` with no
  successful card op and no fresh watcher; scheduled/webhook turn in a workspace
  with no channel presence emails its explicit approvers/owners (org admins when
  none), visibility-intersected for direct conversations, one mail per batch,
  digest above the hourly cap, idempotent across two replicas, content has no body
  or decision link, user opt-out and instance switch, log-only without SMTP;
  dashboard browser notification only while hidden/unfocused and only for
  decidable approvals (Playwright with a stubbed Notification API).
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

### BYO governance (phase 2, RFE-025, SEC-52)
- Posture: matrix of instance `byo.enabled` × floor × org × workspace (strictest
  wins; org/workspace cannot loosen; `byo.enabled = false` overrides everything);
  creation and token regeneration per posture (`can_use`/`can_own`/nobody,
  `byo_not_permitted` vs `byo_disabled`); hosted agents unaffected; default floor
  `owners_only`.
- Tightening to `disabled`: existing `aug_` and BYO `auc_` refused 403
  `byo_disabled`, live tunnel closed ≤ 5 s, held approval `abandoned`, container-
  config and context-credential routes 403; loosening restores service without
  new tokens; nothing deleted.
- Source restriction: effective list resolution (agent > org > instance), subset
  validation (`byo_cidr_outside_default`), `/0`, empty and oversize lists
  (`byo_cidr_invalid`); gateway admission with IPv4, IPv6, IPv4-mapped,
  boundary addresses; spoofed `X-Forwarded-For`/`Forwarded` ignored; PROXY v2
  header honoured only from `gateway.trusted_proxies`, trusted peer without
  header and untrusted peer with header both closed; NAT/LB-address fail-closed;
  refusal metric and rate-limited request-log row; check runs after authentication
  (a bad token still gets 407 from any address).
- Require context: `aug_` refused with `context_credential_required` only when a
  credential would be injected; uncredentialed traffic and `auc_` pass; effective
  value is the OR of instance, org and agent; `aurora run` mints a labelled
  context credential and the label appears in the request log and approval card.
- Warning: `rule_applies_to_all_agents` for hand-written empty-identity allow
  rules naming `connection`/`app_connections`/`secret`/`secrets`, none for
  derived rules, identity-scoped rules or `app`/`network` targets; publish not
  blocked; editor confirmation lists the rules.
- Audit: `agent.byo_settings.update`, `workspace.byo_posture.update` and
  `agent.token_regenerate` rows are written in the same transaction (rollback test).

### BYO token age (phase 3, RFE-013, SEC-53)
- Effective maximum age: none by default; instance only, org only, both (smaller
  wins), org above instance refused; bounds 1…3 650.
- Mint and regenerate set `expires_at`; BYO context credentials capped by the token;
  retrofit on introduce/tighten with the 7-day minimum, no extension on loosen or
  removal; audit metadata records the count.
- Gateway and API: expired token → 407 `proxy_auth_required` (`agent_token_expired`)
  / 401 `agent_token_expired`; identity cache honours `expires_at` without waiting
  for the TTL boundary (clock test); `last_used_at` written at most once a minute.
- Stale/expiring views: thresholds, never-used tokens count from creation,
  authorization of the workspace/org routes, no token material in responses; the
  dashboard badges.

### Gateway under a database outage (phase 3, RFE-014, SEC-54)
- Fault injection (a TCP proxy in front of Postgres that can drop, stall or refuse,
  plus `pg_terminate_backend`): state machine `ready` ⇄ `degraded` ⇄ `down`
  (two failed probes, hot-path connection error, statement error is *not* an outage,
  start with the database down → `down` until a snapshot loads).
- (a) Cached identity + ResolvedHost keep serving until their TTL (clock-controlled)
  and are then refused; unknown credential → 503 `gateway_degraded` (never 407);
  uncached host, new approval hold, OAuth refresh, uncached plaintext → refused with
  the documented codes; open tunnels and in-flight streams survive; default-deny and
  `credentialed_only` still apply to cached state; an existing hold expires without
  the database and its row is fixed on recovery; request-log buffer fills, drops the
  oldest beyond the cap and flushes on recovery.
- (b) LISTEN-only loss: caches kept, no flapping within `GATEWAY_LISTEN_GRACE`,
  reconnect backoff, flush *after* re-subscribe, an invalidation sent while
  disconnected is honoured after the flush, an invalidation arriving after
  re-subscribe is not lost; miss coalescing after the flush.
- (c) `/readyz` bodies and status codes for the three states and `?strict=true`;
  metrics and the `gateway.state_change` log line; `/internal/v1/status` fields.

### Message idempotency (phase 3, RFE-018, SEC-55)
- Key validation (length, charset), absent key = phase 0 behaviour (two posts, two
  turns), `Idempotency-Key` ignored on other routes.
- Replay: same status and shape as the original for `{turn}` and `{followup}`;
  state reflects the current row (running, done, failed, aborted, promoted);
  `Idempotent-Replayed: true`; no second turn, no second `NOTIFY aurora_work`.
- Concurrency: N parallel identical requests → exactly one row, N identical
  responses; a key colliding with the active-turn index is a follow-up, not a replay.
- Scope and order: per (conversation, user) — two users, same key; the same user
  through session and API key shares the key space; replay after loss of access is
  refused; replay while frozen returns the stored result while a new message is 423;
  replay consumes one message-bucket token.
- Mismatch (`text`, `attachment_ids`) → 422 `idempotency_key_reused`; key freed when
  the conversation is deleted or retention-swept.
- Reserved prefix: a connector `event_id` starting `api:` is `rejected`
  (`reserved_event_id`); ordinary connector dedupe unchanged.

### Deployment-neutral defaults (phase 3, RFE-019)
- New workspace → default agent brief equals the configured `agents.default_brief`;
  unset/empty → the neutral default of `10` (2–3 sentences, no persona, no claims
  about the agent's nature); an over-long value fails startup naming the key;
  existing agents' briefs are untouched by a config change or an upgrade.
- Architecture guard (extends `scripts/guards`): the normative defaults carry no
  deployment-specific content — the Containerfile's `SANDBOX_BASE_IMAGE` default is
  a public registry image pinned by digest (no `*.cern.ch` or other private host),
  `agents.default_brief`'s built-in default contains no persona names or
  backstory — and the CERN values appear only in the documented deployment example.
- Image build: building with and without a `SANDBOX_BASE_IMAGE` override yields the
  same package list and `org.aurora.capabilities` label (smoke test).

### Workspace-tier skills (phase 3, RFE-020, SEC-56)
- Effective set: all combinations of one name at agent/workspace/org tiers with
  enabled/disabled (highest *enabled* tier wins; a disabled higher tier does not
  veto); names unique per tier but allowed across tiers; reserved names refused at
  every tier; CHECK forbids agent+workspace on one row; index replacement migration
  keeps existing org skills unique.
- Authorization matrix for list/create/edit/delete/files per tier; org admin writes
  a workspace skill (as `can_own`); plain member read-only; agent tools cannot write
  workspace/org tiers and `skill_list` shows tier and `shadows`.
- Projection: every agent of the workspace (and only it) receives the skill after a
  generation bump; unchanged for agents of other workspaces; removed skill is pruned
  by the manifest; shadowed skill is not projected; read-only modes unchanged.
- Dashboard: tier badges, shadow indicators, read-only open from the agent tab,
  workspace skills page (owner vs member).

### Default Rule and LLM hosts (phase 3, RFE-023)
- Policy corpus (`crates/aurora-policy/tests/corpus/`): Default Rule `block` +
  injected non-LLM credential → `default_denied`; + injected LLM key on a governed
  LLM host → no verdict (flows, subject to governance) with trace reason
  `default_rule_skipped_llm_host`; + explicit block / require-approval / rate-limit
  rule naming the LLM host → the rule wins; uncredentialed request to an LLM host
  under `credentialed_only` → `egress_denied` unless allow-listed; same results for
  org and workspace scope and for the combined verdict; simulate and effective view
  report the reason.
- Dashboard: the Default Rule note appears at org and workspace scope and links to
  governance.

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

### Plugin major upgrades (phase 3, RFE-022, SEC-57)
- Activation matrix over (minor, major) × (required field kept / renamed / removed /
  added optional / added required) × (connection `active`, `reauth_required`,
  `disabled`, `orphaned`, legacy null keys): exactly the failing active connections
  flip, with the reason text and `reauth_required_at`; others untouched.
- Atomicity: failure after the first flip rolls back the activation and every flip;
  `connection` invalidation reaches the gateway; the gateway returns
  `connection_reauth_required` with the reconnect URL; reconnect clears it and
  records the new `app_version`.
- Approval: `impact` counts match the later activation (and are recomputed in it);
  a builtin major upgrade that would break connections stays `pending_approval`
  with `breaks_connections`, one with no impact activates automatically as before;
  dashboard banner and mandatory Impact section; no tenant names in the response.
- Fixtures: `fixture-bearer` v2 (renamed required field) and v1.1 (added optional
  field); legacy row without `credential_keys` for both fallback paths.

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
- (phase 2, RFE-021) Automation continuity: reassign (authority, target must be an
  active member with `can_use`, `re_enable`), bulk reassign, `reassign_to` on
  member removal/suspension and workspace-binding removal commits or rolls back with
  the removal, offboarding views (org and workspace scope) match what actually gets
  disabled by the removal, anchored schedules/webhooks keep firing after the
  creator is removed and stop when no active owner exists or the agent is frozen,
  members and agents cannot anchor, audit rows (`automation.reassign`) are atomic.
- Security probes inside a real sandbox (SEC-01, 02, 09, 26, 34).

### Self-contained deprovision (phase 3, RFE-024, SEC-58)
- Atomicity: delete of an agent/workspace/org with dedicated presences commits the
  ops with the delete; an injected failure after the op inserts (or in the cascade)
  rolls back both; detach of a single presence likewise.
- Self-containment: with the integration, presence, agent and org rows deleted, a
  *different* connector replica claims the op (integration_id NULL) and the fake
  connector executes deprovision from payload + `credentials` alone; Mattermost
  conformance: token revoke + bot disable with only the payload; unknown bot → 404 →
  `dropped`.
- Credential copy: sealed under the op context (open under another context fails
  `SecretRefMismatch`), plaintext only in the claim response, `discard`ed and nulled
  on done/failed/dropped and by pruning; ext-store managed versions deleted; never in
  logs or audit metadata.
- Retry/failure: `unavailable` retried for `DEPROVISION_RETRY_WINDOW` (clock test),
  `credential_dead` → failed with `presence.deprovision_failed` audit and metric;
  shared presence enqueues nothing; org deletion does not touch the shared bot.

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

### API rate limiting (phase 2, RFE-017)
- Bucket semantics per class (burst then steady refill), a request must pass
  general + class + org buckets and a refused request consumes none of the others;
  principal isolation (a second key and another user are unaffected), a user's
  sessions share one budget, org bucket across principals, stream/long-poll
  concurrency cap (`too_many_streams`), 429 envelope with `retry-after` and
  `ratelimit-*` headers and `details.scope`, exempt routes (health, metrics,
  machine planes, webhooks), config overrides and multiplier, startup validation,
  idle-bucket eviction; dashboard client retry behaviour (idempotent reads retried,
  writes not).

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

**Implementation (OPS-019, `.github/workflows/ci.yml`).** `rust-build` compiles once and publishes a
`cargo nextest archive` (all workspace tests except the e2e crates, plus the fixture packages);
`rust-test` runs it in 4 shards (`--partition count:i/4`, profile `ci` in `.config/nextest.toml`);
doctests run in `rust-build` (nextest skips them). `rust-lint`, `rust-deny`, `web` and `guards` are
separate jobs. Actions are SHA-pinned and on Node 24; runners are pinned to `ubuntu-24.04`.
The e2e crates run via `scripts/e2e/*.sh`, not in this workflow. See `docs/ops/build-performance.md`.
