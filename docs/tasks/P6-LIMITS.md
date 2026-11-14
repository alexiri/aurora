# P6-LIMITS: Usage limits, quotas and load knobs — **proposal, spec batch (Opus) later — phase 6**

> **Status: proposal (2026-11-14).** Written ahead of phase 6; nothing here is decided. No D-, C-, SEC-
> or migration numbers are taken yet (the spec batch takes them from PROGRESS's next-free note). Open
> questions are `Q-P6-LIM-n` at the end; the recommendations are the coordinator's.

## Goal

Owner, 2026-11-14: "usage limits (to protect from the kind of abuse our gitlab gets): runtime of
sandboxes, number of schedules, rate limiting of expensive endpoints, etc. we need knobs to turn when
load is too high."

The GitLab abuse pattern is free compute used for something else (mining, scraping, spam), automation
that never stops (scheduled pipelines nobody looks at), sign-up and API floods. Aurora hands every user a
container with a shell, a cron and an egress path, so it is exposed to all of it. Two different needs:

1. **Fairness and abuse caps** — per user / org / workspace / agent limits that stop one actor from taking
   more than their share, enforced everywhere, visible to the people they apply to.
2. **Load knobs** — instance-wide levers an admin turns *now* when the cluster is overloaded, that work
   even when the dashboard itself is slow, and that degrade background work before interactive work.

## Today (what already exists)

| Area | Existing limit | Where |
|---|---|---|
| API | Per-principal token buckets by class (`general`, `write`, `message`, `upload`, `simulate`, `search`, `export`, `telemetry`) + per-org bucket; 20 concurrent streams per principal; live-tunable multiplier and class overrides (instance settings `limits`) | `05 §Conventions`, `02 API_RATE_*` |
| Auth | OIDC start/callback 20/min/IP; unauthenticated GETs 120/min/IP | `04 §Rate limits` |
| Shared counters | `CounterStore` (memory / postgres / redis), never fail-open | `15 §Shared rate-limit counters` |
| Gateway | Per-rule `ratelimit` outcome in policy; tunnel caps per credential/gateway | `06 §Rate limiting`, `§Tunnel limits` |
| Sandboxes | `IDLE_STOP` 30 min, `BACKGROUND_CAP` 12 h since last engagement, `TURN_CEILING` 6 h, terminal/preview/SSH awake caps 4 h, size 2 GiB / 1 CPU / 512 PIDs / 10 GiB home, `runner.max_sandboxes` 4, capacity wait 300 s then `at_capacity` | `11`, `02` |
| Metering | `runtime_intervals` (one row per boot, no FKs) and `runtime_daily` | `03` |
| Automations | 50 schedules and 20 webhooks **per agent**; cron minimum interval 5 min (fixed); 5 consecutive failures disable; webhook deliveries 60/min per webhook | `10`, `02` |
| Content | Attachment quota per org (10 GiB default, notices at 80/100 %), memories, skills, previews bytes/shares, skill-library bytes | `02`, `10` |
| Structure | Seat cap `max_members` per org (SCIM, placement) | `04` |

**Gaps.** Every count is per agent, so creating agents multiplies it (50 agents × 50 schedules every 5 min
keep 50 sandboxes awake forever: each fire is an engagement, so `BACKGROUND_CAP` never trips). Nothing limits
agents per workspace, workspaces or orgs per user, concurrent awake sandboxes per user/org, total awake
hours, turns per day, or schedule fires per day. Expensive endpoints that are not in a strict class (sandbox
start/restart, terminal open, SSH mint, connection probes, OAuth start, invitations, message search under
load) share `write`/`general`. There is no instance "load" lever except editing individual settings, and no
usage view for admins.

## Proposed model

### One quota framework

A **limit** is `(resource, scope, window, value)`:

- **resource** — from the catalogue below (`sandbox.awake_hours`, `schedules.count`, …);
- **scope** — who it applies to: `user`, `agent`, `workspace`, `org` (with the org tree,
  `P6-ORGTREE`, an org limit caps its **subtree**: usage counts against the org and every ancestor);
- **window** — `concurrent` (a gauge), `count` (things that exist), or a rolling/calendar period
  (`hour`, `day`, `month`);
- **value** — a number, or unlimited.

Where values come from, most specific wins **but never above the level above** (tighten-only, like
governance):

1. **Instance defaults** — a new instance settings section `quotas` (`15 §Instance settings`, Admin ›
   Limits; Helm may lock keys as for every movable key).
2. **Limit profiles** (recommended, Q-P6-LIM-1) — named sets of overrides, e.g. `standard` (= defaults),
   `trusted` (higher), `restricted` (much lower), created by instance admins and assigned to an org or a
   user. This is "plans" without billing (`21` says no billing/plans/budgets — this is abuse control, not
   charging; Q-P6-LIM-2 confirms the line). A user's effective limit is their profile's, capped by their
   org's.
3. **Org admins** may lower limits for their org, its children and its workspaces/users, never raise them.

Enforcement happens in one service in `aurora-api` (`quota::check(resource, subject)`), the same way
`authz` is one module: every surface that creates a counted thing or starts metered work calls it.

### Resource catalogue (first cut)

| Resource | Window | Scopes | Enforced at |
|---|---|---|---|
| `sandbox.concurrent_awake` | concurrent | user, org | sandbox start (wake). Over the limit: interactive work waits (`waiting_for_quota`, shown like capacity) up to the capacity wait, then fails `quota_exceeded`; background work is skipped (below) |
| `sandbox.awake_hours` | day, month | agent, user, org | metered from `runtime_intervals` (attributed to the turn's person / the automation's fire-time authority); at 100 %: no new background starts, interactive starts allowed until a hard ceiling (Q-P6-LIM-3); running sandboxes stopped `stop_reason = quota` at the next keep-awake check |
| `sandbox.background_cap`, `idle_stop`, keep-awake caps | duration | profile | existing deadlines become per-profile (restricted: 1 h background, 10 min idle) |
| `sandbox.size` | — | profile | CPU / RAM / PIDs per profile within the runner's bounds (runners already advertise axes and bounds) |
| `schedules.count`, `webhooks.count`, `watches.count` | count | agent (existing), workspace, user (as fire-time authority), org | create / re-enable / reassign; 409 `quota_exceeded` |
| `schedules.min_interval` | duration | profile | create/edit (today fixed 5 min) and **fire time** (a lowered floor stretches existing schedules without editing them) |
| `automations.fires` | day | workspace, org | fire time: over the limit the fire is skipped with a recorded reason, not counted as a failure |
| `agents.count`, `workspaces.count`, `orgs.created` | count | workspace / org / user | create |
| `turns.started` | hour, day | user, org | message post / automation fire (429 `quota_exceeded` with `retry-after` at window end) |
| `turns.concurrent` | concurrent | user, org | queued turns wait their turn instead of starting |
| `gateway.requests` | minute | agent, org | gateway, outer cap above policy rules (`429 rate_limited`, request-log row) |
| `gateway.egress_bytes` | day | agent, org | gateway (counted on the tunnel / response stream; over the limit: new requests refused) |
| `llm.requests` | day | agent, user, org | gateway LLM step (request count only; **tokens/spend are out**, see Q-P6-LIM-4) |
| `invitations.sent` | day | user, org | invitation create (mail spam) |
| `storage.*` | — | — | existing quotas (attachments, previews, skill libraries) move into the framework unchanged |

All values have defaults generous enough that no existing user notices on upgrade (Q-P6-LIM-5 sets them from
real usage: `runtime_daily` gives the distribution).

### Expensive endpoints

- New API rate-limit classes for routes that cost a sandbox, an external call or a heavy query:
  `sandbox` (start, restart, Retry now, terminal open, SSH mint — they wake containers), `probe` (connection
  and self-hosted host checks, OAuth start), `invite` (invitations, resend), and `search` extended to message
  and conversation search (P5-CHAT-4). Defaults set from measured cost; class overrides already live-tunable.
- **Concurrency caps per replica** for the heaviest handlers (search, export, simulate, storage refresh,
  backfills): a semaphore per class; when full → 503 `busy` with `retry-after` instead of piling up queries.
  This protects the database under load, which per-principal buckets cannot.
- The `org` bucket applies at every level of the org tree.

### Load knobs ("pressure level")

One instance switch with levels, each a bundle of overrides applied on top of profiles (all values
configurable; Admin › Limits › Load):

| Level | Effect (defaults to discuss) |
|---|---|
| `normal` | nothing |
| `elevated` | schedule floor 15 min; `automations.fires` halved; background cap 2 h; idle stop 10 min; API multiplier 0.5 for `general`/`write`; background sandbox starts queue behind interactive ones |
| `critical` | schedules, webhooks and watches **deferred** (fires skipped with reason `instance_load`, nothing disabled, the next fire after the level drops runs normally); no background sandbox starts; awake background sandboxes stopped at their next idle check; heavy classes closed (503 `busy`); new sign-ups refused with `instance_busy`; interactive chat keeps working within `sandbox.concurrent_awake` |

- Applied live through the settings reload (`NOTIFY aurora_settings`), so every replica and gateway switches
  within seconds.
- **Break-glass when the dashboard is unusable:** `aurora-api limits pressure set critical` (the existing
  settings CLI path) and a Helm lock. The CLI is the knob that must always work.
- Every change is security-critical audited and e-mailed to instance admins; a banner tells users
  ("Aurora is under heavy load — scheduled work is paused").
- **Per-subject knobs:** an instance admin can put one user or org on the `restricted` profile or pause its
  automations in one click (lighter than freeze: chat keeps working). Freeze stays the emergency stop.
- Automatic pressure (from runner capacity waits, queue depth, DB latency) is **not** in the first cut: the
  metrics and an alert rule ship, the switch stays manual (Q-P6-LIM-6).

### Behaviour at a limit

- **Counts** (`*.count`): checked at create/enable/reassign under the parent row lock (the `FIX-110`
  pattern); lowering a limit never deletes or disables existing items, it only blocks new ones (the UI shows
  "12 / 10 — over the limit, new ones are refused").
- **Rates and budgets:** refused with the standard envelope, code `quota_exceeded`, `details: { resource,
  scope, scope_id, window, limit, used, resets_at }`, `retry-after` when it resets.
- **Background before interactive:** budgets are spent by both, but when one is exhausted automations stop
  first; a person typing in chat is refused last.
- **Messages to people:** the conversation (dashboard and chat platform) gets a plain notice naming the limit
  and who can raise it ("This agent's daily computer time (8 h) is used up; it resets at 00:00 UTC. Ask an
  org admin to raise it."). Notices at 80 % and 100 % to the subject and the org admins, as the attachment
  quota does today.
- Fire-time skips are recorded on the schedule/webhook (`last_skip_reason`) and never count toward
  `SCHEDULE_FAILURE_DISABLE`.

### Metering and storage

- `usage_counters(scope_kind, scope_id, resource, window_start, value)` in Postgres for day/month windows,
  updated in the transaction that creates the counted event (turn start, fire, invitation) or by the job
  that closes `runtime_intervals` (awake hours, attributed per agent/user/org and every ancestor org).
- Minute-scale rates use the existing `CounterStore` (shared when Postgres/Redis is configured; the
  `security`/`fairness` failure classes of P4-CNT apply).
- Retention: daily rows 400 days (yearly comparisons), then dropped.

### Visibility

- **Admin › Usage:** top consumers by org/user/agent (awake hours, turns, fires, egress, LLM requests),
  current pressure level, refusals by resource, runner capacity; filters by org subtree.
- **Org › Usage:** the org's (and its children's) use against limits, who is near them.
- **My usage** on the account page; the agent's Computer section shows its own awake time and limit.
- Metrics: `aurora_quota_refused_total{resource,scope}`, `aurora_usage_total{resource}`, `aurora_pressure_level`;
  a Grafana panel and an example alert in `docs/operator`.

### Abuse signals (report first, act later)

Flags on Admin › Usage, no automatic action in the first cut (Q-P6-LIM-7): a sandbox at high CPU for hours
with no person engaged (mining), an agent whose automations fire far more than anyone reads, egress volume
spikes or many distinct destinations (scanning), bursts of sign-ups or org creations from one domain.

## Deliverables of the spec batch (when phase 6 starts)

Owning-doc changes: `02 §Limits registry` (catalogue, defaults), `03` (`usage_counters`, profiles,
assignments, skip reasons), `04` (permission matrix: who sets limits/profiles/pressure), `05` (quota error,
new classes, concurrency caps, Usage and Limits routes), `06` (gateway caps, egress bytes, LLM requests),
`10` (schedules/webhooks/watches at limits, turn refusals, notices), `11` (quota stop reason, concurrent
awake, per-profile deadlines and sizes, background vs interactive starts), `14` (Admin › Limits/Usage, Org ›
Usage, banners, counters in forms), `15` (instance settings section `quotas`, pressure CLI, metrics,
alerting), `17` (SEC: limits are enforced server-side on every path; an agent tool can never raise or bypass
one), `18`, `API_CONTRACTS`. Then implementation briefs, roughly:

| Brief (proposed) | Scope | Size |
|---|---|---|
| P6-LIM-1 | Quota framework: settings section, profiles and assignments, `quota::check`, `usage_counters`, error shape; move existing count caps into it | L, Rust |
| P6-LIM-2 | Sandbox limits: concurrent awake, awake-hour budgets from `runtime_intervals`, per-profile deadlines/sizes, quota stop, background-before-interactive start ordering | L, Rust |
| P6-LIM-3 | Automation limits: counts per workspace/user/org, profile minimum interval at create and fire, fires per day, skip reasons | M, Rust |
| P6-LIM-4 | API classes `sandbox`/`probe`/`invite`, per-replica concurrency caps, gateway request/egress/LLM-request caps | M–L, Rust |
| P6-LIM-5 | Pressure levels: settings, live reload, CLI, banners, audit + e-mail | M, both |
| P6-LIM-6 | Dashboard: Admin › Limits (defaults, profiles, pressure), Admin › Usage, Org › Usage, My usage, limit notices in forms and chat | L, Next.js |
| P6-LIM-7 | Abuse signals and metrics/alerts, operator doc | M, both |

Order: P6-LIM-1 first (everything depends on it); with the org tree, after P6-ORG-1 so org scopes count
subtrees from the start (or P6-LIM-1 first with flat orgs and the tree adding ancestors later — Q-P6-LIM-8).

## Open questions for the owner

- **Q-P6-LIM-1** Limit profiles (`standard` / `trusted` / `restricted`, assignable to orgs and users)?
  *Recommended: yes* — one knob per org instead of twenty numbers.
- **Q-P6-LIM-2** `21` says "not planned: billing, plans, budgets". Confirm these are abuse/fairness limits
  only — no prices, invoices or cost reporting.
- **Q-P6-LIM-3** When a person's awake-hour budget is used up, may they still chat (interactive) up to a hard
  ceiling, or does everything stop? *Recommended: background stops at 100 %, interactive at a separate hard
  ceiling (e.g. 2× the budget).*
- **Q-P6-LIM-4** LLM token or spend limits? Aurora does not count tokens (`21`: gateway token counting is
  out; LiteLLM does budgets). *Recommended: request-count caps only; token budgets stay in LiteLLM.*
- **Q-P6-LIM-5** Default values: set from measured production usage (`runtime_daily`) before the spec batch?
  *Recommended: yes — take a snapshot of the production distribution first.*
- **Q-P6-LIM-6** Automatic pressure levels driven by metrics, or manual only? *Recommended: manual + alerts
  in phase 6; automatic later once the signals are trusted.*
- **Q-P6-LIM-7** Abuse signals: report-only, or auto-restrict (put the subject on `restricted`)?
  *Recommended: report-only first.*
- **Q-P6-LIM-8** Build the org tree before limits, or limits first (flat orgs) with subtree counting added by
  the tree? *Recommended: limits framework first — it is the more urgent protection; the tree then adds
  ancestor counting in P6-ORG-1.*
- **Q-P6-LIM-9** Which abuse has CERN GitLab actually seen that we should design against explicitly (e.g.
  mining in CI, spam accounts, API scraping)? The catalogue above is the coordinator's guess.
