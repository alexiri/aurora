# 07 — Policy, Grants and Approvals

Phase: **0**. Crate: `aurora-policy` (pure, no I/O), linked by **both** the
gateway (enforcement) and the API (simulation, effective views, warnings).
There is exactly one implementation of every matcher and of the evaluator.

## One rule model, three jobs

1. **What a request may do**: allow, block, require approval, rate-limit.
2. **Which credentials may be injected** for an agent (injection selection).
3. **Defaults**: a per-scope Default Rule (deny-by-default posture for
   credentialed traffic).

## Rule sources and order

Per **scope** (organization; workspace) the evaluator receives one ordered list:

1. **Hand-authored rules** of the scope's *published* generation, by ascending
   `priority`.
2. **Derived rules** of the scope (from grants and app blocklists), ordered by
   `(source_kind, source created_at, ordinal)` — `blocklist` before `grant`.
3. The scope's **defaults**:
   - the **Default Rule** (`default_action`: `none` = no default) for
     requests on which a credential is injected;
   - the **uncredentialed default** (`none` | `block`, with an allow-list) for
     requests on which nothing is injected. For the org scope it comes from the
     org's effective **egress posture** (`08 §11.6`: `credentialed_only` ⇒
     `block`, allow-list = org egress allow-list ∪ instance baseline); for the
     workspace scope from the published generation's
     `uncredentialed_default` / `uncredentialed_allowlist`.

Hand-authored rules are the administrator's overrides and therefore come first;
grants provide the per-agent defaults beneath them.

LLM governance (`08 §11`) is not expressed as rules: it is enforced by its own
gateway step before evaluation (`06`), so it cannot be loosened by any rule.

## Rule anatomy

**Fail-closed speaker binding (phase 2, RFE-005).** A privilege tied to a
person is never granted to unattributed traffic: an **allow** rule whose
identities are `user` kinds does not match (so it neither allows nor *selects a
credential for injection*, see *Injection selection*) when there is no matching
speaker. A **block / require-approval / rate-limit** rule naming a `user` also
does not match speakerless traffic — that traffic is not that person's; to
restrict unattributed traffic write the rule with an `agent` identity or empty
identities. Authors who want "anyone with access" write empty identities, not a
roster of users. Existing phase-0 rules are not rewritten; their meaning
narrows, and the editor/warnings say so (below).

Stored in `policy_rules` (hand) or `derived_rules.rule` (derived).

