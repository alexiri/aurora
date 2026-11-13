# 18 — Testing Strategy

Every milestone in `19` lists acceptance tests from this document. A feature is
done when its tests exist, pass in CI, and the security invariants it touches
(`17`) have their proving tests.

## Layers

| Layer | Scope | Tooling | Runs |
|-------|-------|---------|------|
| **Unit** | pure logic per crate (matchers, evaluator, sanitizers, templates, envelopes, cron, state machines) | `cargo test`, `cargo nextest` (the documented fast path, `just test-nextest`), `proptest`, `insta` | every PR |
| **Database** | repositories, claims, partial-unique races, migrations up from empty and from the previous release's schema | ephemeral Postgres from `aurora-testkit` (podman), one database per test | every PR |
| **Service** | one service in-process with real Postgres and mocks for its neighbours (API with fake runner/connector clients; gateway with mock upstreams and a stub hook host) | `aurora-testkit`, `wiremock` | every PR |
| **Contract** | OpenAPI document vs handlers; JSON Schemas of every wire type vs checked-in schemas; dashboard client regenerated without drift | schema diff | every PR |
| **Conformance** | harness adapters, connectors, plugin packages against their contracts | `tests/conformance/*` | every PR (fake implementations); env-gated for live ones |
| **End-to-end: gateway** | real gateway + API + hook host + Postgres + fixture plugins + mock upstreams/OAuth providers, black-box over the proxy port; CI-only strict front doors (pinned nginx + haproxy containers between the gateway and the mock upstream, TEST-021 tier 2: `AURORA_E2E_FRONTDOOR`, skipped when unset) | `tests/e2e-gateway` | every PR |
| **End-to-end: hosted** | real API, gateway, hook host, runner (podman backend), sandbox image with the `fake` harness, fake connector | `tests/e2e-hosted` | every PR on Linux CI with podman; nightly on Kubernetes (kind with a NetworkPolicy CNI) |
| **Dashboard** | component tests; Playwright flows against the host-process stack (`scripts/e2e/m1.sh`, default) or the kind dev stack (`m1.sh --kind`; phase 1, OPS-017: kind replaces the compose stack) | vitest, Playwright | every PR |
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
  same package list and `org.aurora.capabilities` label (smoke test:
  `scripts/images/sandbox-base-smoke.sh` builds `sandbox-min` with the default digest-pinned base and with the
  same image by another reference and compares `rpm -qa` and the label; `SMOKE_FULL=1` also covers the full
  `sandbox` target, which is what the nightly job `sandbox-base-smoke` runs).

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
- (P2-TEST-1) `process_start` environment probe (SEC-34 / I-23): a process started through
  the supervisor's `process_start` tool has no `AURORA_BOOT`, no `aub_`/`aup_`/`auc_`
  value and neither the bootstrap nor the gateway boot token in its environment, read from
  inside the process and from another agent-uid process (`/proc/<pid>/environ`). A negative
  control proves the matcher fires on an environment that does carry them (a `podman exec`
  process gets the container's configured environment, `AURORA_BOOT` included, by podman's
  design, so a sweep over every PID is not a valid probe).
- (P2-TEST-1) Chromium wrapper on the **full** sandbox image (`browser` capability label,
  SEC-41, FU-P1-003b): boot document carries `browser`, the open proxy listens on 3129, the
  wrapper starts Chromium behind it, an open-lane `allowlist_only` host is fetched through
  the gateway and another host is refused (`403 blocked_by_egress_policy`, nothing reaches
  the page server). Runs only with `AURORA_E2EH_FULL_IMAGE=1` (otherwise the scenario
  prints `SKIPPED`): `hosted.sh` then builds `--target sandbox` as
  `localhost/<prefix>-sandbox-full:0.1.0`. The build is about 10 minutes when the supervisor
  layer is not cached (dnf Chromium, Claude Code CLI); `SKIP_IMAGE_BUILD=1` reuses it.
- (P2-TEST-1) Memory checksum trailer across crates (FUP-M5): `aurora-api` integration test
  `home_checksum` renders memory with the API's real `home::render`, installs the generation
  with the supervisor's real `HomeProjector` (which seals the trailer) and scans with the
  real `Harvester`: an unmodified projection is not harvested, and an appended line, a
  changed body under the old trailer, an altered digest and a removed trailer each are. It
  lives in `aurora-api` because that crate already depends on `aurora-supervisor`.
- (P2-TEST-1) **Running hosted e2e next to another run.** Every name and address of
  `scripts/e2e/hosted.sh` derives from `AURORA_E2EH_PREFIX` (default `aurora-e2eh`:
  networks `<prefix>-sbx`/`-db`, containers `-pg`/`-dbtarget`/`-anchor`, image
  `<prefix>-sandbox:0.1.0` unless the prefix is the default, and the installation token, so
  the `aurora.installation` label differs per run). Addresses: `AURORA_E2EH_SBX_BASE`
  (sandbox /24, default `10.89.251`), `AURORA_E2EH_DB_BASE` (default `10.89.252`),
  `AURORA_E2EH_API_IP` and `AURORA_E2EH_OPS_IP` (loopbacks in the shared rootless netns,
  defaults `127.0.0.41`/`.42`). Two concurrent runs need different values for all of them.
  Scenario s9 (session purge) no longer uses a 10 s idle stop: a `podman exec` right after
  a turn raced the idle stop (rc 125 once the container was gone), so it runs with the
  default idle window and parks the sandbox through the restart endpoint; the idle stop
  itself is covered by s4.

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

### Managed connectors (phase 4, P4-MCON, SEC-69, SEC-70)
- Catalogue: a host's `register` lists its types; a bad signature or `package.id` mismatch is
  rejected and omitted; a host older than `CONNECTOR_HOST_STALE` drops out (`host_running`).
- Add: managed create with the reviewed digest → `awaiting_registration`, no token; unknown type
  422; stale digest 409; missing required field 422 per field; supplied `generated_secret` 422;
  the generated value exists and differs per connector.
- Secrets (SEC-70): no admin response, audit row or captured log line contains a secret or
  generated value; PATCH replace / clear (required → 422) / regenerate bump the revision; a
  failing store → 503 and nothing changed; delete discards the sealed values.
- Delegated auth (SEC-69): `auh_` + `Aurora-Connector` works for a managed enabled connector;
  401 for self-hosted, disabled, unknown id, missing header; `aun_` + header ignored; disabling
  stops the connector being served and removes it from `assignments`.
- Assignments: long poll returns on change and on timeout; defaults applied (`public_url`);
  only types the calling host announced.
- Host (`aurora-connector-host` tests): runs only compiled types; starts, restarts on a new
  revision and stops instances; callbacks of two connectors served under their prefixes;
  status reported, sanitized.
- E2E: `scripts/e2e/kind.sh` adds the `fake` type through the admin API with the chart's host
  (`connectorHost.testTypes`), creates an integration and completes one message round trip;
  `channels.sh`/conformance unchanged for self-hosted connectors.
- Web: vitest for the add flow (type list, review, settings form with secret and generated
  fields, field errors, stale digest), detail edit/replace/regenerate/disable; Playwright
  `stage4-routes` and `stage4-axe` cover the new screens in both themes and at 390 px.

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

