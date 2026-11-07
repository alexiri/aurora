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

Stored in `policy_rules` (hand) or `derived_rules.rule` (derived).

| Part | Shape / meaning |
|------|-----------------|
| `identities` | `[{ "kind": "agent", "id": uuid } | { "kind": "user", "id": uuid }]`. Empty = anyone. A `user` identity matches when the user is in the request identity's `principals` (users with `can_use` on the agent's workspace). |
| `targets` | Empty = anywhere. Kinds below. A rule matches if **any** target matches. |
| `action` | `allow` \| `block` |
| `require_approval` | allow only |
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
present), `is_llm_host`, `identity` (agent id, principals).

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
   consume quota).
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
| Connection `C`, mode `custom` | 1. `allow` target `connection{C, tools: allow_tools}` (if non-empty) · 2. `allow + require_approval` target `connection{C, tools: ask_tools}` (if non-empty) · 3. `block` target `connection{C}` (everything else, including requests with no matching tool) |

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

`GET /v1/agents/{agent}/effective` returns, for every candidate credential in
scope: whether it is selected, by which rule(s), and for each app tool the
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
- **Validation at authoring** (422): host patterns with >1 `*`, invalid path
  patterns, unknown identities (agent/user not in scope), connection/secret
  targets outside scope, unknown app or tool ids (warn only — may be a removed
  tool), modifiers on block rules, rate limits out of range, condition values
  too long.
- **Warnings** (`GET …/policy/warnings`): rules shadowed by an earlier
  unconditional rule with identical/broader identities and targets; rules
  naming removed tools or lost host claims (inert); allow rules in workspace
  scope fully overridden by an org block.
- **Simulate**: `POST /v1/workspaces/{ws}/policy/simulate` builds request facts
  from `{agent_id, method, url, body?}` using the same resolution code as the
  gateway (candidates, selection, app/tool identification) and returns the
  verdict, the winning credential, and the full trace; `use_draft: true`
  substitutes the draft generations.

## Approvals (gateway holds)

**Trigger**: final verdict `needs_approval` (an *ask* tool or an explicit
require-approval rule). The hold mechanics are in `06 §Approval holds`.

**Lifecycle**: `pending` → `approved` | `denied` | `expired` | `abandoned`.
First decision wins (`UPDATE approvals SET status=… WHERE id=$1 AND
status='pending'`); a losing decider gets 409 `already_decided`.

**Who may decide**: any user with `can_use` on the approval's workspace,
through the dashboard, the API (session or key of such a user), or a channel
card (clicker resolved through the user link; `13 §Clicker authorization`).
The chat platform never authorizes. (phase 1, RFE-009) The decider set is further
intersected with **conversation visibility**: an approval whose `conversation_id`
is a `direct` conversation can be seen and decided only by callers who
`can_see_conversation` it (the direct user and `can_own`; `04`). Approvals
without a conversation are unaffected. Other members neither see such approvals
nor receive their cards/notifications (SSE, long-poll, batch).

**Delivery**:
- Dashboard: `GET /v1/approvals/stream` (SSE) and the agent chat view (the
  `approval.pending` transcript event).
- API/SDK: `GET /v1/approvals/pending?wait=` long-poll.
- Channels: when the approval names a conversation that has a thread link, the
  channel core posts a card into that thread (`13 §Approval cards`). Approvals
  without a conversation (BYO without context credential, boot-token traffic)
  are delivered to the dashboard and API only.

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
3. Decision `approve` → `approved` (durable, so a crash before execution is
   visible) → handler executes → `executed` or `failed` with error. `deny` →
   `rejected`. Expiry → `expired`.
4. The outcome is appended to the origin conversation as a `notice` event and,
   when the origin turn has already finished, a system follow-up turn is
   queued ("Your message to #ops was approved and sent.") so the agent learns
   the result.

## Agent links and peer tasks

- **Agent link**: one row per unordered agent pair, policy per side (`ask`,
  `allow`, `blocked`). A message from A to B flows only if neither side is
  `blocked` and every `ask` side has approved (the approval is an action
  approval decided by someone with `can_use` on that side's workspace; approving
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
  decisions → `audit_log`.
- Every proxied request → `request_log` with the deciding rule's
  `logical_id` and denormalized `rule_name` (survives deletion).