| Part | Shape / meaning |
|------|-----------------|
| `identities` | `[{ "kind": "agent", "id": uuid } | { "kind": "user", "id": uuid }]`. Empty = anyone. An `agent` identity matches when the id equals the request's agent. A `user` identity: **phase 0** matched when the user is in the request identity's `principals` (users with `can_use` on the agent's workspace — "workspace roster" semantics, so a rule naming Alice matched every request by the agent whoever was speaking). **From phase 2 (RFE-005)** a `user` identity matches **only when the request's `speaker_user_id` equals that user**, i.e. the turn that caused the request was authored by that person (`06 §Credential authentication`). With **no speaker** (boot-token, BYO agent-token and BYO context traffic, schedule/webhook/watch/peer/system/bot-authored turns, or a speaker who lost `can_use`) a `user` identity **never matches**. Several identities are OR-ed. |
| `targets` | Empty = anywhere. Kinds below. A rule matches if **any** target matches. |
| `action` | `allow` \| `block` |
| `require_approval` | allow only. (phase 2, RFE-011a) With the optional `approval_timeout_secs` (60 … `approvals.max_timeout_secs`; absent = no preference, see *Approval deadlines*) |
| `rate_limit` | allow only: `{ "count": 1..100000, "per": "minute"|"hour"|"day" }` |
| `conditions` | AND-ed: `[{ "on": "body", "op": "contains"|"not_contains"|"equals", "value": string ≤1024, "json_pointer"?: string }]` |
| `enabled` | disabled rules are skipped |

Target kinds (matching uses the **request facts** below):

| Kind | Fields | Matches when |
|------|--------|--------------|
| `app` | `app_id`, `tools?: [tool_id]` | request's app = `app_id` and (no tools, or request tool ∈ tools) |
| `connection` | `id`, `tools?` | winning credential is that connection (and tool constraint) |
| `app_connections` | `app_id`, `scope: workspace|organization|any`, `tools?` | winning credential is a connection of `app_id` at that scope |
| `secret` | `id` | winning credential is that secret (generic or LLM) |
| `secrets` | `scope: workspace|organization|any`, `kind?: generic|llm` | winning credential is a secret at that scope (and kind) |
| `network` | `host` pattern, `path?` pattern, `methods?` | host/path/method patterns match |

**Request facts**: `host`, `port`, `path` (normalized), `method`, `body_prefix`
+ `truncated`, `app_id?`, `tool_id?`, `winning_credential?` (`connection{id,
app_id, scope}` \| `secret{id, kind, scope}`), `injected` (= winning credential
present), `is_llm_host`, `identity` (agent id, principals; from phase 2 also
`speaker_user_id?`).

## Matching primitives

- **Host patterns**: case-insensitive, no trailing dot. Either exact, or
  containing **exactly one** `*` that splits the pattern into prefix and suffix:
  a host matches if it starts with the prefix, ends with the suffix and is
  strictly longer than both together (`*.example.com` matches `a.example.com`
  and `a.b.example.com`, not `example.com`). Patterns with more than one `*`
  are rejected at authoring and, if ever encountered at runtime, match nothing.
  An optional `:port` in a pattern must equal the request port; without it any
  port matches.
- **Path patterns**: `/`-separated segments; a segment is a literal, `*` (exactly
  one non-empty segment) or, as the **last** segment only, `**` (zero or more
  segments). A literal segment may end with `*` for prefix matching within that
  segment (`/v1/file*`). Query strings never participate.
- **Methods**: uppercase list; empty = any.
- **GraphQL kind** (tools tagged `graphql: query|mutation`): parse the JSON
  body (`{query, operationName}` or a batch array) and the GraphQL document
  (`graphql-parser`). Result is `query` only if every operation that could
  execute is a query; **missing, truncated or unparsable body, subscriptions,
  or any doubt ⇒ `mutation`**. A block on mutations always holds on doubt; an
  allow on queries never admits a mutation.
- **Git operations** (tools tagged `git: push|fetch`): `…/git-receive-pack` and
  `…/info/refs?service=git-receive-pack` are `push`; `…/git-upload-pack` and
  `…/info/refs?service=git-upload-pack` are `fetch`.
- **Conditions and truncation**: a condition whose answer depends on bytes
  beyond the buffered prefix (or on an unparsable JSON body when
  `json_pointer` is set) is **unknown**. Unknown resolves by the rule's
  polarity: restrictive rules (block, require approval, rate limit) treat it as
  matched; plain allow rules as not matched. Doubt only tightens.

## Injection selection

Before any rule is evaluated, the gateway computes which candidates the agent
may use. A candidate is **selected** if at least one **enabled allow rule** (hand
or derived, either scope, regardless of order) whose identities match the
request identity has a target that *names* it: `connection{id}`,
`app_connections{app_id, scope}` covering it, `secret{id}`, or `secrets{scope,
kind}` covering it. `app` and `network` targets never select credentials. The
selection is a union and order-independent. **No selecting rule ⇒ nothing from
the workspace/org pool is injected** — there is no "all credentials" default.

(phase 2, RFE-025) **Broad rules arm future agents.** An allow rule with **empty
identities** that names credentials selects them for *every* agent of its scope —
including a BYO agent created minutes later by a member. That is intended for
convenience rules but easy to forget, so the policy editor and
`GET …/policy/warnings` flag it: warning `rule_applies_to_all_agents`
(severity `warning`, `details.rule_id`, `details.targets`) — "applies to all
agents, including agents created later" — for every enabled **hand-written** allow
rule (org or workspace) with empty identities and at least one target of kind
`connection`, `app_connections`, `secret` or `secrets` (derived rules always name
their agent; `app` and `network` targets select nothing and do not warn). When the
scope's effective BYO posture is `enabled` the warning's text adds "and by members
creating BYO agents". The warning never blocks publishing; the editor shows it
inline on the rule and in a confirmation step before publish listing the rules
concerned.

## Evaluation

```
verdict(scope) =
  for rule in ordered_rules(scope):
     if rule.enabled and identity_matches and (no targets or any target matches) and conditions_match:
         return outcome(rule)          # block | allow | allow+approval | allow+ratelimit(…)
  if not facts.injected and uncredentialed_default(scope) == block:
     if allowlisted(scope, host, path) → no verdict          # fall through to nothing: allowed unless the other scope objects
     else → block (reported as egress_denied)
  default_action(scope) applies:
     none  → no verdict
     allow → allow
     block → block only if facts.injected and not facts.is_llm_host, else no verdict
```

Under `uncredentialed_default = block`, an explicit allow rule that matches the
request (e.g. a `network` target) still allows it, because rules are evaluated
before defaults. Allow-list entries use the policy host/path patterns.

### Why the Default Rule skips LLM hosts (phase 3, RFE-023, D-66)

A Default Rule of `block` means "block every credentialed request that no rule
allows" — **except requests to governed LLM hosts** (`08 §LLM hosts`). This
carve-out is deliberate and was undocumented until phase 3:

- **Why.** The agent's own model loop is credentialed traffic: every turn calls an
  LLM host with an Aurora-injected key. A Default Rule that blocked it would brick
  every agent the moment an administrator chose "default: block" (the reasonable
  default-deny posture for *tool and app* credentials), and administrators would
  have to remember to write an allow rule for each LLM endpoint. Control of LLM
  traffic is a different question with its own, purpose-built controls: **LLM
  governance** (`08 §11`) decides which endpoints and keys may be used at all
  (instance → org, `sanctioned_only`, key origin verification, model allow-lists)
  and runs *before* policy in the gateway pipeline (`06`).
- **What it means.** An administrator who sets Default Rule = block and expects
  *everything* not allow-listed to stop will still see LLM traffic flow, subject to
  governance. To restrict LLM traffic use `08 §11` (posture `sanctioned_only`,
  endpoint permissions, model allow-lists) — and, for a specific host, an explicit
  rule.
- **What still applies to LLM hosts.** (1) **Explicit rules win**, as always: rules
  are evaluated before defaults, so a **block rule naming an LLM host** (a
  `network` target, or any target that matches it) blocks it, and so do
  require-approval and rate-limit rules; an explicit allow rule is simply
  redundant. (2) The **uncredentialed default** (`credentialed_only` egress) still
  governs requests to an LLM host on which Aurora injects nothing (the agent
  brought its own key): they are blocked unless allow-listed. (3) The `allow`
  Default Rule is unaffected (it allows anyway). (4) Everything in `08 §11`.
- (phase 4, P4-LLM, D-121) **More endpoint hosts, same rule, still exact.** The
  providers of `08 §Phase 4 providers` (OpenAI, Azure OpenAI, Bedrock, Vertex, generic
  OpenAI-compatible) add the **exact hosts** of their configured endpoints to the
  governed LLM hosts, so the carve-out covers them and only them: enabling Bedrock
  exempts `bedrock-runtime.<region>.amazonaws.com`, never `*.amazonaws.com`, and the
  S3/EC2 hosts of an AWS app connection stay under the Default Rule. Because such a
  credential (an IAM key, a service account) could do more than call a model, the
  carve-out is paired with the inference-only path set of `08` (SEC-65): a request
  the set refuses is refused by governance before policy, whatever the Default Rule
  says.
- The carve-out is exactly the pseudocode condition `facts.injected and not
  facts.is_llm_host`; it is evaluated per scope, so an org Default Rule of `block`
  and a workspace one behave identically. Simulation and the effective view show
  the reason `default_rule_skipped_llm_host` instead of "no verdict" when the
  carve-out applied to a request.

1. Evaluate the org scope and the workspace scope independently.
2. Map each to a **scope verdict**: `block`; `rate_limited` (the matched rule's
   counter for this agent and window is already at the limit); `needs_approval`;
   `allow`; or none.
3. **Combine by strictness**: `block` > `rate_limited` > `needs_approval` >
   `allow`. The final verdict is the strictest scope verdict; if neither scope
   produced one, `allow`. Consequence: a workspace rule can never loosen an org
   rule, and a workspace cannot widen the org's egress allow-list. When the
   strictest verdict comes from a Default Rule the reported decision is
   `default_denied`; from an uncredentialed default, `egress_denied`.
4. Rate-limit counters of matched rate-limited rules are incremented only when
   the final verdict is `allow` or `needs_approval` (a blocked request does not
   consume quota). (phase 4, P4-CNT) The evaluator stays a pure function: the
   caller passes in the current counts of the matched rate-limited rules (a
   `counts` argument, empty for rules without a count) and receives the list of
   `(rule logical_id, period, limit)` to consume when the final verdict allows;
   the gateway then consumes them atomically through the `CounterStore`
   (`06 §Rate limiting`), and a consume the store refuses turns the answer into
   `rate_limited`. With a shared backend a peek can be stale by one request; the
   consume is authoritative. Simulation (`POST …/policy/simulate`) neither
   reads nor consumes counters, as before (it reports the rule's limit, never a
   count), whatever the backend.
5. The evaluator returns a **trace** (per scope: rules considered, the matching
   rule, why) used by simulation and the request log (`rule_logical_id`,
   `rule_name`).

Performance: rules are part of the cached ResolvedHost; **the per-request
decision never touches the database**.

### CONNECT-time evaluation (open lane; phase 1, RFE-003)

In the open lane the gateway sees only `host` and `port` when it must decide
(the tunnel is opaque afterwards) and injects nothing. Phase 0 left rules with
`path`, `methods`, conditions or tools undefined at that moment. From phase 1:

- **Unknowable** facts at CONNECT time: `path`, `method`, `body_prefix`,
  `app_id`/`tool_id` beyond "this host is served by app X", and GraphQL/git
  classification. **Known**: `host`, `port`, the request identity, and that
  *no credential is injected* (`winning_credential = none`, `injected = false`).
- **Restrictive rules** (`block`, `require_approval`, `rate_limit`) treat
  every unknowable part as **matched** (consistent with *Conditions and
  truncation*: doubt only tightens): a `network` target matches on host/port
  alone even if it carries `path`/`methods`; an `app` target matches when the
  host is served by that app (its `tools` are treated as matched); body
  conditions are treated as matched. Targets that name a *winning credential*
  (`connection`, `app_connections`, `secret`, `secrets`) do **not** match: the
  tunnel uses no credential, which is known.
  - `block` → CONNECT refused, 403 `blocked_by_policy` (`details.connect_time =
    true`, so the agent learns the rule was applied to the whole host).
  - `require_approval` → refused (a tunnel is never held), 403
    `blocked_by_policy` with `details.reason = "approval_required"`.
  - `rate_limit` → the tunnel counts as **one request** against the rule.
- **Allow rules** need a **full match on what is knowable**: an allow rule
  matches at CONNECT time only if *every* constraint it carries is knowable and
  satisfied (identity, host, port). A rule with a `path`, `methods`, body
  conditions, tools or credential targets is treated as **not matched** (an
  allow scoped to `/org/**` must not admit a tunnel that can reach every path).
  The same holds for uncredentialed allow-list entries: an entry with a `path`
  does not admit a tunnel.
- The credentialed Default Rule never applies to the open lane (nothing is
  injected). The **uncredentialed default** applies as for the MITM lane
  (`credentialed_only`, workspace `block`) using these matching rules, and the
  open-lane posture of `06 §Open-lane admission and posture` adds the
  `allowlist_only` check.
- Scope combination, rate-limit accounting (increment only when the final
  verdict is `allow`) and the trace are as in *Evaluation*; the request-log row
  of a tunnel records `lane = open` and the deciding rule.

Policy simulation (`POST …/policy/simulate`) accepts `"lane": "open"` with a
`url` of the form `https://host[:port]` and evaluates under these rules.

## Grants and their compilation

A grant attaches a connection or a secret to one agent. Grants are compiled
into `derived_rules` in the agent's workspace scope, transactionally with the
grant write, and the gateway is invalidated (`agent` scope).

| Grant | Derived rules (identity = that agent) |
|-------|---------------------------------------|
| Secret `S` | 1. `allow` target `secret{S}` |
| Connection `C`, mode `full` | 1. `allow` target `connection{C}` (all tools, including future ones) |
| Connection `C`, mode `custom` | 1. `allow` target `connection{C, tools: allow_tools}` (if non-empty) · 2. `allow + require_approval` target `connection{C, tools: ask_tools}` (if non-empty; phase 2, RFE-011a: carrying the grant's optional `ask_timeout_secs` as `approval_timeout_secs`) · 3. `block` target `connection{C}` (everything else, including requests with no matching tool) |

(phase 2, RFE-011a) A grant may carry `ask_timeout_secs` (custom connection grants with `ask_tools`; validated like a rule's timeout, 422 `approval_timeout_out_of_range`).

Validation laws (422): a custom grant needs ≥1 tool in `allow_tools ∪
ask_tools` (message: "detach instead"); a tool may not be in both lists; every
tool id must exist in the app's current catalogue. **New catalogue tools
arrive blocked** in custom grants (they are in neither list). Tools removed by
a package upgrade remain in the lists, are flagged `inert` in the UI and match
nothing.

**App blocklists** (manifest `agent.default_blocklist`): when a connection is
created, a `blocklist` derived rule is created in the connection's scope:
`block` target `network{host}` for each listed host, identities empty. Deleted
with the connection.

Org-scoped connections and secrets can be granted to agents in any workspace of
the org; the derived rule lives in the agent's workspace scope.

### Tool levels in the UI

Each app's catalogue groups tools into `read` and `write`; a group may offer a
wildcard tool that is valid only if it is a true superset (host set, path
prefixes, method set) of the group — enforced by the plugin validator. For a
connection grant the UI offers per tool **allow / ask / block**; "full" is a
separate mode.

### Effective views

`GET /v1/agents/{agent}/effective[?speaker=<user_id>]` (phase 2, RFE-005: the
optional `speaker` selects the person whose turns are evaluated; default: the
caller; `speaker=none` shows speakerless traffic; credentials whose selection
depends on the speaker are flagged `depends_on_speaker`) returns, for every
candidate credential in scope: whether it is selected, by which rule(s), and for each app tool the
combined decision with the org-only decision exposed as `org_ceiling` so the UI
can explain locks. Computed by running the evaluator over synthetic request
facts per tool (one per tool's first method/path example).

## Authoring

- **Drafts**: each scope has exactly one draft generation. `PUT …/policy/draft`
  replaces the whole draft (rules, Default Rule, and — for workspace scope —
  `uncredentialed_default` and `uncredentialed_allowlist`) and requires the current
  `draft_revision` (409 `stale_revision` otherwise). Rules keep their
  `logical_id` when edited; new rules get a new one.
- **Publish**: copies the draft into a new `published` generation (previous
  becomes `superseded`), creates a fresh draft as a copy, invalidates gateways.
- **Rollback**: publishes a copy of an older generation's rules as a new
  generation (history is append-only).
- **Validation at authoring** (422): (phase 2, RFE-011a) `approval_timeout_secs`
  outside `[APPROVAL_TIMEOUT_MIN, approvals.max_timeout_secs]` →
  `approval_timeout_out_of_range`, and a timeout on a rule without
  `require_approval`; host patterns with >1 `*`, invalid path
  patterns, unknown identities (agent/user not in scope), connection/secret
  targets outside scope, unknown app or tool ids (warn only — may be a removed
  tool), modifiers on block rules, rate limits out of range, condition values
  too long.
- **Warnings** (`GET …/policy/warnings`; phase 2, RFE-005: rules with `user`
  identities carry the informational warning `user_rule_needs_speaker` — "matches
  only turns spoken by <name>; never schedules, webhooks, BYO or boot traffic" —
  and, for allow rules that grant credentials, `user_rule_no_agent_fallback`;
  phase 2, RFE-025: `rule_applies_to_all_agents`, see *Injection selection*):
  rules shadowed by an earlier
  unconditional rule with identical/broader identities and targets; rules
  naming removed tools or lost host claims (inert); allow rules in workspace
  scope fully overridden by an org block.
- **Simulate**: `POST /v1/workspaces/{ws}/policy/simulate` builds request facts
  from `{agent_id, method, url, body?, speaker_user_id?}` (phase 2, RFE-005:
  omitted = no speaker, so `user` rules do not match; the dashboard pre-fills the
  caller) using the same resolution code as the
  gateway (candidates, selection, app/tool identification) and returns the
  verdict, the winning credential, and the full trace; `use_draft: true`
  substitutes the draft generations.

## Approvals (gateway holds)

**Trigger**: final verdict `needs_approval` (an *ask* tool or an explicit
require-approval rule). The hold mechanics are in `06 §Approval holds`.

### Approval deadlines (phase 2, RFE-011a, D-56)

Phase 0 gave every approval the same fixed `APPROVAL_TIMEOUT` (300 s). A "deploy to
prod" ask deserves longer than a routine one, so from phase 2:

- A rule or custom-grant ask may set `approval_timeout_secs`. The **effective
  timeout** of a hold is the **minimum** of the timeouts *set* on the matching
  requiring rules of both scopes (a rule without one expresses no preference); if
  none is set it is `approvals.timeout_secs` (default 300 s). It is always within
  `[APPROVAL_TIMEOUT_MIN` (60 s)`, approvals.max_timeout_secs]`; the **instance
  maximum** defaults to 3 600 s, may be raised to at most 14 400 s, and startup
  refuses a maximum ≥ `TURN_CEILING − TURN_CEILING_WARNING` (a hold must not be able
  to outlive the turn ceiling).
- `approvals.deadline = now() + effective timeout` at hold creation (`06`); the card
  and dashboard show the exact expiry; `GET …/policy/simulate` and the effective view
  show the timeout a request would get; the transcript `approval.pending` event
  carries `expires_at`. Expiry still **denies** (`expired`, 403 `manual_approval_expired`).
- Clients may time out before a long deadline (an agent tool's own HTTP timeout is
  the client's business). Parking a request and resuming it later is **not** part of
  phase 2 (RFE-011b, phase 4).

**Stall exemption (phase 2, RFE-011a).** Stale-turn recovery fails a turn after
`TURN_STALL` (10 min) without progress. A turn that is *blocked on a pending
approval* makes no progress by design, so with timeouts beyond 10 min the platform
would have killed turns it was itself holding. The rule: **while any approval row of
the turn is `pending`** (gateway hold with `turn_id`, `deadline` not passed) the
turn is **exempt from stall detection**; the clock restarts when the approval is
resolved (`approvals.resolved_at`, set on approve, deny, expire and abandon). Stalled
means: no pending approval of the turn **and** `now − GREATEST(last_progress_at,
latest approval resolved_at) ≥ TURN_STALL`. The exemption never extends `TURN_CEILING`
(wall-clock cap, still `failed time_limit`) and does not apply to action approvals
(the agent is not blocked on them: it received `pending_approval` and continued).

**Lifecycle**: `pending` → `approved` | `denied` | `expired` | `abandoned`. (phase 2, RFE-006) Freezing the agent, its workspace or its org marks its `pending` approvals `abandoned` and pending action approvals `rejected` (reason `frozen`); unfreezing does not revive them.
First decision wins (`UPDATE approvals SET status=… WHERE id=$1 AND
status='pending'`); a losing decider gets 409 `already_decided`.

**Who may decide**: any user with `can_use` on the approval's workspace
(phase 0/1; **from phase 2 (RFE-012): `can_approve`**, i.e. workspace `owner` or
`approver`, or org owner/admin — a plain `member` cannot decide), through the dashboard, the API (session or key of such a user), or a channel
card (clicker resolved through the user link; `13 §Clicker authorization`).
The chat platform never authorizes. (phase 1, RFE-009) The decider set is further
intersected with **conversation visibility**: an approval whose `conversation_id`
is a `direct` conversation can be seen and decided only by callers who
`can_see_conversation` it (the direct user and `can_own`; `04`). Approvals
without a conversation are unaffected. Other members neither see such approvals
nor receive their cards/notifications (SSE, long-poll, batch).

**Delivery**:
- Dashboard: `GET /v1/approvals/stream` (SSE) and the agent chat view (the
  `approval.pending` transcript event). (phase 2, RFE-011a) With the user's opt-in
  the dashboard also raises a **browser notification** (below).
- API/SDK: `GET /v1/approvals/pending?wait=` long-poll.
- Channels: when the approval names a conversation that has a thread link, the
  channel core posts a card into that thread (`13 §Approval cards`). Approvals
  without a conversation (BYO without context credential, boot-token traffic)
  are delivered to the dashboard and API only.

- **E-mail when no surface is live (phase 2, RFE-011a).** A schedule- or webhook-
  triggered turn in a workspace with no channel presence used to die silently.
  Now, for every approval (gateway hold or action approval) still `pending`
  `APPROVAL_NOTIFY_DELAY` (20 s) after creation, the API checks for a **live
  delivery surface**: (a) a channel card whose `message.post` op finished
  successfully, or (b) a **watcher** — a user who can decide it and holds an open
  `approvals/stream` or `approvals/pending` long-poll seen within
  `APPROVAL_WATCHER_TTL` (45 s; replicas record `approval_watchers`). If neither
  exists it sends **one e-mail per decider** (the decider set of this section:
  `can_approve` users with an explicit workspace `owner`/`approver` binding,
  intersected with conversation visibility, `04`; if the workspace has no explicit
  binding the org owners/admins) through the SMTP mailer (`04 §Email`). The e-mail
  is platform voice, sanitized, and carries only: agent and workspace names, the
  approval's title (no summary fields, no body), the rule name, the expiry and a
  link to the dashboard approvals page (`<external>/o/<org>/w/<ws>/approvals?focus=<id>`)
  — **never** an approve/deny link (deciding needs a signed-in session). Approvals
  sharing a `batch_id` produce one e-mail; each user receives at most
  `APPROVAL_EMAIL_PER_USER_HOUR` (20) per hour, the rest folded into a "N more
  waiting" digest; `approval_notifications` makes it idempotent across replicas. A
  user can turn it off (`PATCH /v1/me { approval_email: false }`); with SMTP unset the
  mail is logged (log-only mode) and the instance overview warns "approval e-mails
  disabled". `approvals.email_notifications = false` disables it instance-wide.

**Transcript**: when an approval names a conversation, the API appends
durable events `approval.pending` (at creation) and `approval.decided`
(`{approval_id, decision, decided_by_label, via}`) or `approval.expired`.

**Batching**: requests carrying `x-aurora-batch-id` (+ label, total) are grouped
in UIs and cards ("Approve all 14 / Deny all"); a batch decision applies to all
*pending* approvals of that batch id within the same agent that the caller may
decide.

## Action approvals (platform-level)

For privileged agent **actions** that are not HTTP requests. Registered
handlers by `action_key` (phase 0: `send_message`, `message_agent` when the link
requires it). Unknown key at decide time → 500 and a logged error (fail loud).

1. A platform tool creates an `action_approvals` row with the **frozen
   payload** and a **platform-composed** one-line summary (never raw model
   text; recipient labels sanitized) and returns to the agent
   `{ "status": "pending_approval", "approval_id" }`.
2. Cards/dashboard as for gateway holds.
(phase 2, RFE-012) Action approvals follow the same rule: decided by `can_approve`.
3. Decision `approve` → `approved` (durable, so a crash before execution is
   visible) → handler executes → `executed` or `failed` with error. `deny` →
   `rejected`. Expiry → `expired`.
4. The outcome is appended to the origin conversation as a `notice` event and,
   when the origin turn has already finished, a system follow-up turn is
   queued ("Your message to #ops was approved and sent.") so the agent learns
   the result.
5. (phase 1, RFE-009, D-47 clarification C) **Cross-workspace approvals.** The
   conversation-visibility rule applies to an approval only within the
   conversation's own workspace. An action approval whose workspace differs from
   its conversation's (the other side of a cross-workspace `message_agent`
   sent from a direct conversation) is decided under its own workspace's law,
   but its decider sees only the platform-composed summary: the frozen payload
   (including the message text) and the conversation and turn ids are never
   exposed to them in any DTO, card or activity row (`conversation_id` and
   `turn_id` are `null`, `conversation_hidden: true`).

## Agent links and peer tasks

- **Agent link**: one row per unordered agent pair, policy per side (`ask`,
  `allow`, `blocked`). A message from A to B flows only if neither side is
  `blocked` and every `ask` side has approved (the approval is an action
  approval decided by someone with `can_use` on that side's workspace — phase 2, RFE-012: `can_approve`; approving
  sets that side to `allow`). The first `message_agent` creates the link with
  both sides `ask` (or `allow` for agents in the same workspace — same
  operators).
- **Peer task**: opened by `message_agent` when the sending turn has a person as
  speaker; creates a `peer` conversation on the receiving agent and a turn.
  One open task per ordered pair (later ones `queued`); `PEER_TASK_BUDGET`
  messages and `PEER_TASK_TTL`; exactly one report reaches the requester's
  origin conversation — the receiver's `complete_task` report, or the
  platform's own close line on budget/TTL exhaustion or cancellation.
- **App-turn cap**: turns whose speaker is a bot or agent increment
  `conversations.app_turn_streak`; any person-authored turn resets it. When the
  streak reaches `APP_TURN_CAP` further bot/agent turns are refused with a
  notice until a person writes or an owner clicks "continue" (resets the
  streak). No timers: people meter loops by being present.

## App availability

`organizations.app_posture = open` (default): every active app is available.
`restricted`: an app is available to a workspace only if
`org_app_availability` has `(org, app, NULL)` or `(org, app, workspace)`.
Enforced at connect time (403 `app_unavailable`) and in the gateway (candidates
of unavailable apps are dropped; requests get `app_unavailable`).

## Audit and logs

- Policy publish/rollback, grant changes, approval decisions, action-approval
  decisions → `audit_log`. (phase 2, RFE-008) These are **security-critical**:
  the audit row is inserted in the same transaction as the publish, grant write
  or decision (`04 §Audit`), so a decision that cannot be audited is not made
  (the decider gets 503 `audit_unavailable`; the approval stays `pending`).
- Every proxied request → `request_log` with the deciding rule's
  `logical_id` and denormalized `rule_name` (survives deletion).