### Shared rate-limit counters (phase 4, P4-CNT, D-119, SEC-62)
- **Conformance suite** (`crates/aurora-counters/tests/it/conformance.rs`, one body run against every backend): `memory` always; `postgres` on the test database; `redis` against a `redis:7` container pinned by digest (gated `AURORA_TEST_REDIS=1`; the container is started by `scripts/e2e/` and by CI, like the Vault dev server). Cases: concurrent `fixed_window_consume` from 64 tasks never exceeds the limit and exactly `limit` succeed; all-or-nothing across several units (a refusal consumes nothing, including units earlier in the call); window rollover and `retry_after` from the store clock (no test depends on the replica clock); TTL expiry and pruning (`counters_prune` removes dead rows only); `bucket_take` burst, steady refill and all-or-nothing across buckets; `peek` has no side effect; large and unicode keys; error mapping (every transport error is `Unavailable`).
- **Two-replica proofs**: two API replicas (and two gateways) on one backend share one limit: the combined number of allowed requests equals the configured limit, not 2×, for a gateway rule quota, the `message` class and the unauthenticated per-IP limit; the same test on `memory` shows today's N× (proves the default is unchanged).
- **Fault injection**: stop Redis / break the counter pool mid-traffic. `local` keeps the per-replica limit (the sign-in limiter is never unlimited), recovers when the store returns (`aurora_counters_state`), no readiness change; `closed` answers 503 `counter_unavailable` with `retry-after`, never 429, only for requests that need a count; the breaker opens after `COUNTER_BREAKER_FAILURES` and probes after the cooldown; the `postgres` backend during a Postgres outage with the gateway `degraded` (`SEC-54`) falls back and never changes a policy decision other than the quota.
- **Gateway pipeline**: peek then consume; a blocked request consumes nothing; a request refused by a consume that lost a race gets `rate_limited` and a request-log row; held approvals consume once; `reason = counter_fallback` in the log on fallback; CONNECT counts as one request.
- **Startup**: `open`/unknown `on_unavailable` value, unknown backend, password in the Redis URL, plaintext Redis to a non-loopback host without `allow_plaintext`, `ratelimit.counters.redis.address` missing for `redis` — each refuses with a message naming the key. An existing `aurora.toml` and the unchanged Helm values render and start on `memory` (golden test).
- **Key privacy**: after running the sign-in and message flows with known e-mails and IPs, no stored key (Postgres rows, Redis `KEYS`) contains an e-mail, an IP, a token or a secret; two replicas with the same signing key derive identical hashes.
- **Performance**: the gateway budgets (`## Performance budgets`) are asserted on `memory` only; the `postgres` and `redis` backends add one round-trip to requests matching a rate-limited rule — measure when the machine is idle and record the number next to the budget, do not gate on it.

### S3-compatible blob store (phase 4, P4-S3, D-120, SEC-63)
- **Conformance** (`crates/aurora-blobstore/tests/it/conformance.rs`, one body over both backends): put/get/delete round trip with SHA-256 and size, overwrite refused (`If-None-Match`), get of a missing key, `list(prefix)` paging, 25 MiB object, concurrent puts. The `s3` run uses a **MinIO container pinned by digest** (gated `AURORA_TEST_S3=1`, started by the e2e script and CI like the Vault dev server; `MINIO_KMS_SECRET_KEY` set so SSE-S3 works) and, for failure cases, an in-process **mock S3** (throttling 503, 500, timeouts, a missing object, a server that ignores `x-amz-server-side-encryption`, a versioned bucket, Object Lock, wrong checksum).
- **Startup probe**: each refusal of `15 §S3-compatible blob store` (unconfirmed SSE, versioned bucket without the flag, Object Lock, missing bucket, refused credentials, plaintext remote endpoint, inline key, invalid prefix, unknown backend) fails with a message naming the key; `sse = "none"` starts with a warning; a loopback `http://` MinIO starts.
- **API on the `s3` backend** (`api/it/attachments_s3.rs`, the existing attachment tests re-run with the backend switched): upload, download, 410 after purge, quota (422 before the put, no overshoot under concurrent uploads), retention purge, delete one, conversation/workspace/org delete removes the objects, export zip, connector `file.fetch`, `send_file`; **store unavailable** (mock): upload 503 `blob_store_unavailable` with no row and an unchanged usage counter, download 503 (never 404/410), delete leaves an orphan the sweep removes, `readyz` unchanged, `aurora_blob_store_up` 0; checksum or `Content-Length` mismatch → 502 `blob_store_corrupt`; upload concurrency cap → 503 `blob_store_busy`; **no pre-signed URL**: no response, redirect, header, log line or audit row contains `X-Amz-Signature`, `X-Amz-Credential` or the endpoint host.
- **Isolation**: gateway, runner and sandbox never hold the credentials or reach the endpoint (NetworkPolicy render test; e2e-hosted probe resolves nothing); credentials absent from `GET /v1/admin/storage`, config dumps and logs (secret-grep over a run).
- **Orphan sweep**: unreferenced old objects deleted; unreferenced young objects (< `S3_ORPHAN_GRACE`) kept; referenced objects kept; keys outside the prefix untouched; 50 000-key cap; versioned mode deletes every version.
- **Migration** (`api/it/blobs_migrate.rs`): postgres → s3 and back; per-row verification failure leaves the row on its source and reports it; compare-and-set loses cleanly against a concurrent purge; resumable after a kill; `--dry-run` changes nothing; usage counters and quota unchanged; audit row without content.
- **Default unchanged**: with no `blobs.*` config every existing attachment test passes untouched; an existing `aurora.toml` and Helm values render and start.
- **e2e** (`scripts/e2e/` MinIO suite, gated): real stack, attachments through the dashboard API and a hosted turn (inbox delivery and `send_file`) on the `s3` backend; run when the machine is otherwise idle.

