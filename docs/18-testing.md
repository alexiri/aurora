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
| **End-to-end: gateway** | real gateway + API + hook host + Postgres + fixture plugins + mock upstreams/OAuth providers, black-box over the proxy port; CI-only strict front doors (pinned nginx + haproxy containers between the gateway and the mock upstream, TEST-021 tier 2: `AURORA_E2E_FRONTDOOR`; CI runs `gateway.sh` with `FRONTDOOR=1` and `FRONTDOOR_HAPROXY=1`, and with `CI=true` a `SKIPPED:` line fails the script, FIX-101) | `tests/e2e-gateway` | every PR |
| **End-to-end: hosted** | real API, gateway, hook host, runner (podman backend), sandbox image with the `fake` harness, fake connector | `tests/e2e-hosted` | every PR on Linux CI with podman (the chromium scenarios, SEC-41, need the full sandbox image and run in the nightly `hosted-full` job; the PR job declares that one exception in `AURORA_E2E_ALLOW_SKIPPED`); channels (`scripts/e2e/channels.sh`, SEC-18/19/20) in the nightly `channels` job; nightly on Kubernetes (kind with a NetworkPolicy CNI) |
| **Dashboard** | component tests; Playwright flows against the host-process stack (`scripts/e2e/m1.sh`, default) or the kind dev stack (`m1.sh --kind`; phase 1, OPS-017: kind replaces the compose stack) | vitest, Playwright | every PR |
| **Live** | `claude-code` adapter against a configured LiteLLM/Anthropic endpoint through the gateway; Mattermost connector against a Mattermost container (phase 1) | env-gated | nightly |
| **Performance** | gateway overhead and throughput; due-work poll cost with 1 000 agents | criterion, k6 | **manual, on an idle machine** (FIX-101: no workflow runs it; the owner's rule is that performance budgets are measured only while no lane builds or tests, and a shared CI runner never is that machine) |

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
  lives in `aurora-api` because it needs the API's test harness; `aurora-supervisor` is a
  dev-dependency of `aurora-api` only (VEL-007).
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
- (phase 5, FIX-158, D-319, C-908) **Receipt lifetime and working indicator**
  (`api/it/channels_inbox`): the receipt `reaction.add` is done and no
  `reaction.remove` exists until the `answer:<turn_id>` op is enqueued; a turn
  ending with no text while follow-on work is pending leaves the receipt open
  until a later post to the same thread link clears it; with nothing pending it
  clears with reason `no_followon`; an add still `pending` at clear time is
  dropped with no remove; a remove is not claimed while its add is in flight;
  the sweep clears an open receipt after `RECEIPT_ORPHAN_SECS`; `status.set`
  on `running` and `status.clear` before the answer, none without
  `native_status` and none for web/API turns. Mattermost unit tests (mock
  server): the refresher calls `/users/{id}/typing` every `TYPING_REFRESH_SECS`
  with the sender's token, `channel_id`/`parent_id`; it stops on clear, at the
  ceiling and on 403; two sets are one refresher. Conformance s19 (fake: the
  working status shows past one refresh and ends with the answer; Mattermost
  driver: the ops are accepted). The typing endpoint was checked by hand on
  Mattermost 11.7.15 (see FIX-158 §Evidence).

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

### S3-compatible blob store (phase 6 (D-318, moved from phase 5), P4-S3, D-120, SEC-63)
- **Conformance** (`crates/aurora-blobstore/tests/it/conformance.rs`, one body over both backends): put/get/delete round trip with SHA-256 and size, overwrite refused (`If-None-Match`), get of a missing key, `list(prefix)` paging, 25 MiB object, concurrent puts. The `s3` run uses a **MinIO container pinned by digest** (gated `AURORA_TEST_S3=1`, started by the e2e script and CI like the Vault dev server; `MINIO_KMS_SECRET_KEY` set so SSE-S3 works) and, for failure cases, an in-process **mock S3** (throttling 503, 500, timeouts, a missing object, a server that ignores `x-amz-server-side-encryption`, a versioned bucket, Object Lock, wrong checksum).
- **Startup probe**: each refusal of `15 §S3-compatible blob store` (unconfirmed SSE, versioned bucket without the flag, Object Lock, missing bucket, refused credentials, plaintext remote endpoint, inline key, invalid prefix, unknown backend) fails with a message naming the key; `sse = "none"` starts with a warning; a loopback `http://` MinIO starts.
- **API on the `s3` backend** (`api/it/attachments_s3.rs`, the existing attachment tests re-run with the backend switched): upload, download, 410 after purge, quota (422 before the put, no overshoot under concurrent uploads), retention purge, delete one, conversation/workspace/org delete removes the objects, export zip, connector `file.fetch`, `send_file`; **store unavailable** (mock): upload 503 `blob_store_unavailable` with no row and an unchanged usage counter, download 503 (never 404/410), delete leaves an orphan the sweep removes, `readyz` unchanged, `aurora_blob_store_up` 0; checksum or `Content-Length` mismatch → 502 `blob_store_corrupt`; upload concurrency cap → 503 `blob_store_busy`; **no pre-signed URL**: no response, redirect, header, log line or audit row contains `X-Amz-Signature`, `X-Amz-Credential` or the endpoint host.
- **Isolation**: gateway, runner and sandbox never hold the credentials or reach the endpoint (NetworkPolicy render test; e2e-hosted probe resolves nothing); credentials absent from `GET /v1/admin/storage`, config dumps and logs (secret-grep over a run).
- **Orphan sweep**: unreferenced old objects deleted; unreferenced young objects (< `S3_ORPHAN_GRACE`) kept; referenced objects kept; keys outside the prefix untouched; 50 000-key cap; versioned mode deletes every version.
- **Migration** (`api/it/blobs_migrate.rs`): postgres → s3 and back; per-row verification failure leaves the row on its source and reports it; compare-and-set loses cleanly against a concurrent purge; resumable after a kill; `--dry-run` changes nothing; usage counters and quota unchanged; audit row without content.
- **Default unchanged**: with no `blobs.*` config every existing attachment test passes untouched; an existing `aurora.toml` and Helm values render and start.
- **e2e** (`scripts/e2e/` MinIO suite, gated): real stack, attachments through the dashboard API and a hosted turn (inbox delivery and `send_file`) on the `s3` backend; run when the machine is otherwise idle.

### Additional LLM providers (phase 5 (D-138, moved from phase 4), P4-LLM, D-121, SEC-64, SEC-65)
- **Mock upstreams** (`aurora-testkit`, one per provider; no live network in CI): OpenAI/compatible (list, chat, responses, SSE), Azure (v1 routes, `api-key`), Bedrock runtime (SigV4-**verifying**: recomputes the signature from a known secret; event-stream responses) plus control plane and STS, Vertex (prediction routes, SSE) plus a token endpoint (verifies the RS256 JWT: `iss`, `scope`, `aud`, `exp`, signature against the test key). Each mock records what it received so tests assert the credential that arrived and what did not.
- **Per provider** (`crates/aurora-llm/tests/it/<provider>.rs`, gateway tests `gw/it/llm_<provider>.rs`): registry entry, config validation and every startup refusal of `15` (unknown key, credential-named key, non-https, bad region/location/project, empty `models` for Azure/Vertex, duplicate id), host reservation and exact-host Default Rule carve-out, model listing and its fallback, key validation (`llm_key_invalid`), key-origin verification and the response backstop with foreign accounts (OpenAI headers, STS account, service-account project), injection (agent-supplied `Authorization`, `api-key`, `x-api-key`, `X-Amz-*`, `x-goog-*`, `OpenAI-*` removed and replaced; SigV4 against AWS test vectors and the verifying mock; token mint, cache TTL, single-flight under concurrency, re-mint at skew, mint failure → `credential_unavailable` with nothing forwarded; `token_uri` pinning), streaming passthrough byte-for-byte with first-byte latency not delayed (SSE and event stream), `model_from_request` for every location form (body, percent-decoded Bedrock id/ARN, Vertex path, truncated body → refused under an allow-list), body above `LLM_SIGN_BODY_MAX`.
- **Bedrock** (P4-LLM-4, D-312, D-313): gateway `governance::llm_bedrock_*` (in `governance.rs`'s harness with a scratch database for the org's external id: each mode, external id per organization, denied role, 413 before the secret is opened, runtime-only routes, event stream byte-identical and streamed with the first frame ahead of a gated rest, exact-host carve-out under `credentialed_only`); API `secrets::bedrock_*` (validation, origin, listing merge and 403 fallback; mocks through `llm.dev_upstream_overrides` incl. `sts.`/`bedrock.`); e2e `s24_bedrock_assume_role_…`. The mocks verify SigV4 with their own implementation, not `aurora-llm::sigv4`.
- **Harness reach** (P4-LLM-5, SEC-64): supervisor unit tests in `adapters/claude_code/config.rs` (env per dialect, credential variables dropped, unknown dialect refused), `adapters/provider_error.rs` and `codex/{config,stream}.rs` (classification, `mint_rejected` → `provider_auth`, C-809 base URLs); hosted e2e `s25` (codex on Azure, `api_key` and `azure_entra`) and `s26` (claude-code on Bedrock, the three auth modes, sandbox environment byte-identical, the sandbox probe finds no seeded key, session token or minted token) in `tests/e2e-hosted/tests/hosted.rs`, run by `scripts/e2e/hosted.sh`.
- **Path corpus** (SEC-65): allowed routes pass; refused: Bedrock guardrail/async-invoke/control-plane host, Vertex other project or location and non-prediction routes, OpenAI files/fine-tuning/admin, Azure management paths, compatible-endpoint admin routes (with the derived set and with `allowed_paths = ["/**"]`); the check applies only where Aurora injects (an agent's own key under `open` is unchanged); `litellm` and `anthropic` unchanged.
- **Governance matrix** re-run over the new endpoints: `sanctioned_only` with and without a key, endpoint not permitted, scope not permitted, origin mismatch/unverifiable, model allow-list; plugin claim of an endpoint host refused; an S3 request under Default Rule `block` still blocked while the Bedrock endpoint works.
- **Classification** table tests (`classify` for every documented provider error, shared by gateway and adapters), request-log `error_code` and `aurora_gateway_llm_upstream_total`; the gateway never changes an upstream status or body.
- **Usage**: a turn on each provider records `provider`, `endpoint_id`, `model` from the `ModelChoice` (a harness claiming another provider is ignored); `GET …/usage` aggregates by provider.
- **Workspace usage** (phase 5, P5-USAGE-1, D-359, C-1000..C-1002): the ledger is written once per finished turn (a replayed terminal report adds nothing), per model, with turn count and time under the default model; a turn that never started adds nothing; an aborted turn with only a streaming claim is counted after the trust rule; deleting the conversation leaves the totals unchanged; the backfill matches the live rule; the rollup splits an interval across UTC midnight, counts an open interval up to the run, catches up after missed runs and is idempotent; the route's range rules, zero days, deleted-agent names, workspace isolation and access match `…/activity`; the instance route (C-1003) is admin-only and the organization route (C-1006) is for org owners and admins and never reads another organization's rows; their top-50 cut plus `other_workspaces` adds up to the totals. Views (P5-USAGE-2, D-360): `StatTile`/`BarChart` unit tests (accessible names, keyboard, tooltip), `UsageView` states and URL state, the four placements, and a Playwright pass in both themes at 1280 and 390 px.

### Model roles and usage per model (phase 5, P5-MODELS, D-302..D-304, SEC-122, SEC-123)

- **Selection** (`api/it/model_roles.rs`, P5-MODELS-1):
  - resolution order (agent stamped role, then endpoint role, then unset);
  - an agent role on another endpoint, or no longer allowed, is dropped with one notice and never re-pointed, while the other roles stay;
  - an endpoint role outside an org's allow-list gives `role_not_allowed` and is not sent;
  - `role_unset` appears on non-`anthropic` endpoints only;
  - `subagent_role` is omitted with `subagent_role_unset` when its role resolves to nothing on a non-`anthropic` endpoint;
  - readiness (C-878) equals the dispatcher's choice;
  - the eval fingerprint includes `model_roles` and `subagent_model`;
  - PATCH errors `model_id_invalid`, `llm_model_not_allowed` and `model_role_endpoint_mismatch`;
  - config and settings refusals use `value_invalid`.
- **Adapter** (golden tests, P5-MODELS-1):
  - the `claude-code` environment for each role combination (alias variables, `CLAUDE_CODE_SUBAGENT_MODEL`, capabilities, context window; no `_FORCE`);
  - `codex` ignores roles;
  - the `harness.env` section with roles (SEC-122);
  - the supervisor refuses a pattern-violating id with `harness_failed`.
- **Live conformance** (env-gated, LiteLLM endpoint serving two models, P5-MODELS-1):
  - a subagent with `model: haiku` and the CLI's background requests reach `roles.small` (checked with `request_log.llm_model`);
  - the main loop uses the default;
  - with `subagent_model = small` and no definition, the general-purpose subagent uses `small`.
- **Usage** (P5-MODELS-2):
  - parsing the `claude-code` result's per-model report;
  - the totals equal the sum of `by_model`, and a fixture with a subagent proves the main-loop-only `usage` is no longer used;
  - the fallback to `usage` when no per-model report is present;
  - the trust rule (an allowed, role or default model is kept; a foreign model is folded with `unverified_models`; more than `USAGE_BY_MODEL_MAX` entries are folded);
  - with no allow-list, a pattern-valid id is kept;
  - `costUSD` is dropped;
  - old turns without `by_model` aggregate under `model`.
- **Gateway** (P5-MODELS-2):
  - `request_log.llm_model` is set for injected LLM requests with and without an allow-list (body, Bedrock path and Vertex path forms);
  - null for `Unknown`, with no refusal when no allow-list applies;
  - no first-byte delay beyond the existing prefix read (the streaming passthrough test re-runs).
- **Sandbox isolation** (e2e-hosted with mock endpoints): a Bedrock and a Vertex turn through claude-code with the "skip auth" environment; seeded AWS secret and service-account private key found nowhere in the sandbox, transcripts, logs, request log or metrics; `client_env` contains no key-looking value.
- **Live suites** (`AURORA_TEST_LIVE_OPENAI=1`, `…_AZURE`, `…_BEDROCK`, `…_VERTEX`, `…_COMPAT`; not in CI): one real model call and listing per provider through a real gateway, used to pin the header, error-type and client-variable details the spec leaves to "pinned in the provider".
- **Default unchanged**: with the default `enabled_providers` the registry, `/v1/llm/providers` and every phase-0 provider test are untouched.

- (phase 5, P4-LLM spec batch, D-260..D-262, SEC-97, SEC-98, C-814) **Mint service** (`gw/it/llm_mint.rs`): the three minted modes against verifying mocks in `aurora-testkit` (`llm::sts` checks the SigV4 of `AssumeRole` with the source key and records `RoleArn`, `ExternalId`, `RoleSessionName`, `DurationSeconds`; `llm::entra` checks the client-credentials form and issues a JWT with `tid`/`appid`; `llm::google_token` as above); single-flight, skew, failure hold, eviction, invalidation, `mint_rejected` vs `mint_unavailable`; Bedrock `aws_assume_role` inference signed with the session key and `X-Amz-Security-Token` (verifying runtime mock), never the source key; `ExternalId` differs between two seeded orgs. **Anthropic path set** and **`allowed_paths` semantics** (adds for the five key providers, refused for `bedrock`/`vertex` at start and on a settings PUT). **No translation**: a byte-for-byte body and stream comparison for one request per dialect; an `openai_chat`-only endpoint is `no_shared_dialect` for both shipped harnesses in model readiness. **Endpoint destinations** (SEC-98, after SET-3): Helm vs dashboard endpoint on a private address, metadata and link-local never implicit, `http_needs_private_host`, `endpoint_host_claimed`, `endpoint_resolves_private`, reload revokes the pass, endpoint test per provider (C-803) obeys the guard. **Test hosts**: every gateway and e2e test reaches fixed hosts only through `llm.dev_upstream_overrides`; a config with the override and without `insecure_loopback_injection`, or with a non-loopback target, refuses to start. **Harness reach** (e2e-hosted, mock upstreams): a Codex turn on `openai`, on `azure_openai` (`api_key` and `azure_entra`) and on an `openai_compatible` endpoint declaring `openai_responses`; a claude-code turn on `bedrock` with each of the three auth modes and on `vertex`; the sandbox environment is identical across a provider's auth modes.

### Harness adapters (phase 4, P4-HAR, D-122, SEC-66)
- **Conformance**: `tests/conformance/harness` gains `codex`: CI runs it against a mock **Responses/chat upstream** in `aurora-testkit` (SSE, tool call, error injection: 401, 429, `insufficient_quota`, 5xx) through a real gateway; an env-gated variant (`AURORA_TEST_HARNESS_CODEX_LIVE=1`) runs it against a real OpenAI endpoint. Same cases as every adapter plus: steer refusal (`steer = false`, follow-up becomes a separate turn).
- **Adapter unit tests**: event mapping from recorded Codex JSONL fixtures (golden), exactly one terminal event after abort and after crash, effort table, session-id parsing and resume, `purge_session` (idempotent, only the named session), error mapping to `provider_*` through the shared table.
- **Rendering**: golden test of `config.toml` and the process environment for each dialect and for Azure/compatible endpoints: no secret, `OPENAI_API_KEY=aurora-placeholder`, `base_url` from `ModelChoice`, approvals/sandbox settings, MCP bridge entry; no `auth.json` created; `AGENTS.md` rendered, `CLAUDE.md` not (and the reverse for `claude-code`).
- **Selection**: `hosted.enabled_harnesses` validation (unknown id, default not enabled, `fake` gating); `PATCH` harness (422 not enabled, 409 active turn); boot document carries the id; unknown id or missing runtime → `harness_unsupported`; runner without the harness → `harness_unavailable` and no sandbox start; images without the label read as `claude-code,fake`; dialect intersection and first-of-harness-list choice; model override dropped with a notice after a switch; `no_model_key` when nothing is compatible.
- **Switch**: epoch bumped for every conversation, `harness_session_ref` cleared, purge rows queued and executed by the old adapter at next boot before any turn, instructions re-rendered for the new harness, transcript untouched.
- **e2e-hosted**: a Codex agent against the mock upstream (turn, tool call, `send_file`, platform tools, restart and resume) and the SEC-01 secret probe over its sandbox; run after P4-LLM lands.
- **`pi`** (phase 5, P5-HARN, D-361, D-362, SEC-133): `aurora-testkit`'s `HarnessUpstream` serves the mock **Messages, Responses and Chat** upstreams (SSE; scenarios `say`, `tool`, `steer`, `mcp`, `long`, `fail-401/429/quota/500`; fixed usage so counters add up). `tests/conformance.rs` runs the common suite with the real pinned Pi against **each** declared dialect (`AURORA_PI_BIN`, skipped when not installed) and then proves: every request carried only `aurora-placeholder` on that dialect's inference path, a refusing session proxy saw no request (no vendor network); the four usage counters per model add up to the totals; an MCP tool call (`mcp__aurora__ping`) comes from a **descendant of the Pi process** (D-301, a probe MCP server records its ancestry). Steer needs Pi's acknowledgement of the prompt first, so the conformance steer is retried until accepted; the "missed steer" case is not forced on a real harness (a steer after the run is refused, covered by the between-turns case and by the adapter test of a steer that raced the end of the turn). Adapter tests (scripted RPC child): one prompt per turn, steer refused between turns and never sent, queued steer joins on Pi's transcript, a raced steer is aborted and waited for, abort settles or the child is killed and restarted, exit → `harness_failed` and a restart that resumes the same session id, the configuration re-rendered before every turn with a planted `auth.json` and a planted `models.json` gone, `purge_session` removes one session directory only, unspeakable dialects refused. Golden tests of `models.json`, `mcp.json`, `settings.json`, the argv and the environment for each dialect: a `$VAR` reference, no secret, every vendor switch off. Env-gated live run: `AURORA_TEST_HARNESS_PI_LIVE=1` (+ `PI_LIVE_BASE_URL`, `PI_LIVE_DIALECT`) through a real gateway and LiteLLM.
- **`goose`** (phase 5, P5-HARN, D-361, D-362, SEC-133): the same mock upstreams (`AURORA_GOOSE_BIN`, skipped when not installed) against **each** declared dialect (`anthropic_messages` through goose's `anthropic` provider, `openai_chat` through `litellm`; `openai_responses` is not declared). The common suite runs, then: every inference request carried only `aurora-placeholder` on the dialect's path (the one other request, `GET /model/info` of the `litellm` provider, is allowed explicitly; the gateway forwards it by default for `litellm` endpoints, D-367, with its own tests: `litellm_endpoint_forwards_get_model_info_by_default_and_nothing_else_of_its_metadata`, the `aurora-llm` `litellm::tests`, and `GET /model/info` refused on `anthropic` and `openai` endpoints), and the refusing session proxy saw **no request at all**, which proves the rendered model catalogue stops the models.dev fetch; the four usage counters with the input net of the cache; a session resumed in a new process with **another model** sends the new model with the earlier turns in context; an MCP call (`mcp__aurora__ping`) comes from a descendant of the goose process; a delegated subagent's request reaches the model of the role the choice names. Adapter tests (scripted child): the prompt on stdin never in argv, `--resume` only once the session exists and for a resume reference, a start without an event is `harness_failed` with goose's stderr, error text classified (401, 429, quota, 500), the root re-rendered before every turn with a planted `secrets.yaml`, config, catalogue and a planted `data` link all replaced, abort kills the child and leaves the session usable, `purge_session` removes one root only. Golden tests of the environment (both providers, subagent model, caller values dropped), `config.yaml`, the catalogue (with the newline in the etag) and the argv. Env-gated live run: `AURORA_TEST_HARNESS_GOOSE_LIVE=1` (+ `GOOSE_LIVE_BASE_URL`, `GOOSE_LIVE_DIALECT`).
- **`opencode`** (phase 5, P5-HARN, D-361..D-363, SEC-133): the same mock upstreams (`AURORA_OPENCODE_BIN`, skipped when not installed; `HarnessUpstream::start_with_all_tools("aurora_ping", "bash", "task")`) against **each** of its three dialects. The common suite runs (the refusal case uses the mock's `fail-402`: OpenCode retries a 429 for about 70 s, `12`), then: every request carried only `aurora-placeholder` on the dialect's inference path, and the refusing session proxy saw **no request at all** (no models.opencode.ai, no npm registry, no update or share service; fail-before check: without the rendered `package.json`/lockfile/`node_modules` files six `CONNECT registry.npmjs.org:443` reach it); a session resumed in a **new process** continues through the `ses_…` id read with `opencode db`, with **another model** and the earlier turns in context, and a purge by the reported ref removes the directory; per-model usage from the database on all three dialects (four counters, totals = sum, exactly two requests so no title call), and a delegated subagent's tokens as a second entry under the role's model with its request reaching that model; an MCP call (`mcp__aurora__ping`) comes from a descendant of the opencode process; a **400 KB prompt** reaches the model request whole through stdin (Messages and Chat) and the effort reaches it (`thinking.budget_tokens` 16000, `reasoning_effort` `high`). Adapter tests (scripted child): the prompt on stdin never in argv (also over 128 KiB), the session continued only once it exists (or found with the database query, or started fresh when the data is gone), a start without an event or a non-zero exit is `harness_failed` with stderr, error events classified (401, 402, 429 with the retry note, quota, unknown), a lingering process after the complete answer is stopped, the root rebuilt before every turn with a planted credential store, a second configuration, an agent definition and a planted `data` link all gone, abort kills the child, usage fallback to one default-model entry when the query fails, `purge_session` removes one root only. Golden tests of the environment (every vendor switch on, caller values dropped, placeholder only), `opencode.json` (no secret, no update, no share, no title, the bridge, each dialect's package and base URL, the effort variants) and the argv. Env-gated live run: `AURORA_TEST_HARNESS_OPENCODE_LIVE=1` (+ `OPENCODE_LIVE_BASE_URL`, `OPENCODE_LIVE_DIALECT`).

#### Model readiness and credential-side grants (phase 4, FIX-019..FIX-021, D-136, D-137)

- **Readiness (API)**: one test per `reason` of `08 §Model readiness` and for `usable`; `default` for each step of the chain (override, endpoint default, provider default, allow-list, static list) and `null` for a LiteLLM endpoint with only `base_url`; the dispatcher fails a queued turn `no_default_model` in that last case and `no_model_key` with reason counts (and no key names) when every granted key is unusable — both before any sandbox start; with the agent model set the same turn is dispatched; `model-readiness` is `can_own` (permission matrix).
- **Config**: a `litellm` endpoint without `default_model` starts and logs the warning; `key_origin.method = "litellm-team"` refuses startup with the suggestion.
- **`litellm_team`**: against the kind mock's real LiteLLM, a **team key** verifies with its `team_id`; a **teamless key** verifies (`team_id = null`) without required team ids and is refused `key_reports_no_team` with them; `/key/info` 401 and an unreachable endpoint record `origin.detail`.
- **e2e (kind, llm-mock)**: the owner's production endpoint shape (`provider: litellm`, `base_url`, `key_origin.method: endpoint`, no `default_model`, no `models`, no `dialects`): store a key, grant it, chat → (phase 5, FIX-170, D-372) the sandbox starts with the first chat model the key serves (`source = key_listing`) and the reply arrives, with no model chosen anywhere; the conversation stream stays `Live` throughout. (Before FIX-170 this step expected `no_default_model`, then a model chosen on the agent.)
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
- **Recorded IdP traffic** (`api/it/scim_idp_replay.rs`, fixtures `services/aurora-api/tests/fixtures/scim/{entra,okta,keycloak}/*.json`, request/response pairs reconstructed from each vendor's documented provisioning behaviour with synthetic tenants and users, no real data): initial sync (probe `GET filter`, `POST`, `409` → `PATCH`), attribute updates, deactivate/reactivate, group create and member churn, a token rotation mid-sync, a repeated `POST` after a lost response. Replay asserts final state and the exact statuses the IdPs rely on.
- **Auth and scope** (SEC-72, `scim_auth.rs`, `scim_scope.rs`): `/scim/v2` is 404 with `scim.enabled = false` and 401 (identical body) for no token, malformed, unknown, revoked, expired, other-prefix (`auk_`, `aug_`) and org-disabled tokens; an `aud_` token on `/v1/**` is 401 and a session cookie on `/scim/v2` is ignored; token lifecycle (owner only, session only, no re-authentication since FIX-157, D-317, hash only in the database, shown once, expiry cap, five-token limit, expiry notice and audit rows 14 d and 1 d before); org A's token cannot read, filter or write org B's users and groups (404, never 403); manual groups invisible; no way to set a role, create an owner or admin, or mutate an owner (PUT/PATCH/DELETE → 403, last-owner untouched); e-mail domain allow-list and instance allow-list; seat limit; per-token rate limit and 429 with `retry-after`, bad-token IP limit; body, operation, member and page limits; audit coverage scan for the new actions and forced audit failure (nothing applied, 503); token never appears in any response, log line or audit row.
- **Linking and deprovisioning** (SEC-73, `scim_link.rs`, `scim_deprovision.rs`): a SCIM user is pre-provisioned unverified and cannot act before sign-in; the OIDC sign-in with the verified e-mail (and with a trusted-domain e-mail, D-118) binds it, an unverified claim or a matching `externalId` alone does not; a signed-in user's `users.email`/`display_name` are unchanged by SCIM updates; an existing user is linked without touching their row; the full effects matrix of `04 §SCIM` for suspend and remove (membership, sessions rule with one and with two orgs, key revocation including after reactivation, running speaker turns `user_deprovisioned`, group-derived access, automations disabled at fire time and listed in the offboarding view, pending invitations cancelled); JIT and invitation acceptance refused for a suspended or removed link, allowed again after the IdP re-provisions; reactivation only when `suspended_by_scim`; `exclusive` mode; owner untouched; the person's other org is unaffected.
- **Groups through SCIM** (`scim_groups.rs`): SCIM groups appear in `/v1` as `source = scim` with read-only name and members (`group_managed_by_scim`), members must be this org's links, removed links rejected, `PUT` replaces atomically with one audit row per change, deleting in the IdP deletes bindings, the effect on access and gateway identities is that of `## Groups`.
- **Dashboard** (`web/tests`): Directory sync page (hidden when the instance switch is off, owner only, token shown once, expiry banner, status card), Members "Directory" badges and the offboarding BYO list; both themes, 390 px.
- **Edge**: the Helm Ingress/HTTPRoute renderings include `/scim/v2` only with `scim.enabled` (edge-routes guard).

### Approval park-and-resume and Web Push (phase 5 (D-138, moved from phase 4), RFE-011b, D-133, D-134, SEC-74, SEC-75, SEC-76)
- **Parking decision** (`gw/it/approval_park.rs`, real Postgres, mock upstream): with parking off the hold is byte-for-byte the phase 2 behaviour (existing suite unchanged); each eligibility condition has a positive and a negative case — timeout at, above and below `threshold_secs`; BYO, boot-token and conversation-less requests held and clamped; body at and over `max_body_bytes`, chunked/streamed and upgrade requests held; a verdict that differs without a speaker (a `user` or `group` rule) held; per-agent pending and hourly caps → 403 `approval_limit_reached`. A parked request is answered 403 `approval_parked` with `x-should-retry: false` and `details`, **nothing is forwarded upstream** (mock upstream sees zero requests), no row for the body, and an identical repeat returns the same `approval_id` and creates nothing.
- **Grants** (`gw/it/approval_grants.rs`): approval → `resume_state = ready`; the identical retry in the resume window is consumed and forwarded exactly once (32 concurrent retries → exactly one forwarded, the rest get a new approval/park); fingerprint sensitivity — each of agent, conversation, method, host, port, path, query (value and order), one byte of body, rule, credential, tool changes the fingerprint and the retry is not matched; the window end (`resume_state = expired`); a policy change between approval and retry to `block` (blocked), to `allow` (allowed, grant unused), to another requiring rule or credential (not matched); a retry from another conversation of the same agent not matched; consumption before refresh and forwarding (a forced upstream or refresh failure still leaves the grant consumed); deny cooldown → `manual_approval_denied`; a deadline expiry of a parked approval (`05` job) with no gateway involved; the stale-holder sweep and SIGTERM never abandon a parked approval; a freeze abandons pending and revokes ready; a database outage parks and consumes nothing (SEC-54 still holds).
- **API** (`api/it/approval_park.rs`): decide approve/deny/expire for parked approvals under the same decider matrix (RFE-012, RFE-009, groups); the system follow-up (steered into a running origin turn, else a queued system turn; none for a frozen agent or an expiry; text platform-composed, canary strings in the request path/title do not appear beyond the sanitized summary); `approval.pending/decided/expired` transcript events and DTO fields (`parked`, `resume_state`, never fingerprint or salt); the stall exemption does **not** apply to parked approvals while it still applies to held ones; cards for parked approvals settle in all four outcomes; critical audit rows for decisions; the reminder is sent once and only without a live watcher.
- **Sandbox lifecycle** (`api` + `fake` runner): a parked approval is not a keep-awake reason (the sandbox parks after `IDLE_STOP`), and the resume follow-up starts a new boot with the harness session restored (`harness_session_ref`); no process or connection of the old boot is needed.
- **Web Push server side** (`api/it/web_push.rs`, a mock push service in the test kit): RFC 8291 Appendix A and RFC 8292 vectors; the mock verifies the VAPID JWT (`aud`, `exp` ≤ 1 h, `sub`, key id), decrypts with a test subscription key pair and asserts the exact payload fields; **canary test**: approvals whose title, summary fields, rule name, host, path, query and body contain unique strings produce pushes none of which contain them, with and without `show_details`, and `show_details` adds only the sanitized agent and workspace names; routes (limits, moves, validation errors, ownership 404, session-only, `push_unavailable` when off); delivery rules — decider at send time (lost `can_approve`, direct-conversation visibility, group-derived access), live watcher suppresses, session revoked/expired/sign-out stops delivery, endpoint re-registered by another user moves, approval decided/expired/frozen before the delay sends nothing, claims are idempotent across two replicas, batches fold into one push, hourly cap and digest, the e-mail path is unaffected; failures — 404/410 deletes, 429/5xx retry bound, five failures disable; key handling — first-start generation under concurrent replicas yields one key, the private key and subscription secrets are sealed and absent from every response, log and audit row, rotation (old-key subscribers still delivered, 409 `push_key_stale`, silent migration, retired key destroyed), operator-managed key mode.
- **SSRF** (`api/it/web_push_ssrf.rs`): registration refuses http, userinfo, IP literals, private/loopback/link-local/metadata addresses, non-allow-listed and look-alike hosts, over-long URLs; a send re-validates (an allow-list change takes effect), a host that resolves to a private address (DNS rebinding after registration), a redirect and a slow or oversized response are all refused or bounded; startup refuses an empty or invalid `push.allowed_endpoint_hosts`; with `push.enabled = false` no outbound call is ever made (network-denied test).
- **Dashboard and service worker** (`web/tests`): the Notifications card states (unsupported, insecure origin, permission denied, iOS Home Screen note, registered, paused, not receiving, key rotated → silent re-subscribe), the gesture requirement, sign-out removes the device; the service worker (unit tests in a service-worker test environment) shows the generic notification for a malformed or unknown-version payload, builds text only from the fixed fields, `notificationclick` navigates only to a same-origin path starting with `/o/` and falls back to `/` for `//evil`, `https://…`, `javascript:`, backslashes, and has no `fetch` handler; parked chips and sentences on the Approvals page, chat and overview; both themes, 390 px.
- **e2e** (extension of `scripts/e2e/`, owned by its lane; the browser-push leg uses the mock push service and headless Chrome's push emulation): a rule with a 2-hour timeout and parking on → the agent is refused `approval_parked`, ends its turn, the sandbox parks, the approver approves from a card, the system follow-up wakes the agent and the identical request succeeds exactly once.
- **Delivered by P4-011B-3** (proves SEC-75/SEC-76): `api/it/web_push.rs` (routes and limits, move/rebind, canary payload, decider/watcher/session at send time, replicas/batch/cap/digest/reminder, revocation deletes, outcomes and the five-failure disable, `rejected_host`, the test route, key generation/inline sealing/rotation/destruction, the operator-key startup refusal), `api/it/web_push_ssrf.rs` (registration refusals, redirect not followed, slow service cut off, oversized answer not read, a non-public name fails closed without the test seam), the `deliver` and `crypto` unit tests (RFC 8291/8292 vectors, payload fields ≤ 1 KiB, `Topic`, status mapping, the resolver's private-address judgement) and `aurora-config` `push_validation_follows_the_switch_and_the_override_rules` (startup refusals). Not separately proven there: direct-conversation visibility and group-derived access (the decider set is the e-mail notifier's own SQL, `approval_notify.rs` proves those), and a successful operator-key start (the key forms are a unit test; an integration test would need to set a process environment variable).
- **P4-011B spec batch additions** (D-264..D-267, C-820..C-839; briefs P4-011B-1..5): **gateway** — the C-821 fingerprint vectors; a long-timeout request to a governed LLM endpoint is held and clamped, never parked; `approvals.park.enabled` turned off (settings notify) with a `ready` grant → the identical retry is still consumed exactly once, and a new long-timeout request is held; request-log rows carry `approval_parked`, `approved` + `resumed`, `approval_id` on holds, `rate_limited` + `approval_limit_reached`. **API** — each FIX-084 access-removal event, agent deletion, conversation deletion and an epoch bump (harness switch, `reset_session`) abandon pending parked rows and revoke ready grants in their own transaction (notice, card edit, no turn; a later identical retry is not matched); a batch decision over three parked approvals of one conversation writes one follow-up listing three lines, door `{ kind: "system", ref: "approval:…" }`, in the deciding transaction (a forced failure after the decision rolls both back); the reminder e-mail claims `stage = 'reminder'` once across two replicas; settings writes refuse `park_threshold_above_hold_max` / `park_max_below_hold_max`; `GET /v1/policy/approval-limits` follows a settings change live; rule authoring bounds follow the switch. **Web Push** — sign-out, sign-out-others, admin revocation and deactivation delete the session's rows; `rebind: true` re-binds the caller's endpoint and is 404 for another person's (nothing moves); `Topic` differs per device for one approval and contains no approval id; sealed rows use the `inline` store with Vault as the default store (Vault mock sees no call); startup refuses `push.dev_endpoint_overrides` with a non-loopback target or external URL. **Service worker** (SEC-102) — the source test and Playwright checks of `17`; the browser leg delivers pushes to the registered worker with Chromium's DevTools `ServiceWorker.deliverPushMessage` (Playwright `newCDPSession`) and asserts the shown notification (title, body, tag) and the click navigation; delivery through a real push service is a manual release check.

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

### SSH access (phase 5, P4-SSH-1, P4-SSH-2, D-239, SEC-92..SEC-96)
- **API, keys and certificates** (`api it/ssh_keys.rs`, `it/ssh_cert.rs`; P4-SSH-1): key parsing
  (canonical form rebuilt, comment dropped, options/certificates/RSA/ECDSA/private-key pastes refused,
  `SHA256:` fingerprint equal to `ssh-keygen -lf`), per-user uniqueness and limit, session-only writes,
  critical audit rows; the mint auth matrix of SEC-93; certificate fields parsed back with the `ssh-key`
  crate and verified against the CA public key; `ssh_mint_counters` exact under concurrency from two
  pools; `GET /v1/ssh` shape; every route 404 while `ssh.enabled` is off.
- **API, sessions** (`api it/ssh_sessions.rs`; P4-SSH-2): the terminator plane auth (`aut_` only, 401
  uniform), session open matrix and caps, channel grants (single-use, TTL, stale boot, closed session),
  heartbeat revocation causes, close exactly once against a concurrent sweep, wake demand on a parked
  sandbox within the shared budget, keep-awake `kind: "ssh"` stamping and the cap, list/disconnect
  authority (own, `can_own`, other → 404).
- **Terminator** (`aurora-terminator` tests, a real `russh` client in-process and a stub API): SEC-92
  auth matrix with byte-identical refusals; pre-auth limits; channel/request allow-list of `16 §W20`
  (each refused kind opens nothing downstream); `exit-status`/`exit-signal`, EOF and close ordering;
  heartbeat failure → close-all; idle and lifetime with shortened limits; plus the machine's real OpenSSH
  client (`tests/it/openssh.rs`: a certificate logs in and gets the exit status, a plain key / a foreign-CA
  certificate / an expired one get the same `Permission denied (publickey)` and cost the API no call).
- **Runner** (`run it/ssh_legs.rs`): `auz_` redemption (`kind: "ssh"`; `aux_`/`auy_` never open an exec
  and vice versa), first-frame `exec.open` deadline, frame validation (unknown tag/kind → 4502),
  `SSH_CHANNELS_PER_SANDBOX` cap, W6 reconnect closes every exec 4503, older supervisor → 4502,
  keep-awake stamps.
- **Supervisor** (`sup` exec tests): the three modes, stdin EOF, stdout/stderr separation without a
  tty, exit status and signal, env allow-list and the SEC-34 probe, `sftp-server` path fixed and
  `unsupported` without it, fd-leak probe (FIX-059 posture), credit bounds under a hostile writer.
- **Dashboard** (`web/tests`): Account › SSH keys (add, inline errors, delete confirm, sessions list and
  disconnect), the agent's SSH page gating and instructions, certificate download, 390 px layout.
- **kind e2e** (`scripts/e2e/ssh.sh`, own lock like the other e2e suites): enable SSH, register a key,
  `aurora ssh <agent> -- echo aurora-ssh` and plain `ssh` with a downloaded certificate (output asserted),
  `ssh host 'exit 7'` → 7, `sftp` put/get round trip and `scp` (both protocols) of a 5 MiB file
  with checksums, `-L`/`-R`/`-A`/`-X` refused while the shell works, a wrong pinned host key refused by
  the client, a parked sandbox wakes on connect, freeze and a disconnect close the session within 35 s,
  a deleted key ends its certificate's trust, and a canary typed in a shell and passed as a command is
  absent from every log, `audit_log` and `request_log` (scenario `k9_ssh_journey`; the cluster's
  terminator is reached through a port-forward; the script prints `SKIPPED:` without OpenSSH's client tools).

### Files (phase 5, P5-FILES-1..4, D-316, SEC-125, SEC-126)
- **API** (`api it/files.rs`; P5-FILES-1, against a real runner and supervisor over the kind-less
  podman substrate or the in-process test harness the terminal ITs use): the SEC-125 auth matrix; the
  C-899 path table (each refusal before any SFTP request — a counting fake leg proves it); listing
  order, kinds, symlink targets (inside, outside, dangling), `truncated` at a shortened
  `FILES_LIST_MAX`; download headers of C-903 and byte-exact content (sha256) for an empty, a 1-byte, a
  16 KiB + 1 and a 20 MiB file; text endpoint reasons (`too_large`, `binary`, `not_utf8`) and CRLF
  detection; PUT create/replace, `If-None-Match: *` → 409, `If-Match` stale → 409 with
  `current_etag`, `0444` target → 403, missing parent → 404, over `FILES_UPLOAD_MAX` (shortened) → 413
  before any write, aborted body → no target change and no temp file left; atomic replace (a
  concurrent reader sees the old or the new bytes, never a mix); mkdir/rename/delete including
  `parents`, existing targets, rename into itself, non-empty without `recursive`, recursive over a
  shortened `FILES_DELETE_MAX` (nothing deleted), symlink-in-tree not followed; `.aurora` read-only;
  wake demand on a parked sandbox (503 + `terminal_wake_at` set, shared budget → 429); keep-awake
  stamping throttle and the `files` reason in `keep_awake_reasons`; pool idle close and reopen after a
  W6 reconnect (503 `files_unavailable` once, then success); audit rows carry paths and sizes, never
  content.
- **Runner** (`aurora-runner` ITs): `auf_` redemption only as `kind: "files"`; first frame other than a
  bare `sftp` `exec.open` → 4502; the cap → 4409; an older supervisor (ignores `exec.open`) → 4502;
  no keep-awake stamp from `auf_` legs.
- **Supervisor** (`aurora-supervisor` ITs): the `sftp` exec over pipes (stdout `0x02`, stderr `0x03`,
  `exec.eof`, `exec.exit` status), env probe, uid/cwd, fd-leak probe (FIX-059 style), process-group kill
  on close, `exec.refused` for a missing `sftp-server` and over `EXECS_PER_SANDBOX`, credit bounds
  under a hostile writer.
- **Web** (vitest, P5-FILES-2/3): breadcrumbs and `&path=`, hidden switch, clash dialog choices, upload
  queue concurrency (3) and cancel, error-code sentences, editor dirty state, Save with `If-Match`,
  conflict dialog's three actions, CRLF kept, image magic-byte check, SVG/HTML no preview.
  Playwright + axe in both themes at 1280 and 390 px (no horizontal scroll).
- **Journey** (P5-FILES-4): `scripts/e2e/files.sh` on kind (uses the e2e lock; `require_or_skip` when
  `terminal.enabled` is off in the cluster) — upload a folder of three files, list it, download one
  and compare sha256, edit and save, provoke a conflict by changing the file from the terminal leg,
  rename, recursive delete, `.aurora` write refused, a symlink to `/etc` refused, freeze → 423, park →
  next request wakes; plus the Playwright journey of C-907; a secret-grep over the run for a canary
  uploaded in a file (no log, audit or request-log hit).

### Git skill library (phase 4, P4-GSL, D-148, D-149, SEC-83; any git URL since P5-SKLIB-1, D-448, SEC-148)
- **Git server in the test kit** (`aurora_testkit::git_server::GitServer`, P5-SKLIB-1): fixture repositories built
  with the `git` command line in a temporary directory (commits, branches, annotated and lightweight tags, a
  moving head, renames and deletions, symlink and submodule entries, an oversize file, 51 files, a very large
  tree, hostile names and descriptions, a SHA-256 repository) and served by **`git http-backend`** (CGI) behind a
  small axum server, with switches: protocol v2 off, `uploadpack.allowFilter` off, `uploadpack.allowAnySHA1InWant`
  off, Basic auth with a given token, an injected status (401/403/404/429 with `Retry-After`/503), redirects (same
  origin, another origin, to `http:`), a slow answer, an oversize or wrong-objects answer, and a log of the
  requests received with their `Authorization` and `Git-Protocol` headers. It needs `git` 2.31 or later on the
  machine and **panics with a clear message when `git` is missing** (never skips). The API reaches it through
  the guarded client's test override (`Env::override_origin`), which maps every https origin to the server while
  the reader still decides by the logical URL which origin gets the token and which addresses are allowed.
- **Registry** (`reg/it/library.rs`): the `library` block still parses and lints (the linter matrix of
  `09 §3.4 library`) but is **ignored**: the warning `library_ignored`, and the built-in GitHub and GitLab
  packages carry no block.
- **Core** (`aurora-core` `git_url`): the address table of C-1123 (30+ cases, every refused form with its reason).
- **API unit and property tests** (`skill_libraries::git`): the pkt-line and pack parsers never panic (truncated
  input, bad lengths, flush/delim, bad zlib, delta offsets out of range, chain depth 51, object count over cap,
  wrong object id, SHA-256 capability).
- **Reader** (`api/it/skill_library/git.rs`, SEC-148): every server configuration (v2 with and without filter,
  v0, v0 without any-sha1-in-want → whole-pack fallback), branch, annotated tag (peeled), lightweight tag, full
  commit, default branch by symref, a folder, a private repository (no token, wrong token, right token, other user
  names, 403 and 404 texts), three redirects allowed and a fourth refused, redirects to `http:`, private,
  loopback, metadata and IPv6 literals and to credentials refused, **`Authorization` absent from every request
  to another origin**, a pack over `LIBRARY_PACK_MAX`, objects that were not asked for, an inflate over
  `LIBRARY_INFLATE_MAX`, SHA-256, a slow host, more than `LIBRARY_BROWSE_MAX` skills, the check route (shape,
  stores nothing, org admins only, 10 a minute), the token's life (sealed, replaced, kept on a failed re-read,
  removed, never in a response, `last_error` or audit row), `url` patch only while null, blob ids equal git's own
  ids, and a source scan that only the library service opens the token. **Instance libraries**
  (`api/it/skill_library/instance.rs`, P5-SKLIB-2, SEC-149): two organizations on one instance library (their own
  numbers, `?org=` browse and chips, 400/404 rules), the authority per tier of caller (instance admin session; org
  owner/admin/member, stranger and API key all 404), one refresh writing one proposal per skill with per-organization
  budgets (an organization over budget gets a state row and no proposal while the other gets its own), deletion with
  an audit row in each organization, paused libraries, the kill switch and `LIBRARIES_PER_INSTANCE`; the migration
  test `skill_library_instance_migration` (an `over_budget` row moves to its organization's state, null `org_id` is
  one scope for the unique indexes). Migration test (`db/it`
  `skill_library_git_url_migration`): a database with GitHub, GitHub Enterprise, GitLab with an instance and
  without, a duplicate and a third-app library.
- **API** (`api/it/skill_library.rs`, real Postgres, the git server of the test kit):
  create/list/patch/delete with the permission matrix (org admin yes, member no, address refusals with their
  reasons, same address 409, limit 409), browse (live, nothing stored, validity and reasons, caching 5 min), **add** at each tier
  with its authority (agent `can_use`, workspace `can_own`, org admin; agent tool and agent credential
  refused; the exact requested commit is read even if the head moved), validation (symlink, submodule, big
  file, bad frontmatter name, reserved name, size limits), uniqueness and shadowing across tiers unchanged,
  **budgets** per agent/workspace/org at add and approve, **read-only** enforcement through every route and tool,
  detach and library deletion; **refresh** under the lease (two replicas never refresh one library; lease
  expiry; backoff; unchanged head costs one discovery and one `ls-refs`; changed blobs → one proposal;
  superseding; deletion → `removed`; invalid → `invalid`; over-budget; repository gone or turned private,
  token revoked; host throttling honours `Retry-After`; interval floor) with the invariant checks of SEC-83 (no change to `skills`, `skill_files`, any
  home-sync generation or the sync parts until approval); **approve** (session only; hash quoted; race with
  a newer proposal → 409 `update_superseded`; re-validation; audit rows critical; generation bump for exactly
  the affected agents), reject remembered, `skill.library.update_proposed` best-effort.
- **Supervisor/projection** (`sup`, `api` + `fake` runner): library skills are projected exactly like manual
  skills (read-only modes, manifest pruning, tier precedence); a pending proposal changes nothing in the home.
- **Dashboard** (`web/tests`): libraries page and connect dialog (errors in words, read-only warning), Add from
  library flow (preview of the exact stored content, over-budget disabled), badge and read-only banner, detach and
  remove confirms, update review (full diff, disabled approve for non-authority with explanation), repository text
  rendered as data (an HTML/markdown/URL payload is inert), no horizontal scroll at 390 px, both themes.

### App guides on demand (phase 5, P5-GUIDE-1..7, D-170..D-172, D-295..D-297, SEC-91, SEC-117..SEC-120)

**Amendment of 2026-11-22 (D-452, D-453; P5-APPSKILL-1/2) — supersedes every guide-file, recipe,
`fixture-guide` and `aurora plugin guide` mention below.** Apps ship skills; the agent guide is gone.
The tests that replace the guide ones:

- **Shared validator** (`aurora-registry::skill_format`, SEC-150): unit tests there; the library
  ITs (`services/aurora-api/tests/it/skill_library*.rs`) and the skills-service tests pass
  **unchanged** and are the proof the API's behaviour did not move.
- **Registry** (`tests/it/app_skills_lint.rs`): one case per `app_skill_*` / `app_skills_*` code
  (unknown app, name and prefix, the longest matching app id, frontmatter, body, total, path
  grammar with `Skill.md` as a file name and depth 5, not text with NUL / BOM, file size 64 KiB + 1,
  51 files, 17 skills, per-app size), a legacy `agent.guide` package that is valid with exactly one
  `agent_guide_ignored` warning, the operations-file codes, archive paths. `tests/it/app_skills_package.rs`:
  the digest of a package without `skills/` equals the value recorded on master, an executable
  skill file adds `\0x` to its line, flipping the bit breaks the signature (both ways), the reader
  records the bit under `skills/` only, a link under `skills/` is refused, every built-in passes
  without a warning. `tests/it/agent_guide.rs`: the snapshot carries `skills` sorted with digests
  and bits, no `guide` key, a stored version with `agent.guide` still compiles.
- **API** (`tests/it/plugins_agent_guide.rs`): `agent_file` blobs read by digest, nothing stored for
  old guide files or for an `invalid` package, the snapshot's `skills`, a package breaking a rule is
  `invalid` with the code, the admin detail `agent` and the file routes (`text/plain`, `nosniff`,
  404, 400, 403), `changed` and `agent_changed` (a bit flip alone counts); migration
  `20261201000272` (`schema_matches_spec`, `scripts/extract-schema.py --check`).
- **CLI** (`tests/plugin.rs`): `preview` against `tests/golden/preview/` (skills with the provenance
  line, modes 0555/0444), `build` keeps `100755` in the digest, `validate` warns for an old guide,
  `docs` lists skills, `import-openapi` goldens without reference files (`tests/golden/openapi/`).
- **Fixtures**: `fixture-app-skills`, `fixture-legacy-guide`, `fixture-invalid-app-skill-*` and
  `fixture-invalid-agent-operations-missing` with their lines in `expected-invalid.txt`
  (`scripts/fixtures/build.sh`; the oversize cases are written by `mkfixture.py`).

**Amendment of 2026-11-15 (D-295..D-297).** Where the list below says *inline* or *index*, read:
the supervisor snapshots are **index always** (one, two and 23 apps), the older-API fallback
(no `guide_path` → today's inline file byte for byte), and the paragraph with and without
`agent_cli`; the "inline at the threshold" case is gone. Added:

- **Recipes** (P5-GUIDE-1): linter rows for `agent_recipe_not_markdown` and `agent_guide_kind`;
  renderer snapshots of `guide.md` with recipes (section order, *Procedure* header on the
  projected file); `fixture-guide` gains one recipe.
- **Operations and `api.json`** (P5-GUIDE-6): one `fixture-invalid-agent-operations-*` package
  per rule of `24 §3.5` (format, unknown key, forbidden header in each case variant, foreign
  host, path with `..`/`//`/query, undeclared or duplicate path parameter, bad JSON path,
  unknown tool, too many, too large, listed in `guide`); `api.json` `insta` snapshots: package
  operations mapped by the gateway matcher (including one that maps to no tool → `untooled`
  access), catalogue-only operations, two accounts with different grants, an instance-bound
  account's `base_url`, truncation at a lowered `AGENT_API_JSON_MAX`; `import-openapi` writes a
  byte-stable `operations.json` for the three checked-in documents and it validates.
- **`aurora-agent`** (P5-GUIDE-6, `services/aurora-supervisor` unit tests with a local mock proxy):
  `search` ranking on a fixed `api.json`; `describe` bound; `call` argument coercion and refusals
  (missing required, unknown name, enum) with exit 2 and **no request sent**; path encoding of
  `/`, `..`, `?`, `#`, `%2F` as one segment; `--path` refusals (scheme, `//host`, `@`); account
  selection (implicit with one, refused with several and none given); `x-aurora-connection-id`
  set and no other Aurora header; pagination for each style, the page cap and a cross-origin
  `next` refused; save path, `AGENT_CLI_SAVE_MAX` stop, preview bound on a 10 MiB body, `--raw`;
  exit 1 on upstream 404, exit 3 on a gateway refusal body (SEC-118, SEC-120); options that do
  not apply to the target refused with exit 2 and nothing sent; the linter rejects a package
  with an app id `aurora` (`app_id_reserved`). P5-GUIDE-7 adds: one `search` returns app
  operations, recipes and platform tools ranked together on a fixed `api.json` plus a fixed tool
  list.
- **Platform tools on demand** (P5-GUIDE-7): registry test that every tool declares a family and
  the core set equals `send_file`, `complete_task` (a new tool that forgets `family` fails the
  test); bridge tests: `tools/list` in `on_demand` = core set, in `all` = everything, without
  `agent_cli` = everything; `tools/call` of an on-demand tool served in every mode; instruction
  snapshots for the *platform tools* fragment (both modes, a family with an unavailable tool
  omitted); `aurora-agent` target-`aurora` tests: no `AURORA_SESSION` → exit 2 without touching the socket,
  argument coercion against a schema, preview/save, error result exit 1, socket down exit 4;
  `AURORA_SESSION` present in a harness spawn's environment and absent from terminal, SSH and
  `process_start` environments; session release: changing an on-demand tool's description
  releases no idle session, changing the core set does (SEC-119).
- **End to end** (kind, `fake` harness scripted to run shell commands): `aurora-agent call
  fixture-guide <op>` reaches the mock upstream with the connection header and the gateway's
  injected credential; the same operation under a grant that blocks it exits 3; `aurora-agent call aurora
  memory_save key=… content=…` produces the same `tool.started` / `tool.finished` events and audit
  row as the MCP path.
- **Live, env-gated** (as below, reported, not a gate): with `on_demand`, an agent asked to
  "remind me every Monday at 9" creates a schedule through `aurora-agent call aurora`; with three connected
  apps, an agent asked for the Indico free-room task opens the recipe before calling. P4-EVAL
  runs comparing `all` and `on_demand` are recorded in P5-GUIDE-7's completion report (`24 §13`).

- **Fixture packages** (`plugins/fixtures`): `fixture-guide` (`09 §13`) and one
  `fixture-invalid-agent-guide-*` package per linter rule of `24 §3` (unlisted file, missing file,
  reserved name, bad name, not text, file too large, summary required), each with its line in
  `expected-invalid.txt`.
- **Registry** (`crates/aurora-registry/tests/it/agent_guide.rs`): the linter matrix; a package
  without the new keys keeps its digest and its snapshot fragment byte for byte; snapshot carries
  `summary` and `guide[]` with `sha256`/`size` and omits both when absent; the **renderer**
  (`agent_guide::render`) under `insta` snapshots for: one account full, one account custom
  (allowed / approval / blocked all present), two accounts with different grants (per-letter
  states), a lost host claim (`unavailable`), `block_untooled`, an instance-bound and a
  tenant-bound account (host shown), a database app, an app with no tools, classifiers (`graphql`,
  `git`, `params`), reference files left out by the budget; a hostile-text corpus (names, labels,
  titles, descriptions, essentials with backtick runs, headings, control characters) never leaves
  its line or fence; the property of SEC-91.
- **API** (`services/aurora-api/tests/it/home_agent_guides.rs`, and `home_render_inputs.rs`):
  the `apps` part carries exactly the folders of the apps in the render inputs, every path is in
  the manifest, entries fit a part; `render_inputs.apps[]` keeps its old fields and gains `app_id`,
  `summary`, `guide_path`; a custom grant edit, a revoked grant, a disconnected account, an app
  made unavailable and a new package version each produce the expected next generation (changed
  `tools.md`, folder gone, new reference file); `AGENT_GUIDE_SYNC_MAX` (lowered in the test
  configuration) drops reference files in `app_id` order, keeps the generated files and counts the
  metric; a guide file whose bytes equal a blob of another kind is still served (read by digest);
  an `invalid` package stores no guide blob; `agent_guide_changed` is in the audit metadata; the
  guard `scripts/guards/agent-guide-advisory.sh` (SEC-91); the admin routes
  (C-648): shape, `changed` against the active version, instance-admin only, `text/plain` +
  `nosniff`, 404s.
- **Supervisor** (`services/aurora-supervisor`): `.aurora/apps/` managed-root rules (exact depth,
  name grammars, traversal refused, 0444/0555, prune); instruction snapshots for **inline** (at the
  threshold), **index** (one character above it), an older API (no `guide_path` → today's file,
  byte for byte against the existing snapshot), and the preamble paragraph present only with an
  app; a sync that changes only `.aurora/apps/**` re-renders identical instruction files and
  releases no session.
- **Rolling upgrade**: regression tests written *before* the feature code pin what an older
  supervisor does with the new wire: a sync with an unknown part kind and with unknown
  render-input fields commits and renders the pre-feature instruction snapshot (they pass on the
  unmodified supervisor and must keep passing for the `_` arm); the compiled snapshot of the
  built-in packages as they are before P5-GUIDE-4 has an unchanged hash after P5-GUIDE-1.
- **CLI**: `aurora plugin guide` output equals the API's for the same inputs (shared renderer,
  one golden directory); `new` scaffolds a package that validates; `import-openapi` (P5-GUIDE-5)
  on three checked-in documents (small 3.0 JSON, 3.1 YAML with tags and `$ref` parameters, one
  that exceeds `AGENT_GUIDE_APP_MAX`): output is byte-stable across two runs, validates, stays
  within the limits, and the dropped-detail report is printed for the third.
- **Built-in packages** (P5-GUIDE-4): all validate and pass their fixtures unchanged; one supervisor
  snapshot with every built-in app connected (index mode) records the size of the *connected apps*
  fragment, which must not exceed 270 characters per app plus the paragraph.
- **End to end** (`fake` harness, kind): a hosted agent granted `fixture-guide` under a custom
  grant has `.aurora/apps/fixture-guide/{guide.md,tools.md,api.md,operations.tsv}` with the expected
  content and modes; revoking the grant removes the folder at the next sync.
- **Live, env-gated** (harness conformance against `claude-code` and `codex` with an LLM endpoint):
  in index mode the agent is asked for something that needs an endpoint named only in
  `fixture-guide`'s `api.md`; the mock upstream must receive a request on that path. This is the
  behavioural check that an agent reads the guide before calling; it is reported, not a merge gate.
- **Dashboard** (P5-GUIDE-3): Agent guide section in the plugin view and in the review, change
  marks, the plain-text viewer with an HTML/Markdown payload inert, a 64 KiB file, no horizontal
  page scroll at 390 px, both themes.

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
  re-authentication (`reauth=1`; (phase 5, FIX-157, D-317) no operation answers `reauth_required` any more, account deletion included);
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

### Live narration cards (phase 5, P4-NARR, D-279, D-280, C-846..C-852, SEC-111)
- **Projector** (`api/it/narration.rs`, a scripted turn with fake tool events): one card per turn created in place of the receipt; steps added and updated from `tool.started`/`tool.finished`; `NARR_MAX_STEPS` cut with "N earlier"; the card collapsed (D-286: one closing update with the collapsed content of C-847, no delete op) at answer post, at failed turn and at stalled turn, the closing update retried after `rate_limited` and never superseded, a collapsed card not counted toward `NARR_CARDS_PER_SPACE`; restart mid-turn resumes the same card (no second `narr:<turn_id>` post).
- **Privacy** (SEC-111): no `activity`, `thinking.delta` or `text.delta` content in any card write; a shared-space card carries names only, a `direct` one carries the sanitized summary; canary argument and canary secret absent from every card write and from the connector request log.
- **Budget** (`NARR_MIN_INTERVAL`, `NARR_SPACE_UPDATES_PER_MINUTE`, `NARR_CARDS_PER_SPACE`): bursts of tool events coalesce; the space cap holds under two concurrent turns; a third turn in a full space gets no card and its answer is unchanged; `rate_limited` drops the update and never retries it.
- **Connector** (Mattermost, `20 §Rendering`; pinned dev server): a card with `@here`, `@all`, `<@user:…>`, a group mention and a bare URL in a label produces no notification and no link preview (checked against the server's notification and preview data); the card fits `message_max_chars` at the cap.
- **Degradation**: a connector without `edits` gets no card and the receipt of `13 §4.5`; `forbidden` on a card write leaves the turn running and the answer posted.
- **Dashboard**: vitest for the Channels switch (default off, PATCH on change, error inline); Playwright at 390 px and both themes for the help text.

### User tokens (phase 5, P4-UTOK, D-281..D-284, C-853..C-859, SEC-112, SEC-113)
(Where it lives: the API tests are `services/aurora-api/tests/it/user_tokens.rs` — the connector is played over the real
connector plane with a connector token and a replica, the secret store is a memory store so "no version left" is a
count, and the canary token is searched in every response, queued op, stored row and audit row; the connector tests
are `connectors/mattermost/tests/user_tokens.rs` (a wiremock platform that records every request); the conformance
scenario is `s21` in `tests/conformance/connector`, run against the fake connector and, with a driver that can mint a
personal access token, against the live server; the dashboard is `web/tests/user-tokens.test.tsx` and
`web/e2e/send-as-me.spec.ts`.)
- **Add and verify** (`api/it/user_tokens.rs`, the connector mocked for verify): a token of the caller's own link → 201 and `active`; another platform user → 422 `user_token_mismatch`; rejected → 422 `user_token_invalid`; limit 5 → 409; store unavailable → 503 with nothing stored; the token absent from responses, audit rows, captured logs and the request-log body (canary token).
- **Custody** (SEC-112): the per-op copy exists from the send's claim to its terminal state and is then null with no store version left; a claim after revoke finds no copy and fails the op `credential_revoked`; the gateway, runner and sandbox fixtures carry no `user_token` purpose (static check plus boot-document snapshot).
- **Eligibility** (SEC-113): the matrix of C-856 — another speaker in a shared channel, scheduled, webhook, watch, peer, system, dashboard and API turns, a grant of another agent, an inactive token or link — each refused with its code and no approval row; the owner's own DM and the owner's own thread succeed to the approval step.
- **Approval**: a decision by another approver, even one with `can_approve`, refused; there is no `allow` route (route-table test); expiry sends nothing; `USER_TOKEN_PENDING_MAX` refuses the sixth pending send; revoke during a pending approval → rejected `credential_revoked` and no `message.post` reaches the connector.
- **Connector** (`aurora-connector-mattermost`, mock platform): only `GET /api/v4/users/me` (verify) and `POST /api/v4/posts` (send) carry a person's token; the footer is present and the post has no `ephemeral` or file; `401` → `credential_dead` and the token `invalid` with the presence untouched.
- **Audit**: one `user_token.send` row per send (no text, no token, no platform user id by value); the `user_token.revoke` row is critical; the cascade rows (deletion of owner, agent, integration) each appear once.
- **e2e (live, gated)**: against the pinned Mattermost dev server: add a personal token, grant an agent, the owner's DM turn asks for a post in a channel, the owner approves in the dashboard, the post appears under the owner's account with the footer; a second person's turn in the same channel cannot trigger it; revoke, then the same request is refused.

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

### Evals (phase 5, P4-EVAL, D-289..D-294, C-860..C-874, SEC-114..SEC-116)
- **Questions and settings** (`api/it/evals_questions.rs`, P4-EVAL-1): each kind's create and every invalid shape (422); the 100-question cap; `version` bumps; `can_use` visibility (404 without it); judge key refusals (`eval_judge_not_granted`, `eval_judge_not_usable`, `eval_judge_not_permitted`) and `can_own` for the change; the fingerprint's canonical field list and hash stability (unit tests); BYO → 422 `eval_not_supported`. **SEC-116**: a canary expected answer absent from audit metadata, the request log and the sandbox boot document.
- **Runs** (`api/it/evals_runs.rs`, P4-EVAL-2): three questions → three `eval` conversations with `source_event_id = eval:<run>:<pos>`; ordered results; lease expiry mid-run resumes without a duplicate turn or result; one active run (409), the daily cap (429), a frozen agent (423 at start, `cancelled` mid-run); scoring of numeric (exact, tolerance), text (normalised), a missing `ANSWER:` line (`fail`), a failed, timed-out or cancelled turn (`error`, `not_scored`).
- **Side-effect refusal** (SEC-114, `api/it/evals_refusal.rs` and `gateway` ITs): a canary write in an eval turn is refused with `eval_read_only` at the gateway and at each platform write tool, creates no approval and increments `refused_writes`; the same write in a normal conversation is unchanged; a read-only app tool runs; the tool registry exposes no eval route to the agent (snapshot); `channels/mirror.rs`: an eval conversation is never mirrored.
- **Judge** (SEC-115, `api/it/evals_judge.rs` with a mock provider, P4-EVAL-3): input is the question, criteria and that turn's answer only (canaries absent); strict `{verdict, rationale}` output, prose and extra keys → `judge_unparseable`; revoked grant, refused endpoint and inactive key send no request; an unreachable judge → `judge_unavailable` with a second healthy provider never called; hanging judge → `judge_timeout`; `max_tokens` fixed; usage summed on the run and `turns.usage` unchanged; grep over captured logs and audit rows finds no key value.
- **Schedules and notices** (`api/it/evals_schedules.rs`, P4-EVAL-2): fire through the due-work arm; frozen → `skipped_frozen` with no run; active run → `skipped_busy`; five failures → `failures`; the creator's lost access and the anchor rule; intervals below one hour → 422; the regression notice at a 10-point drop, not at 9, suppressed when `regression_notify` is off, counts only in the body (SEC-116).
- **Retention** (SEC-116): a run older than `EVAL_RUN_RETENTION` and its eval conversations are deleted through the normal path, the session purge recorded.
- **Dashboard** (P4-EVAL-4): vitest for the question dialog's validation, the judge control's disabled states, the cap message and the fingerprint diff; Playwright at 390 px and 1280 px in both themes, no horizontal scroll, axe clean at serious and critical levels; the page is absent for BYO agents and for callers without `can_use`.

### Outgoing webhooks (phase 5, P5-WEBHOOK-1/2, D-320, SEC-127)
- **`aurora-webhook` unit tests**: every C-909 validation code, each slot type and rule, the injection set (`{{secret:a}}`, `"`, `\`, `}`, `&`, `=`, CR/LF, NUL, non-BMP) literal and correctly escaped for JSON in and out of strings, form and text, optional slots left out in each position, the masked render, signature vectors (also in the setup guide's Python and Node snippets), a proptest of random strings through every slot, `effective_rate`.
- **API** (`api/it/connect.rs` → `outgoing_webhooks.rs`, the `webhook` package as shipped): connect (sealed secrets, config stored, nothing sent, a generated signing secret returned once), every refusal code and field path, the org switch, edit (secrets set, deleted and kept; the credential version moves only when secrets change; signing keep, generate, provided, off), reconnect keeps the configuration, preview, presets, the send test (rendered body, allowed headers, verifying signature, rate-counted, echoed secret withheld, audited) and the org ceilings; the URL, secret and signing canaries appear in no response, read endpoint, audit row or stored column. Permission-matrix rows and the `openapi_matches_checked_in` drift test cover the routes.
- **Gateway** (`gateway/it/apps.rs` → `webhook_send.rs`, a loopback HTTPS receiver): the receiver gets exactly the rendered body and the allowed headers (the agent's `Authorization`, `Cookie` and custom headers absent), the signature verifies, the delivery id is in the response and the request-log row, the row holds slot names and no value or secret; 405/415/413/422 before any upstream call or approval; dry run (masked, nothing sent, not rate-counted, not held); the rate limit (connection, org ceiling at once); `307` keeps the body, `303` becomes `GET`, a loop, `http` and private hops refused; 404 → `link_not_found`, an echoed secret → 502, a long answer truncated, a time-out → 504 `may_have_been_delivered: true` with one request seen, a connect failure → 502 `false`; the approval summary shows the slot values and no canary, an invalid send creates no approval, approve sends and deny does not.

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
pulled in. Tests stay on the directly touched crates, plus `aurora-db`,
`aurora-api` and the **reverse-dependency closure of `aurora-testkit`** (gateway,
supervisor, …: they build their databases from the same migrator) when
`crates/aurora-db/migrations/**` changed (migrations change behaviour
everywhere); `plugins/**` and `scripts/fixtures/**` select `aurora-api` (the
`connect::` tests run against built fixtures); a change under `deploy/`,
`images/`, `.github/`, `scripts/` or `justfile` runs
`bash scripts/guards/run-all.sh` (the guards read the chart, the images and the
workflows; FIX-103). Tests run under `cargo nextest` when it is installed
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
testkit test asserts it). **Leaked databases (FIX-103):** a killed run leaves its
`aurora_test_<uuid>` databases (`Drop` never ran); the testkit reaps those with no
connection older than two hours (the v7 uuid carries the creation time) once per test
process, and `just clean-testkit` says how to trigger it. Clones hold the advisory lock
`aurora_test_reap` **shared** while `CREATE DATABASE … TEMPLATE` runs; reaps (leaked
databases, other migration sets' templates) hold it **exclusive** and skip when it cannot
be had, so a reap never drops a template under a clone (its own key, not the template
build lock, which a builder of another migration set holds for a whole migration run).
`scripts/e2e/llm.sh` is a **manual** gate (live-gated, not in CI): it rebuilds the
sandbox images on every run unless `SKIP_IMAGE_BUILD=1`.
`AURORA_TEST_POOL_SIZE` overrides the per-`TestDb`
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
3. Build images (**amd64 only** today, on PRs and on main; multi-arch is not built — FIX-101 corrected the claim).
4. e2e-gateway, e2e-hosted (podman), dashboard Playwright on the host-process stack (phase 1, OPS-017: was compose; `m1.sh --kind` runs it against the kind dev stack).
5. Architecture guards.
6. Nightly: Kubernetes e2e on kind, live suites, the full-image hosted scenarios and the channel e2e (no performance run: see the table — performance is manual, on an idle machine). The kind run
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

**Skip policy (FIX-101).** A suite the specifications cite as evidence must run in CI or fail there; it may
not pass by skipping (REVIEW-5 pattern 2).
- Rust suites that need a service (podman, Redis) name a gate variable and call
  `aurora_testkit::require_or_skip("<GATE>")` first: gate set → run; gate unset and `CI` not `true` → print
  `SKIPPED:` and return; gate unset and `CI=true` → **panic**. The nextest `ci` profile keeps those suites out of
  the shards (`default-filter`), and the CI job `gated-suites` runs them with their gates: the pinned Redis
  service (`AURORA_TEST_REDIS`, `AURORA_TEST_REDIS_ADDR`: the counters conformance suite and FIX-087's
  documented-ACL case, SEC-62) and the runner's podman tests (`AURORA_TEST_PODMAN`, SEC-26), each requiring a
  non-zero passed count. (The Vault suites still skip silently outside their own `vault` job; converting them is
  left open.)
- The e2e scripts (`gateway.sh`, `hosted.sh`, `channels.sh`) run through `scripts/e2e/lib-skip.sh`: with `CI=true`
  a `SKIPPED:` line anywhere in the output fails the script, unless the job declares an exception in
  `AURORA_E2E_ALLOW_SKIPPED` (a regex, commented in the workflow). `scripts/guards/e2e-skip.sh` self-tests it.
- Playwright: the **mocked** project runs on every push in the `web` job, the **production-build** project
  (`AURORA_E2E_PROD=1 pnpm e2e:prod`, port 4110, the only place the real CSP applies) in the same job; the
  real-stack specs share one stack and run in the nightly `first-run` job and the kind scripts.
- Schema guard (`crates/aurora-db/tests/it/schema_matches_spec.rs`): compares table and column **names**, and, for
  every column `docs/03` types, the declared **type and nullability** with the migrated catalog (`udt_name`,
  `is_nullable`). It does **not** compare constraints, indexes, defaults, checks, foreign keys or partitions; the
  `KNOWN_TYPE_DRIFT` list may only shrink.
- Permission matrix (`permission_matrix.rs`): a denial row answered 400/422 (validation before authorisation) is
  INCONCLUSIVE and fails unless it is on the commented `ACCEPTED_INCONCLUSIVE` list in that file.
- Plugin fixtures: every manifest/archive/hooks rule code has an invalid fixture in
  `plugins/fixtures/expected-invalid.txt` (`bash scripts/fixtures/build.sh` prints the count; 57 passed at FIX-101).

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