### Additional LLM providers (phase 4, P4-LLM, D-121, SEC-64, SEC-65)
- **Mock upstreams** (`aurora-testkit`, one per provider; no live network in CI): OpenAI/compatible (list, chat, responses, SSE), Azure (v1 routes, `api-key`), Bedrock runtime (SigV4-**verifying**: recomputes the signature from a known secret; event-stream responses) plus control plane and STS, Vertex (prediction routes, SSE) plus a token endpoint (verifies the RS256 JWT: `iss`, `scope`, `aud`, `exp`, signature against the test key). Each mock records what it received so tests assert the credential that arrived and what did not.
- **Per provider** (`crates/aurora-llm/tests/it/<provider>.rs`, gateway tests `gw/it/llm_<provider>.rs`): registry entry, config validation and every startup refusal of `15` (unknown key, credential-named key, non-https, bad region/location/project, empty `models` for Azure/Vertex, duplicate id), host reservation and exact-host Default Rule carve-out, model listing and its fallback, key validation (`llm_key_invalid`), key-origin verification and the response backstop with foreign accounts (OpenAI headers, STS account, service-account project), injection (agent-supplied `Authorization`, `api-key`, `x-api-key`, `X-Amz-*`, `x-goog-*`, `OpenAI-*` removed and replaced; SigV4 against AWS test vectors and the verifying mock; token mint, cache TTL, single-flight under concurrency, re-mint at skew, mint failure → `credential_unavailable` with nothing forwarded; `token_uri` pinning), streaming passthrough byte-for-byte with first-byte latency not delayed (SSE and event stream), `model_from_request` for every location form (body, percent-decoded Bedrock id/ARN, Vertex path, truncated body → refused under an allow-list), body above `LLM_SIGN_BODY_MAX`.
- **Path corpus** (SEC-65): allowed routes pass; refused: Bedrock guardrail/async-invoke/control-plane host, Vertex other project or location and non-prediction routes, OpenAI files/fine-tuning/admin, Azure management paths, compatible-endpoint admin routes (with the derived set and with `allowed_paths = ["/**"]`); the check applies only where Aurora injects (an agent's own key under `open` is unchanged); `litellm` and `anthropic` unchanged.
- **Governance matrix** re-run over the new endpoints: `sanctioned_only` with and without a key, endpoint not permitted, scope not permitted, origin mismatch/unverifiable, model allow-list; plugin claim of an endpoint host refused; an S3 request under Default Rule `block` still blocked while the Bedrock endpoint works.
- **Classification** table tests (`classify` for every documented provider error, shared by gateway and adapters), request-log `error_code` and `gateway_llm_upstream_total`; the gateway never changes an upstream status or body.
- **Usage**: a turn on each provider records `provider`, `endpoint_id`, `model` from the `ModelChoice` (a harness claiming another provider is ignored); `GET …/usage` aggregates by provider.
- **Sandbox isolation** (e2e-hosted with mock endpoints): a Bedrock and a Vertex turn through claude-code with the "skip auth" environment; seeded AWS secret and service-account private key found nowhere in the sandbox, transcripts, logs, request log or metrics; `client_env` contains no key-looking value.
- **Live suites** (`AURORA_TEST_LIVE_OPENAI=1`, `…_AZURE`, `…_BEDROCK`, `…_VERTEX`, `…_COMPAT`; not in CI): one real model call and listing per provider through a real gateway, used to pin the header, error-type and client-variable details the spec leaves to "pinned in the provider".
- **Default unchanged**: with the default `enabled_providers` the registry, `/v1/llm/providers` and every phase-0 provider test are untouched.

### Harness adapters (phase 4, P4-HAR, D-122, SEC-66)
- **Conformance**: `tests/conformance/harness` gains `codex`: CI runs it against a mock **Responses/chat upstream** in `aurora-testkit` (SSE, tool call, error injection: 401, 429, `insufficient_quota`, 5xx) through a real gateway; an env-gated variant (`AURORA_TEST_HARNESS_CODEX_LIVE=1`) runs it against a real OpenAI endpoint. Same cases as every adapter plus: steer refusal (`steer = false`, follow-up becomes a separate turn).
- **Adapter unit tests**: event mapping from recorded Codex JSONL fixtures (golden), exactly one terminal event after abort and after crash, effort table, session-id parsing and resume, `purge_session` (idempotent, only the named session), error mapping to `provider_*` through the shared table.
- **Rendering**: golden test of `config.toml` and the process environment for each dialect and for Azure/compatible endpoints: no secret, `OPENAI_API_KEY=aurora-placeholder`, `base_url` from `ModelChoice`, approvals/sandbox settings, MCP bridge entry; no `auth.json` created; `AGENTS.md` rendered, `CLAUDE.md` not (and the reverse for `claude-code`).
- **Selection**: `hosted.enabled_harnesses` validation (unknown id, default not enabled, `fake` gating); `PATCH` harness (422 not enabled, 409 active turn); boot document carries the id; unknown id or missing runtime → `harness_unsupported`; runner without the harness → `harness_unavailable` and no sandbox start; images without the label read as `claude-code,fake`; dialect intersection and first-of-harness-list choice; model override dropped with a notice after a switch; `no_model_key` when nothing is compatible.
- **Switch**: epoch bumped for every conversation, `harness_session_ref` cleared, purge rows queued and executed by the old adapter at next boot before any turn, instructions re-rendered for the new harness, transcript untouched.
- **e2e-hosted**: a Codex agent against the mock upstream (turn, tool call, `send_file`, platform tools, restart and resume) and the SEC-01 secret probe over its sandbox; run after P4-LLM lands.

#### Model readiness and credential-side grants (phase 4, FIX-019..FIX-021, D-136, D-137)

- **Readiness (API)**: one test per `reason` of `08 §Model readiness` and for `usable`; `default` for each step of the chain (override, endpoint default, provider default, allow-list, static list) and `null` for a LiteLLM endpoint with only `base_url`; the dispatcher fails a queued turn `no_default_model` in that last case and `no_model_key` with reason counts (and no key names) when every granted key is unusable — both before any sandbox start; with the agent model set the same turn is dispatched; `model-readiness` is `can_own` (permission matrix).
- **Config**: a `litellm` endpoint without `default_model` starts and logs the warning; `key_origin.method = "litellm-team"` refuses startup with the suggestion.
- **`litellm_team`**: against the kind mock's real LiteLLM, a **team key** verifies with its `team_id`; a **teamless key** verifies (`team_id = null`) without required team ids and is refused `key_reports_no_team` with them; `/key/info` 401 and an unreachable endpoint record `origin.detail`.
- **e2e (kind, llm-mock)**: the owner's production endpoint shape (`provider: litellm`, `base_url`, `key_origin.method: endpoint`, no `default_model`, no `models`, no `dialects`): store a key, grant it, chat → `no_default_model`, choose a model on the agent → the sandbox starts and the reply arrives; the conversation stream stays `Live` throughout.
- **Bulk grants (API)**: SEC-77 cases; idempotency; one audit row per new grant.
- **Dashboard**: `?tab=` selects each Credentials tab and an unknown value falls back; `no_model_key` link opens LLM keys and `no_default_model` opens Instructions; the "Give agents access" step after creating an LLM key, a secret and a connection (full, and `custom_only` with tools); the row Access action (list, remove, add); model readiness rows; axe and 390 px for the new step and dialog, both themes.

### Custom grants only and parameter-level matching (phase 4, P4-GRANTS, Q8, D-123, D-124, SEC-67, SEC-68)
- **Manifest and linter** (`crates/aurora-registry`): `grants` accepted and copied into the snapshot, defaults false, unknown keys and non-booleans rejected, `custom_only_without_tools`; a package without `grants` or `params` compiles to a byte-identical snapshot except for the explicit `grants` defaults (golden); every `params` lint rule of `09 §6`, fixture requirements, shadowing proof, wildcard with `params` rejected, `min_core` gating.
- **Grants API**: `full_grant_forbidden` on `PUT …/grants/connections/{conn}` and every other grant path (CLI, first-run, template, agent proposal); custom grants of a `custom_only` app unchanged (rules 1–3, new tools arrive blocked); `app_restrictions` in responses.
- **Evaluator corpus** (`aurora-policy`): `custom_only` inert full-style targets (derived and hand-written, `connection` and `app_connections`), rules with `tools` unaffected, warning `app_custom_only_target_inert`; `block_untooled` beats allow, approval, rate limit and hand rules, only for that app's winning connection, not at CONNECT time, trace and log reason `app_untooled_blocked`; apps without flags identical to before (the existing corpus unchanged).
- **Activation**: package upgrade introducing `custom_only` converts every full grant atomically with the activation (forced rollback leaves both unchanged), audit rows, held `converts_grants` with counts, no conversion back on removal of the flag, `block_untooled` converts nothing; two-gateway test that the snapshot and converted derived rules arrive together.
- **Gateway** (`gw/it`): an Indico-like fixture app with a broad token: untooled request never injected under `custom_only`/`block_untooled`; catalogued tools behave per grant.
- **`params` classification corpus** (`gw/it/params_matching.rs`, `aurora-policy`/`aurora-registry` unit tests): `equals`, `in`, `prefix`; query, urlencoded body and `x-amz-target` sources; every fail-closed case (missing, repeated, query/body conflict, other content type, compressed, truncated at `CONDITION_BODY_BUFFER`, invalid UTF-8, percent-encoding and `+` handling, case differences); fall-through to the generic write tool and to `tool = None`; body buffered only when a `form` tool may match; summaries via the `form` decoder; effective view and simulator synthetic requests include the parameters.
- **Compatibility**: a gateway without the classifier (older build in the rolling-upgrade test) refuses the snapshot and keeps the previous one; existing built-in packages and their fixtures pass unchanged.

### Groups (phase 4, P4-GRP-1, D-130, D-131, SEC-71)
- **Authz** (`crates/aurora-db/tests/it/authz_groups.rs`, real Postgres): the effective-role matrix — direct only, group only, both (strongest wins in either order), several groups, group `owner` vs direct `member`, removing the direct binding leaves the group role, removing the group leaves the direct one; org-role mapping (`member`→`admin`) makes `can_own` true everywhere and survives a direct demotion, a `suspended` member derives nothing from any group, a group never makes a non-member a member (the composite foreign key refuses the insert), leaving the org empties every membership, mapping `owner` is refused at the CHECK and in the service; the same-org composite FKs (a group cannot bind another org's workspace); `can_approve`, `can_see_conversation` and the fire-time check for a creator who only has group access; the API-key owner's group change reaches the key's next request. A zero-groups database yields the phase 0–3 answers for every existing authz fixture (golden run of the existing suite).
- **Policy corpus** (`aurora-policy`): `group` identity matches a speaker in the group, not another speaker, **not** speakerless traffic (boot, BYO, schedule, webhook, peer, system); OR with `user` and `agent`; allow rules naming only groups select no credential for speakerless traffic; block/approval/rate-limit group rules do not match it; **a rule whose only identity is a removed group matches nobody** (the dead id is in no principal set; an empty array is still "anyone"); `RULE_GROUP_IDENTITIES_MAX`; authoring 422 `unknown_identity` for a group of another org.
- **Gateway** (`gw/it/groups.rs`): the identity carries the speaker's group ids for a hosted context credential and none for boot/BYO/no-person speakers or a speaker without `can_use`; after a `user` invalidation mid-turn the next request re-resolves (member removed → the group rule stops matching, member added → it starts), `workspace` and `org` scopes do the same for binding and mapping changes; requests already in the pipeline finish with the old identity.
- **API** (`api/it/groups.rs`): the permission matrix per route (admin vs member vs workspace owner vs key), error codes and limits, `group_managed_by_scim` and the SCIM-disabled delete exception, optimistic concurrency, the impact view (counts, cap, `remove_member`), `GET …/access?expand=groups` and `derived_members`/`sources`, offboarding views carry `via_groups`, `/v1/me` groups and effective role, the `access` notification closes the removed user's open SSE stream and long-poll (final `access.changed`) while a still-authorized stream is untouched, every route writes its critical audit row in the same transaction (the audit coverage scan lists the new actions; forced audit failure → 503 `audit_unavailable` and nothing changed).
- **Dashboard** (`web/tests`): Groups list and detail (manual vs SCIM read-only), the Admin confirm, the impact dialog, the workspace Access "Members with access" source chips, the policy editor Group picker and the removed-group chip; both themes, 390 px, no horizontal scroll (`## Dashboard visual design`).
- **e2e** (`scripts/e2e/` extension, run by the lane that owns it): a member added to a group bound to a workspace opens it and chats; removed, the next request is 404; a rule naming the group injects a credential only for that member's turns.

### SCIM (phase 4, P4-GRP-2, D-132, D-135, SEC-72, SEC-73)
- **Conformance** (`api/it/scim_protocol.rs` plus an in-repo Rust SCIM test client built from the RFC 7644 examples; an external validator such as the Microsoft SCIM validator is run by hand against a staging instance before a release and is not a CI gate): discovery documents; `POST/GET/PUT/PATCH/DELETE` for Users and Groups; filter subset and `invalidFilter` for everything else; `startIndex`/`count`/`count=0`, stable ordering, empty page past the end, `count` clamping; PATCH `add`/`replace`/`remove` with and without `path`, `emails[type eq "work"].value`, case-insensitive `op`, `active` as boolean and `"False"`, atomicity (one bad operation applies none), `SCIM_MAX_PATCH_OPS`; unsupported attributes (`roles`, `password`) → 400; SCIM error format and `Content-Type: application/scim+json`; `406`/`415`/`413`.
- **Recorded IdP traffic** (`api/it/scim_idp_replay.rs`, fixtures `tests/fixtures/scim/{entra,okta,keycloak}/*.json`, request/response pairs reconstructed from each vendor's documented provisioning behaviour with synthetic tenants and users, no real data): initial sync (probe `GET filter`, `POST`, `409` → `PATCH`), attribute updates, deactivate/reactivate, group create and member churn, a token rotation mid-sync, a repeated `POST` after a lost response. Replay asserts final state and the exact statuses the IdPs rely on.
- **Auth and scope** (SEC-72, `scim_auth.rs`, `scim_scope.rs`): `/scim/v2` is 404 with `scim.enabled = false` and 401 (identical body) for no token, malformed, unknown, revoked, expired, other-prefix (`auk_`, `aug_`) and org-disabled tokens; an `aud_` token on `/v1/**` is 401 and a session cookie on `/scim/v2` is ignored; token lifecycle (owner only, session only, recent re-authentication, hash only in the database, shown once, expiry cap, five-token limit, expiry notice and audit rows 14 d and 1 d before); org A's token cannot read, filter or write org B's users and groups (404, never 403); manual groups invisible; no way to set a role, create an owner or admin, or mutate an owner (PUT/PATCH/DELETE → 403, last-owner untouched); e-mail domain allow-list and instance allow-list; seat limit; per-token rate limit and 429 with `retry-after`, bad-token IP limit; body, operation, member and page limits; audit coverage scan for the new actions and forced audit failure (nothing applied, 503); token never appears in any response, log line or audit row.
- **Linking and deprovisioning** (SEC-73, `scim_link.rs`, `scim_deprovision.rs`): a SCIM user is pre-provisioned unverified and cannot act before sign-in; the OIDC sign-in with the verified e-mail (and with a trusted-domain e-mail, D-118) binds it, an unverified claim or a matching `externalId` alone does not; a signed-in user's `users.email`/`display_name` are unchanged by SCIM updates; an existing user is linked without touching their row; the full effects matrix of `04 §SCIM` for suspend and remove (membership, sessions rule with one and with two orgs, key revocation including after reactivation, running speaker turns `user_deprovisioned`, group-derived access, automations disabled at fire time and listed in the offboarding view, pending invitations cancelled); JIT and invitation acceptance refused for a suspended or removed link, allowed again after the IdP re-provisions; reactivation only when `suspended_by_scim`; `exclusive` mode; owner untouched; the person's other org is unaffected.
- **Groups through SCIM** (`scim_groups.rs`): SCIM groups appear in `/v1` as `source = scim` with read-only name and members (`group_managed_by_scim`), members must be this org's links, removed links rejected, `PUT` replaces atomically with one audit row per change, deleting in the IdP deletes bindings, the effect on access and gateway identities is that of `## Groups`.
- **Dashboard** (`web/tests`): Directory sync page (hidden when the instance switch is off, owner only, token shown once, expiry banner, status card), Members "Directory" badges and the offboarding BYO list; both themes, 390 px.
- **Edge**: the Helm Ingress/HTTPRoute renderings include `/scim/v2` only with `scim.enabled` (edge-routes guard).

### Approval park-and-resume and Web Push (phase 4, RFE-011b, D-133, D-134, SEC-74, SEC-75, SEC-76)
- **Parking decision** (`gw/it/approval_park.rs`, real Postgres, mock upstream): with parking off the hold is byte-for-byte the phase 2 behaviour (existing suite unchanged); each eligibility condition has a positive and a negative case — timeout at, above and below `threshold_secs`; BYO, boot-token and conversation-less requests held and clamped; body at and over `max_body_bytes`, chunked/streamed and upgrade requests held; a verdict that differs without a speaker (a `user` or `group` rule) held; per-agent pending and hourly caps → 403 `approval_limit_reached`. A parked request is answered 403 `approval_parked` with `x-should-retry: false` and `details`, **nothing is forwarded upstream** (mock upstream sees zero requests), no row for the body, and an identical repeat returns the same `approval_id` and creates nothing.
- **Grants** (`gw/it/approval_grants.rs`): approval → `resume_state = ready`; the identical retry in the resume window is consumed and forwarded exactly once (32 concurrent retries → exactly one forwarded, the rest get a new approval/park); fingerprint sensitivity — each of agent, conversation, method, host, port, path, query (value and order), one byte of body, rule, credential, tool changes the fingerprint and the retry is not matched; the window end (`resume_state = expired`); a policy change between approval and retry to `block` (blocked), to `allow` (allowed, grant unused), to another requiring rule or credential (not matched); a retry from another conversation of the same agent not matched; consumption before refresh and forwarding (a forced upstream or refresh failure still leaves the grant consumed); deny cooldown → `manual_approval_denied`; a deadline expiry of a parked approval (`05` job) with no gateway involved; the stale-holder sweep and SIGTERM never abandon a parked approval; a freeze abandons pending and revokes ready; a database outage parks and consumes nothing (SEC-54 still holds).
- **API** (`api/it/approval_park.rs`): decide approve/deny/expire for parked approvals under the same decider matrix (RFE-012, RFE-009, groups); the system follow-up (steered into a running origin turn, else a queued system turn; none for a frozen agent or an expiry; text platform-composed, canary strings in the request path/title do not appear beyond the sanitized summary); `approval.pending/decided/expired` transcript events and DTO fields (`parked`, `resume_state`, never fingerprint or salt); the stall exemption does **not** apply to parked approvals while it still applies to held ones; cards for parked approvals settle in all four outcomes; critical audit rows for decisions; the reminder is sent once and only without a live watcher.
- **Sandbox lifecycle** (`api` + `fake` runner): a parked approval is not a keep-awake reason (the sandbox parks after `IDLE_STOP`), and the resume follow-up starts a new boot with the harness session restored (`harness_session_ref`); no process or connection of the old boot is needed.
- **Web Push server side** (`api/it/web_push.rs`, a mock push service in the test kit): RFC 8291 Appendix A and RFC 8292 vectors; the mock verifies the VAPID JWT (`aud`, `exp` ≤ 1 h, `sub`, key id), decrypts with a test subscription key pair and asserts the exact payload fields; **canary test**: approvals whose title, summary fields, rule name, host, path, query and body contain unique strings produce pushes none of which contain them, with and without `show_details`, and `show_details` adds only the sanitized agent and workspace names; routes (limits, moves, validation errors, ownership 404, session-only, `push_unavailable` when off); delivery rules — decider at send time (lost `can_approve`, direct-conversation visibility, group-derived access), live watcher suppresses, session revoked/expired/sign-out stops delivery, endpoint re-registered by another user moves, approval decided/expired/frozen before the delay sends nothing, claims are idempotent across two replicas, batches fold into one push, hourly cap and digest, the e-mail path is unaffected; failures — 404/410 deletes, 429/5xx retry bound, five failures disable; key handling — first-start generation under concurrent replicas yields one key, the private key and subscription secrets are sealed and absent from every response, log and audit row, rotation (old-key subscribers still delivered, 409 `push_key_stale`, silent migration, retired key destroyed), operator-managed key mode.
- **SSRF** (`api/it/web_push_ssrf.rs`): registration refuses http, userinfo, IP literals, private/loopback/link-local/metadata addresses, non-allow-listed and look-alike hosts, over-long URLs; a send re-validates (an allow-list change takes effect), a host that resolves to a private address (DNS rebinding after registration), a redirect and a slow or oversized response are all refused or bounded; startup refuses an empty or invalid `push.allowed_endpoint_hosts`; with `push.enabled = false` no outbound call is ever made (network-denied test).
- **Dashboard and service worker** (`web/tests`): the Notifications card states (unsupported, insecure origin, permission denied, iOS Home Screen note, registered, paused, not receiving, key rotated → silent re-subscribe), the gesture requirement, sign-out removes the device; the service worker (unit tests in a service-worker test environment) shows the generic notification for a malformed or unknown-version payload, builds text only from the fixed fields, `notificationclick` navigates only to a same-origin path starting with `/o/` and falls back to `/` for `//evil`, `https://…`, `javascript:`, backslashes, and has no `fetch` handler; parked chips and sentences on the Approvals page, chat and overview; both themes, 390 px.
- **e2e** (extension of `scripts/e2e/`, owned by its lane; the browser-push leg uses the mock push service and headless Chrome's push emulation): a rule with a 2-hour timeout and parking on → the agent is refused `approval_parked`, ends its turn, the sandbox parks, the approver approves from a card, the system follow-up wakes the agent and the identical request succeeds exactly once.

### Port previews (phase 4, P4-PRV-1, P4-PRV-2, D-144..D-147, SEC-80..SEC-82, SEC-85)
- **Shares, API** (`api/it/previews.rs`, real Postgres, `fake` runner): off by default (no
  `previews.enabled` → every route 404 `previews_disabled` and `GET /v1/instance` says so; org
  switch off → 409 `previews_not_enabled_for_org`); create/list/patch/delete matrix (`can_use`
  member yes, non-member 404, **agent tool call and agent credential refused** (SEC-80), API key
  allowed for management only); BYO agent 422; port bounds and the supervisor-reserved ports 422;
  duplicate port 409; per-agent and per-org limits; label shape (`p` + 24 base32), uniqueness
  under 1 000 creations, **never reused** after delete and re-create; visibility accepts only
  `private` (a `public`/`org` value is 422, the CHECK constraint rejects it in SQL); start
  command validation (length, newline, cwd traversal) and the audit rows (`preview.share.*`,
  `org.previews.update`, security-critical, same transaction; the command text is **not** in any
  audit row, only its hash prefix).
- **Lifecycle** (`api` + `fake` runner): park/wake keeps the share and the hostname; a wake demand
  starts a stopped sandbox once and honours the start backoff and `PREVIEW_WAKES_PER_SHARE_HOUR`;
  `preview.start` is emitted only after the `previews` part is applied, only for a share with a start
  command whose port is not listening, at most `PREVIEW_START_ATTEMPTS` per window; effectively
  frozen (agent, workspace, org): no wake, no `preview.start`, allow-list empty, 423, share kept, and
  unfreeze restores it with nothing replayed; paused share; org switch off/on; agent/workspace/org
  delete cascades (sessions too); a preview never creates a turn or stamps `last_engaged_at`; the
  `preview` keep-awake reason lasts `PREVIEW_AWAKE_GRACE` after the last heartbeat and ends at
  `PREVIEW_AWAKE_CAP` (a held WebSocket cannot keep the sandbox awake forever); `ports` list
  carries `shared`.
- **Supervisor** (`sup`): the share list lives in memory only (a file planted in the home changes
  nothing); `preview.start` for an unknown port, a share without command, an already listening port or
  an empty list is a no-op; a started command runs as the agent user with the container environment (no
  `AURORA_*` variable, no platform credential) as process `preview:<port>`; the frame carries no
  command text; `ports.state` reports the boot's loopback listeners (unit tests parse
  `/proc/net/tcp{,6}` rows incl. v6 and junk; an IT proves first-report-at-boot, no re-send
  without a change and a re-send after the port closes).
- **Config** (`config` crate): `previews.enabled` without a valid domain refuses to start; domain
  equal to, containing or under the dashboard host, a trusted origin or the gateway host; one label;
  public suffix; upper case; too long.
- **Browser authorization** (`api/it/preview_auth.rs`, real Postgres; the relay logic is
  exercised through the relay crate's test harness with the API in process): the full grant
  exchange (no cookie → state cookie + `302` to `authorize` → `302` to `/_aurora/grant` →
  cookie → path); matrix of failures with the uniform 404 (unknown label, instance/org off, paused,
  no access, member of another workspace); grant replay (second redeem 400), expiry, wrong host, tampered
  seal, wrong user, session revoked in between, missing or mismatched state cookie (login CSRF), a
  `path` that is `//evil`, absolute or under `/_aurora/`; the redirect target is always `https://<label>.<domain>`
  (property test with hostile `host`/`state` parameters); cookie attributes of `__Host-aurora_preview`
  and `__Host-aurora_pstate`; **every Aurora cookie lacks `Domain`**, and a request to a preview
  host in the browser-level e2e never carries `aurora_session`; revocation: sign-out, session revoke,
  member suspended/removed, `can_use` lost through a group, share paused/deleted, freeze, org switch off →
  next resolve denies and open streams close within `PREVIEW_AUTH_CACHE`; preview credential is
  accepted by no API route; `csrf_rejected` for `Sec-Fetch-Site: same-site` XHR with the session cookie
  while previews are on (and no change while off); `preview.session.create` audit.
- **Relay** (`relay` crate tests with a fake runner and fake API): Fetch Metadata matrix (navigation
  allowed; same-site/cross-site fetch, form post, iframe, image refused), WebSocket `Origin`; header hygiene
  both ways (cookie stripped, Aurora headers never forwarded, `Host` kept, `X-Forwarded-*` set, hop-by-hop
  removed, `Set-Cookie` `Domain` removed and `__Host-aurora_*` dropped, cache/robots/HSTS headers); smuggling
  corpus (CL.TE/TE.CL, duplicate `Content-Length`, line folding, absolute-form, `Host` mismatch, `/_aurora/` traversal);
  streaming of large bodies without buffering (memory stays flat for a 1 GiB response), range requests,
  chunked, HTTP keep-alive over one stream, WebSocket echo, server-sent events, slow-loris, idle/lifetime
  timeouts, per-share/sandbox/user stream limits, daily byte quota; wake wait (parked sandbox → 503 page with
  refresh after 45 s; success when the port starts in time), start-command exit page, uniform 404/423 pages,
  resolve cache (15 s) and revalidation closing streams. Where they live (FIX-072/073): `relay
  tests/limits.rs` (per-share and per-user stream caps, 429/`4409`, `preview_quota` refusal, grant
  rate, byte report; the shared quota counter and its two-replica case are `api it/preview_auth.rs
  quota::*`), `tests/behaviour.rs` (slow-loris, plain/upgraded idle, lifetime, SSE, ranges,
  keep-alive, WebSocket `Origin`, revocation within `PREVIEW_AUTH_CACHE`, wake-wait page, start-command
  exit page; time limits shortened through `Streams::with_timeouts`), `tests/big_body.rs` (1 GiB
  in a flat resident set).
- **Streams** (`run` and `sup` crates, the W6 mock): `stream.open` for an unshared port, a reserved
  port, with an empty list, after the share was removed → refused; connect only to loopback (a share whose
  port is bound only on another interface fails, no other address is tried); credit accounting (a sender never
  exceeds the window, a credit above the window or a data frame without credit closes the stream, 64 concurrent
  streams × 128 KiB bound), round-robin fairness under a hostile fast stream, binary framing, control frames
  outside seq/ack (a W6 reconnect ends every stream, nothing is replayed), runner redeems the grant and ignores
  relay parameters (stolen grant opens only its own share; expired, used, stale boot, stopped sandbox,
  frozen); an older supervisor answers nothing → `4502`; the stream listener is unreachable from the sandboxes
  network (kind/e2e and a guard in `scripts/guards`), TLS option.
- **Helm** (`deploy/helm` render tests, part of the edge-routes guard): with `previews.enabled=false` the chart
  renders no relay, no wildcard host, no NetworkPolicy change (golden diff against the current render);
  enabled renders exactly the relay Deployment/Service/PDB, the wildcard host routing only to the relay
  (Ingress and HTTPRoute), the NetworkPolicies of `15 §Port previews`; `previews.enabled` without a domain or with
  a domain under the dashboard host fails the render and the API start.
- **Dashboard** (`web/tests`): tab absent when previews are off/the org has them off/BYO; add dialog
  with listening-port chips and error mapping; copy/open (rel attributes); frozen read-only; org
  settings page and confirmation; no horizontal scroll at 390 px, both themes.

### Web terminal (phase 4, P4-TERM, D-161, SEC-87)
- **API** (`api/it/terminal.rs`): the WebSocket upgrade auth matrix (valid session of a
  `can_use` caller upgrades; API key, agent token, anonymous, cross-origin `Origin` → refused;
  BYO 422 `terminal_not_supported`; disabled 404 `terminal_disabled`; frozen 423; parked →
  wake + upgrade or `{waking: true}`); grant mint (single row, hashed, 30 s TTL, audit row
  `agent.terminal.open` with no content), single-use redemption (second redeem 403), stale
  boot and unknown agent refused; caps `TERMINALS_PER_AGENT`/`_PER_USER` → 4409; the API
  proxies bytes opaquely and **never** logs them (secret-grep over a run with secrets typed
  into the terminal: no API log, no request-log row, no transcript event).
- **Runner** (`run` terminal tests): `kind: "terminal"` redemption on the stream listener
  (single-use, boot-bound; a port grant never opens a terminal and vice versa); unknown kind
  refused by an older runner (API answers 4502); `pty.open` only on the channel of the bound
  boot; close codes 4403/4404/4409/4502/4503/4504; W6 reconnect closes every PTY with 4503;
  keep-awake stamping (`terminal_active_until`) and the cap.
- **Supervisor** (`sup` terminal tests): PTY spawn as the agent user, cwd `/workspace`,
  `TIOCSWINSZ` on open and resize, echo round trip through the credit machinery (window
  bounds hold under a hostile fast writer, tag `0x02` framing beside a concurrent W18
  `0x01` stream), `pty.refused` reasons (`limit`, `disabled`), environment probe — no
  `AURORA_*` variable, no boot-document trace in the PTY's `/proc/self/environ` (SEC-34
  probe), `shutdown` closes every PTY (`pty_leak::shutdown_closes_every_pty`; FIX-073 made the
  supervisor do it). **Idle and lifetime closes are the runner's** (`stream_listener.rs`; the
  supervisor's `pty.rs` has no timers): `run it/leg_limits.rs`
  `terminal_leg_closes_4504_when_idle` / `_at_its_lifetime` with shortened limits
  (`StreamListener::with_limits`), and `older_supervisor_ignoring_pty_open_ends_in_a_clean_refusal`
  (an older supervisor never answers `pty.open`; the dial is refused pre-101 and the PTY released).
- **Dashboard** (`web/tests`): tab gating (off/BYO/no-`can_use` hidden; frozen banner),
  "Waking…" → attach, resize round trip sends `pty.resize`, reconnect notice, session-end
  notices, both themes, the CSP render test asserts `'wasm-unsafe-eval'` present and nothing
  else new, vendored WASM served same-origin (`no third-party font/CDN` rule holds).
- **kind e2e**: terminal round trip on a running sandbox (`echo aurora-terminal` output
  asserted), freeze closes the session, a parked sandbox wakes on open; `kind.sh` env
  overrides (own cluster; ask the coordinator first).

### Git skill library (phase 4, P4-GSL, D-148, D-149, SEC-83, SEC-84)
- **Fake git host** in the test kit (`tests/kit`): serves a GitHub-shaped and a GitLab-shaped API (ref
  resolution, recursive and paginated trees, blobs, raw files, rate-limit responses, truncated trees, a
  moving head, renames/deletions, symlink and submodule entries, oversize files, hostile names and
  descriptions) behind the gateway's guarded resolver (loopback allowed only in the test configuration).
- **Registry** (`reg/it/library.rs`): the `library` linter matrix of `09 §3.4 library` (non-exact or
  wildcard host, host not in `hosts[]`, free placeholders, non-GET, JSON path shapes, bad
  `repo_pattern`, unknown keys), snapshot copy, re-approval when the block changes, the built-in GitHub
  and GitLab packages validate with their fixtures (resolve, tree incl. pagination, blob; instance-bound
  GitLab host).
- **Gateway** (`gw/it/library_fetch.rs`): parameter fuzzing → 400 `library_param`; connection checks (other
  org, workspace-scoped, disabled, other app, no `library` block) → 409/404; the request is built from the
  template (the fake host sees exactly one `GET` per step with the injected credential, no agent headers);
  redirect not followed; size/time caps; OAuth refresh and `reauth_required`; the route needs the internal
  secret and is not on the proxy listener; no log line or response contains a credential; rate limits.
- **API** (`api/it/skill_library.rs`, real Postgres, the fake host, a fake gateway client where useful):
  create/list/patch/delete with the permission matrix (org admin yes, member no, workspace-scoped connection
  422, limit 409), browse (live, nothing stored, validity and reasons, caching 5 min), **add** at each tier
  with its authority (agent `can_use`, workspace `can_own`, org admin; agent tool and agent credential
  refused; the exact requested commit is read even if the head moved), validation (symlink, submodule, big
  file, bad frontmatter name, reserved name, size limits), uniqueness and shadowing across tiers unchanged,
  **budgets** per agent/workspace/org at add and approve, **read-only** enforcement through every route and tool,
  detach and library deletion; **refresh** under the lease (two replicas never refresh one library; lease
  expiry; backoff; unchanged head costs one call; changed blobs → one proposal; superseding; deletion →
  `removed`; invalid → `invalid`; over-budget; connection revoked/deleted/`reauth_required`; package without
  `library`; interval floor) with the invariant checks of SEC-83 (no change to `skills`, `skill_files`, any
  home-sync generation or the sync parts until approval); **approve** (session only; hash quoted; race with
  a newer proposal → 409 `update_superseded`; re-validation; audit rows critical; generation bump for exactly
  the affected agents), reject remembered, `skill.library.update_proposed` best-effort.
- **Supervisor/projection** (`sup`, `api` + `fake` runner): library skills are projected exactly like manual
  skills (read-only modes, manifest pruning, tier precedence); a pending proposal changes nothing in the home.
- **Dashboard** (`web/tests`): libraries page and connect dialog (errors in words, read-only warning), Add from
  library flow (preview of the exact stored content, over-budget disabled), badge and read-only banner, detach and
  remove confirms, update review (full diff, disabled approve for non-authority with explanation), repository text
  rendered as data (an HTML/markdown/URL payload is inert), no horizontal scroll at 390 px, both themes.

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
- (P2-TEST-1) The limiter refills on the wall clock, so a test that spends a burst and
  expects the next request to be refused must remove the refill (class overrides of one
  token an hour keep the default burst sizes); `each_class_has_its_own_limit` did not and
  failed once under load (the default `write` class refills 2 tokens a second).

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
  = false`); the dev IdP (`aurora-dev-idp`) and the sign-in helpers (`Harness::sign_in` in
  the API suite, `aurora_testkit::oidc::sign_in_over_http` for the e2e crates, a `login_hint`
  route for the Playwright flows) — none of them is a bypass: they drive the real
  `start`/`callback`; API keys and CLI device login unchanged.

- (phase 2, RFE-008) Atomic audit (SEC-32): the audit coverage test enumerates
  `SECURITY_CRITICAL_ACTIONS` against the route catalogue (every critical route
  produces its row with `critical = true` in the same transaction); a
  fault-injection hook makes the audit insert fail and every critical action
  must return 503 `audit_unavailable` with **no** state change (policy publish,
  grant put, role change, approval decide via dashboard/API/channel, plugin
  approve, freeze, governance update); best-effort actions still succeed with the
  hook on (`aurora_audit_dropped_total` increments); a repository-signature test
  (critical mutations take the audit entry) guards against future bypass.

### Dashboard visual design (phase 2, P2-UI)
- Token guard (vitest): no component or page under `web/src` uses a Tailwind
  palette colour utility (`neutral-*`, `slate-*`, `red-*`, `amber-*`,
  `emerald-*`, …) or a hex colour literal in a class name. Only
  `globals.css` defines colours (`14 §Visual design`). The one exception is
  the brand colours inside the `AuroraMark` and `AuroraLogo` components.
- Accessibility (Playwright + `@axe-core/playwright`): Overview, Approvals,
  the agent Chat tab, an org page and sign-in, at 1280 px and 390 px widths,
  **in both light and dark themes**, have zero `serious`/`critical`
  violations, including `color-contrast`.
- Theme (Playwright):
  - with no stored preference, the page follows `prefers-color-scheme` (emulate
    both), and switching the emulated scheme while on "Match system" re-themes
    the page live;
  - choosing Light or Dark in the theme switch persists across reloads in that
    browser and overrides the system setting;
  - on a reload with Dark stored, `<html>` already has `data-theme="dark"` in
    the server-rendered HTML response, or is set before the first paint with
    no light frame (check the attribute before hydration);
  - the theme switch is a radio group operable by keyboard (arrow keys, Space),
    and each option has an accessible name;
  - no theme request reaches the API.
- Brand: the favicon and app icons are served from the dashboard origin, and
  `AuroraMark` and `AuroraLogo` give two instances on one page distinct gradient ids. No page loads a wordmark font.
- Narrow shell (Playwright, 390 px): the menu button opens the nav dialog,
  focus moves into it, Esc closes it and focus returns to the button. There is
  no horizontal page scroll.
- Fonts and CSP: the production CSP string is unchanged by the redesign. Font
  files load from the dashboard's own origin and no request goes to a third
  party.
- The existing component and e2e suites keep their role- and label-based
  assertions. Selectors change only where the copy or structure changed by
  design, and no assertion is removed to make a test pass.

### Architecture guards (CI scripts)
- No app/platform names in core paths (SEC-33).
- Dependency graph: `aurora-gateway` and `aurora-api` do not depend on
  `wasmtime`; `aurora-policy` does not depend on `aurora-db`.
- Edge configuration covers all route prefixes (`15`).
- Every table in `03` exists in migrations and vice versa (schema introspection
  diff against a machine-readable extract of `03`).
- **A guard must not pass vacuously (FIX-068).** The Helm guards (`edge-routes`, `hook-policies`,
  `sandbox-network`, `dev-gateway-address`, `no-dev-idp-in-release`) **fail** when `helm` is missing
  (`scripts/guards/lib.sh` `require_helm`); the only opt-out is `AURORA_GUARDS_SKIP_HELM=1` on a box
  without helm, and it is ignored when `CI=true`. Render output is captured before it is grepped
  (`out=$(helm template …)`; `grep -q … <<<"$out"`): `helm … | grep -q` under `pipefail` is green exactly
  when the forbidden resource rendered (SIGPIPE). `hook-policies` also fails if it inspected no pre-upgrade
  hook Job with a `serviceAccountName`, and on a `matchExpressions` operator it cannot evaluate.

## Performance budgets (gateway)

- Added latency for a cached, allowed, bodyless request with injection:
  p50 < 1 ms, p99 < 5 ms (excluding upstream).
- 2 000 requests/s per gateway core on cached paths.
- Connect-time resolution (cache miss): p99 < 25 ms with 1 000 rules and 500
  connections in scope (excluding secret-store latency).

## How the Rust tests are laid out and run (OPS-016)

- **One integration-test binary per crate**: `crates/<c>/tests/it/main.rs` (or `services/<s>/tests/it/main.rs`)
  declares `mod <file>;` for every file next to it, and shared helpers live in `tests/it/common/` (and
  `tests/it/peers_rig/` for the API). A new test file is a new `mod` line in `main.rs`; do not add `tests/*.rs`
  files, each one is a separate multi-hundred-MB link. Crates with a single test file keep it as is.
- **Standalone test targets** (kept out of `it` on purpose): `services/aurora-api/tests/sec10_logs.rs` (installs a
  scoped tracing subscriber, which must not race other tests' callsite registration), `services/aurora-supervisor/tests/conformance.rs`
  (live-gated), and the e2e crates (`tests/e2e-gateway`, `tests/e2e-hosted`, `tests/conformance/connector`).
- **Test ids**: a test in `tests/it/foo.rs` is `foo::name`. Run one with
  `cargo test -p aurora-api --test it -- foo::name`, a file with `-- foo::`, the schema check with
  `cargo test -p aurora-db --test it schema_matches_spec`. `docs/tests/*-coverage.md` refer to files as `api/it/foo.rs::name`.
- **Whole workspace without the gated e2e crates**: `just test` (= `cargo test --workspace --exclude aurora-e2e-gateway --exclude aurora-e2e-hosted`).
  `just test-nextest` runs the same set under cargo-nextest when installed (`.config/nextest.toml`; no retries; 10 min per-test kill).
- **Build speed and disk**: see `docs/ops/build-performance.md` (linker, `dev-fast` profile, optional sccache, lane clean-up).

## Velocity: fast local commands (VTL-001)

**VEL-002 (owner, 2026-11-12): the local dev loop must not slow down.** New
test tiers must respect the environment split the repo already has: in-process
tests belong in the crates' `tests/it` binaries and run locally; anything that
starts a container or binds fixed host addresses belongs in the excluded e2e
crates (`tests/e2e-gateway`, `tests/e2e-hosted`) and runs in CI's serial jobs,
env-gated so a bare local invocation skips gracefully
(`AURORA_E2E_DB_TARGET` pattern). When a spec adds tests, it names the tier
and the measured wall-clock delta for `just test`. Applied by TEST-021
(gateway wire conformance: tier-1 oracles < 5 s local; nginx/haproxy
front-door containers CI-only).

The developer-loop wins, in the order they matter (measured numbers in
`docs/ops/build-performance.md`; this host is 16 cores shared with worker
lanes, so absolute times vary):

1. **Run tests under cargo-nextest.** Same test set as `cargo test` (doctests
   excepted, as always with nextest); each test runs in its own process with
   its own `aurora-testkit` database, so parallel processes are isolated.
   Install once (`cargo install cargo-nextest --locked`, or the prebuilt
   binary from <https://nexte.st>), then `just test-nextest` / `cargo nextest
   run`. `.config/nextest.toml` caps parallelism at 8 processes (the shared
   dev Postgres has `max_connections = 100` and every lane's tests share it;
   16 parallel processes hit "too many clients"), keeps `retries = 0` — a
   flaky test is a bug, not noise — and kills a hung test at 10 minutes.
2. **Keep sccache warm.** `scripts/dev/rustc-wrapper.sh` routes every rustc
   call through sccache when it is installed; a fresh worktree with a warm
   cache compiles its dependency graph in roughly half the time. Leave the
   cache at its 30 GB cap (`SCCACHE_CACHE_SIZE`, set by the wrapper).
3. **Link with lld.** `scripts/dev/linker.sh` picks `clang -fuse-ld=lld` when
   both are installed (they are, on this host) and falls back to `cc` on
   machines without them, so nothing to configure; `AURORA_LD=mold|system`
   overrides. Measured (VTL-001, one `aurora-api --tests` relink): lld and
   mold are within noise of each other at the current debuginfo profile —
   `it` links in ~4.4 s under either, smaller binaries faster — and links are
   a small share of a build, so lld stays the default. The linker is plumbing,
   not the bottleneck.
   **Incremental compilation is on for `dev`/`test` (VEL-004, I-45).** A
   one-line change in `aurora-api` recompiles in ~35 s instead of ~250 s
   (shared host, see build-performance.md §6). It applies to workspace crates
   only; CI keeps `CARGO_INCREMENTAL=0`. Do not export `CARGO_INCREMENTAL=1`
   locally (it would make registry deps uncacheable); run `just clean-stale`
   when `target/` grows.
4. **Playwright mock suites run 4 workers** (`web/playwright.config.ts`,
   `fullyParallel`); the dev server under test is a single `next dev` process,
   so more workers only queue there. Real-stack journeys
   (`first-run.spec.ts`, `m1.sh`) stay one-file-at-a-time scripts. Override
   with `AURORA_E2E_WORKERS`; `AURORA_E2E_RETRIES=0` for a strict no-retry
   run.

5. **Pre-commit, run the quick gate, not the suite** (VEL-002). Since the
   full suite became CI's job, the local pre-commit loop is
   `bash scripts/dev/quick-gate.sh` (`just quick`): fmt, clippy and tests
   for the crates the change touches, plus web typecheck and the related
   vitest files, in minutes instead of the ~1¾ h a cold full local suite
   takes. See §[Quick gate (local loops)](#quick-gate-local-loops-vel-002) —
   a development-loop tool, not a merge gate; `--all` still runs the old
   full pre-commit suite when wanted.

## Quick gate (local loops) (VEL-002)

`bash scripts/dev/quick-gate.sh` is the fast pre-commit loop for the owner:
it detects which crates a change touches (working tree + index + untracked
files vs HEAD; on a clean lane, the commits since the merge-base with
`origin/master`), prints a plan, and runs only what that scope needs —
`cargo fmt --all --check`, `cargo clippy -p <crate> --all-targets
--all-features -- -D warnings` and `cargo test -p <crate>` per affected crate,
plus `npx tsc --noEmit` and `npx vitest` for `web/` changes. Workspace-wide
files (`Cargo.lock`, toolchain, lint config) widen clippy and tests to the
whole workspace minus the e2e crates; a data-model change adds the schema
guard. No mapped scope at all (a docs-only change, or a clean tree on
master) still runs a core set — clippy on `aurora-db` and `aurora-protocol`
— so the gate never silently does nothing. Selectors: `--all` (the old full
pre-commit suite: workspace clippy, all non-e2e cargo tests, web lint +
typecheck + full vitest, architecture guards), `--clippy-only`,
`--tests-only`, `--web`; `AURORA_QUICK_GATE_VERBOSE=1` lists every changed
file before running. Tests and clippy find Postgres through the repo
convention (I-05: `.env`, else the environment, else the documented
aurora-dev-pg URL); sqlx::test uses scratch databases only. Every cargo
command runs from the repo root (no `--manifest-path`), so
`.cargo/config.toml` — the lld linker and the sccache rustc-wrapper — stays
in effect, and heavy commands run under `nice -10` with a load gate for the
shared host. This vitest is 4.x, where `related` is a subcommand
(`vitest related <files>`), not a `--related` flag; changed test files run
directly. When the test scope includes `aurora-api` (or the whole workspace)
and `plugins/fixtures/dist/` is absent — it is gitignored; CI builds it with
`scripts/fixtures/build.sh` before its test shards — the gate builds it once
first, so the `connect::` tests do not fail on a missing package. Measured on
the 16-core dev host (VEL-002, 2026-11-12): a single-crate change ≈ 80 s warm
/ 4 s when already compiled, a web change ≈ 75 s, `--all` ≈ 102 min on a cold
tree (the target is ≤ 3 min for a single-crate change; `--all` is the escape
hatch, not the loop).

**Scope rules, nextest, flags (VEL-005).** Clippy covers the touched crates
*and their workspace dependents* (taken from `cargo metadata --no-deps`; the
plan lists each added crate and the dependency that pulled it in), so a change
in `aurora-core`, `aurora-db` or `aurora-protocol` can no longer pass while
`aurora-api` or `aurora-gateway` stop compiling; the e2e crates are never
pulled in. Tests stay on the directly touched crates, plus `aurora-db` and
`aurora-api` when `crates/aurora-db/migrations/**` changed (migrations change
behaviour everywhere). Tests run under `cargo nextest` when it is installed
(same tests as `cargo test`, one process each; **doctests are skipped** —
run `cargo test --doc -p <crate>` or `just test` for them), else `cargo test`.
Inner-loop clippy drops `--all-features` unless a touched crate defines
feature flags; `--all` and workspace-wide changes keep it. Not CI-parity:
code behind features of an *untouched* dependent crate is not linted, and
doctests do not run — CI runs both. `--dry-run` prints the plan and exits.
The gate prints one line when `target/` exceeds 60 GB (`just clean-stale`);
it never deletes anything. Test databases are cloned from a migration
template (VEL-003) whose provenance stamp is a `COMMENT ON DATABASE`, so a
clone has exactly the schema of the `AURORA_TEST_TEMPLATE_DB=0` path (a
testkit test asserts it); `AURORA_TEST_POOL_SIZE` overrides the per-`TestDb`
pool (default 5 — 2 starves 20 `aurora-api` tests, see
`docs/ops/build-performance.md` §7).

**What the quick gate is NOT.** The quick gate is a development loop tool;
it is not a merge gate — CI runs the full suite on every push; never cite a
quick-gate pass as a merge-gate pass. It is additive: it runs a subset of
CI's checks and weakens, skips or masks none of them. Full-suite-is-CI's-job
is the discipline: local loops optimize for the 1–3 minute
catch-your-own-breakage feedback the quick gate gives; the workspace-wide
suite (and the e2e and Playwright live suites, which the quick gate never
runs) is what a push hands to CI.

## CI pipeline

1. `fmt`, `clippy -D warnings`, `cargo deny` (licenses, advisories), pnpm lint
   and type-check.
2. Unit + database + service + contract tests.
3. Build images (multi-arch on main; amd64 on PRs).
4. e2e-gateway, e2e-hosted (podman), dashboard Playwright on the host-process stack (phase 1, OPS-017: was compose; `m1.sh --kind` runs it against the kind dev stack).
5. Architecture guards.
6. Nightly: Kubernetes e2e on kind, live suites, performance. The kind run
   (`scripts/e2e/kind.sh`) ends with two value-only `helm upgrade` legs
   (OPS-024: `web.replicas` 1 → 2 → 1 against the installed release), which prove
   the pre-upgrade hook path — hooks run under the previous release's policies,
   so a hook that cannot reach what it needs fails there instead of timing out
   in production (see docs/operator/upgrades.md §Rehearse the upgrade).

**Implementation (OPS-019, `.github/workflows/ci.yml`).** `rust-build` compiles once and publishes a
`cargo nextest archive` (all workspace tests except the e2e crates, plus the fixture packages);
`rust-test` runs it in 4 shards (`--partition count:i/4`, profile `ci` in `.config/nextest.toml`);
doctests run in `rust-build` (nextest skips them). `rust-lint`, `rust-deny`, `web` and `guards` are
separate jobs. Actions are SHA-pinned and on Node 24; runners are pinned to `ubuntu-24.04`.
`e2e-gateway` (`scripts/e2e/gateway.sh`) and `e2e-hosted` (`AURORA_TEST_PODMAN=1 scripts/e2e/hosted.sh`, rootless podman, sandbox image cached with `actions/cache`) are separate jobs after `rust-build`, parallel to the shards; both compile in-job via their scripts. If `e2e-hosted` cannot run on GitHub runners (user-namespace limits), it becomes a local and nightly gate. See `docs/ops/build-performance.md`.

**Caching (VTL-001).** Every compiling CI job runs
[mozilla-actions/sccache-action](https://github.com/mozilla-actions/sccache-action) (SHA-pinned)
before rust-cache: dependency rlibs come from the shared GitHub Actions cache (`SCCACHE_GHA_ENABLED`
in the workflow `env`, `SCCACHE_CACHE_SIZE=8G`, namespaced with `ACTIONS_CACHE_KEY_PREFIX=sccache-`),
while the workspace crates keep coming from the per-key rust-cache (`rust-test` / `rust-lint`). This
is the plain `cargo` path only: OPS-023 measured and removed sccache from the *docker image builds*,
where its entries competed with the BuildKit layer cache for the repository's 10 GB Actions cache and
never hit; keep the two paths separate. The nightly Mattermost conformance job compiles on the runner
and uses the same backend. On a fork PR (read-only runtime token) sccache silently falls back to its
local directory — the build is unaffected.
