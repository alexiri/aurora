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

**Group identities (phase 4, P4-GRP-1, D-131, SEC-71).** A `group` identity
(`{ "kind": "group", "id": uuid }`, a directory group of the rule's org, `04 §Groups`)
matches **only when the request has a speaker and the group id is in the evaluator's
principal set**, i.e. the turn's author is currently a member of that group. The
principal set is `{agent_id} ∪ speaker_group_ids` (`06 §Credential authentication`):
the gateway resolves the speaker's groups once per credential resolution and passes them
in; `aurora-policy` stays pure and never loads a group. Everything said for `user`
identities applies unchanged and for the same reason: **with no speaker** (boot-token,
BYO, schedule, webhook, watch, peer, system, bot-authored turns, or a speaker who lost
`can_use`) a group identity never matches, so a privilege granted to a group is never
granted to unattributed traffic, and an allow rule naming only groups neither allows nor
selects a credential for such traffic (*Injection selection*); a block, require-approval
or rate-limit rule naming a group also does not match it. Identities are OR-ed with the
other kinds. Three more rules keep the semantics from widening:

- **A removed group never turns a rule into "anyone".** Deleting a group does not rewrite
  rules (published generations are immutable and drafts are not touched): a rule whose
  identities are non-empty keeps them, and a dead group id is simply in no principal
  set, so such a rule matches nobody. The author is told by the warning
  `rule_names_removed_group` (below); an empty `identities` array is still the only way
  to say "anyone".
- **Authoring validates.** A `group` identity must name a group of the **scope's org**
  (the org rule's org, the workspace rule's workspace org), else 422
  `unknown_identity`; a rule holds at most `RULE_GROUP_IDENTITIES_MAX` (20) group
  identities. Group ids are not secrets; the editor shows names.
- **Org and workspace stay separate.** A group identity in an org rule matches speakers
  of any workspace of the org (the group is org-wide); in a workspace rule it matches the
  same way. Membership, not workspace binding, is what is tested: a group that holds no
  binding on the workspace still matches its members' turns there (a person must still
  have `can_use` to be a speaker at all).

Stored in `policy_rules` (hand) or `derived_rules.rule` (derived).

| Part | Shape / meaning |
|------|-----------------|
| `identities` | `[{ "kind": "agent", "id": uuid } | { "kind": "user", "id": uuid } | { "kind": "group", "id": uuid }]` (phase 4, P4-GRP-1: `group`, below). Empty = anyone. An `agent` identity matches when the id equals the request's agent. A `user` identity: **phase 0** matched when the user is in the request identity's `principals` (users with `can_use` on the agent's workspace — "workspace roster" semantics, so a rule naming Alice matched every request by the agent whoever was speaking). **From phase 2 (RFE-005)** a `user` identity matches **only when the request's `speaker_user_id` equals that user**, i.e. the turn that caused the request was authored by that person (`06 §Credential authentication`). With **no speaker** (boot-token, BYO agent-token and BYO context traffic, schedule/webhook/watch/peer/system/bot-authored turns, or a speaker who lost `can_use`) a `user` identity **never matches**. Several identities are OR-ed. |
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
`speaker_user_id?`; phase 4, P4-GRP-1: `principals` also holds the speaker's group ids).

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
- (phase 5 (D-138, moved from phase 4), P4-LLM, D-121) **More endpoint hosts, same rule, still exact.** The
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
   `rule_name`). (D-178, Q-R5-14) The block response to the agent names the deciding rule
   and the workspace policy URL only for a **workspace** rule; for an **organization**
   rule it answers generically (no rule name, no organization policy URL). The request
   log and the dashboard keep both.

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

**App-level restrictions (phase 4, P4-GRANTS, D-123, SEC-67).** An app's manifest may
declare `grants.custom_only` and `grants.block_untooled` (`09 §3.4 grants`). They are
facts of the approved manifest, copied into the snapshot, and apply **below** the rules:

- `custom_only`: creating or editing a grant of a connection of that app in mode `full` is
  refused (422 `full_grant_forbidden`, `details: { app_id }`) on every path that writes grants
  (routes, CLI, first-run and template flows, agent proposals); the table's `full` row is never
  compiled for such an app. **Defence in depth** in the evaluator: for a request whose
  winning credential is a connection of a `custom_only` app, a `connection` or
  `app_connections` target **without `tools`** (whether in a derived or a hand-written rule)
  matches nothing — it neither allows nor selects the credential for injection — so a stale
  or hand-made full-style rule can never grant what the manifest forbids. Rules with
  `tools` are unaffected. The warnings panel reports such a hand-written rule as inert
  (`app_custom_only_target_inert`).
- `block_untooled`: for a request whose winning credential is a connection of that app and
  whose `tool_id` is `None`, the scope verdict is `block` before any rule is consulted
  (403 `blocked_by_policy`, `details.reason = "app_untooled_blocked"`; reason
  `app_untooled_blocked` in the trace and request log). No rule, allow or approval, can
  override it, and it applies under every grant mode and to hand-written rules alike. It
  does not apply at CONNECT time (no credential is injected there) and does not affect
  requests to the same host without that connection as the winning credential.
- **Composition.** The restrictions only tighten: they combine with grants, rules,
  approvals, governance and the Default Rule by the usual strictness order (`block` wins), never
  add an allow, and never change a custom grant's compilation (rules 1–3 above already block
  untooled requests and unlisted tools). An `ask` tool still holds for approval; a `block`
  entry still blocks. Activation of a package version that introduces `custom_only` converts
  existing full grants (`09 §10`).

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
depends on the speaker are flagged `depends_on_speaker`; phase 4, P4-GRP-1: the
selected speaker's **current groups** are resolved from the database and passed as
principals, `group` identities included, and the response lists them as
`speaker_groups`; `speaker=none` has none, and `POST …/policy/simulate` with
`speaker_user_id` does the same) returns, for every
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
  phase 2, RFE-025: `rule_applies_to_all_agents`, see *Injection selection*;
  (phase 4, P4-GRP-1) rules with `group` identities carry `group_rule_needs_speaker`
  — "matches only turns spoken by a member of <group>; never schedules, webhooks, BYO
  or boot traffic" — and, for allow rules that grant credentials,
  `group_rule_no_agent_fallback`; `group_rule_empty` (info) when a named group has no
  members; and `rule_names_removed_group` (severity `warning`, `details.group_id`,
  "this rule names a group that no longer exists and matches nobody") — the last also
  appears in the editor next to the identity chip, and publishing is never blocked by
  any of them):
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
  phase 2. (phase 5 (D-138, moved from phase 4), RFE-011b, D-133) From phase 4, and only when the operator enables
  it, a long deadline can be served by **parking** instead of holding the connection:
  *Park and resume* below. With parking off (the default) everything in this section is
  unchanged.

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
(phase 5 (D-138, moved from phase 4), RFE-011b) Nor does it apply to **parked** approvals (`parked = true`): the
agent was refused and is not blocked, so a turn that keeps running after a parked
refusal is subject to stall detection like any other.

**Lifecycle**: `pending` → `approved` | `denied` | `expired` | `abandoned`. (phase 2, RFE-006) Freezing the agent, its workspace or its org marks its `pending` approvals `abandoned` and pending action approvals `rejected` (reason `frozen`); unfreezing does not revive them. **An approval authorizes the held request only under the policy in force when it is released.** Deleting or narrowing the grant the request would use, disabling or deleting its connection or secret, and activating a snapshot under which the request would be blocked each mark the affected pending approvals `abandoned` in the same transaction (as freezing does). Independently, the gateway re-resolves the credential and re-evaluates policy after `approved`; if the request would now be blocked or has no usable credential it is refused as it would be on arrival, and a request that would still need approval is released (the approval stands). (FIX-084)
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

- **Web Push when the user has no live tab (phase 5 (D-138, moved from phase 4), RFE-011b, D-134).** Besides the
  e-mail above, a decider who registered a browser for Web Push (`§Web Push`) receives a
  push for each approval created while they have no live `approvals/stream` or
  long-poll in that workspace. It is per user and per device, independent of the e-mail
  (the e-mail remains the fallback and is **not** suppressed by a push), and never an
  approve/deny action.

### Park and resume (phase 5 (D-138, moved from phase 4), RFE-011b, D-133, SEC-74)

**Problem.** A gateway hold keeps the agent's connection open until a person decides
(`06 §Approval holds`). That is right for minutes and wrong for hours: the sandbox is
kept awake by an active turn, the connection and the turn can outlive an edge or tool
timeout, and every approval deadline is capped by `approvals.max_timeout_secs` (1–4 h)
so that no hold outlives the turn ceiling (RFE-011a). Chat-based approvers routinely
take longer. **Parking** ends the wait without losing the decision: the request is
**refused** to the agent in a way that tells it to stop waiting, the approval stays
`pending` for as long as its deadline says, and a decision becomes a **single-use grant**
for the identical request, delivered to the agent by a platform follow-up message in the
same conversation. Nothing is suspended and nothing stays in memory: the agent's turn
ends, the sandbox idles and parks like any other, and the grant is only data in Postgres.

**Opt-in.** `approvals.park.enabled` (default `false`). With it off nothing in this
section runs and phase 2 behaviour is exact.

**Which approvals park** (all must hold at the moment the gateway reaches the hold,
`06` step 8; otherwise the request is held exactly as before):

1. Parking is enabled and the **effective timeout** (*Approval deadlines*) exceeds
   `approvals.park.threshold_secs` (default 900 s). The effective timeout is computed as
   today (minimum of the timeouts set on the matching requiring rules, else
   `approvals.timeout_secs`); its **upper bound** is `approvals.park.max_timeout_secs` (default
   86 400 s, at most 604 800) for a parked approval and `approvals.max_timeout_secs` for a
   held one. Authoring (`approval_timeout_secs`) accepts up to the park maximum when parking
   is enabled; a request that **cannot** park is clamped to the hold maximum, never held
   longer.
2. The request belongs to a **turn of a hosted agent**: the identity is a hosted `context`
   credential with `conversation_id` and `turn_id`, so a follow-up can reach the agent.
   BYO, boot-token and conversation-less traffic is always held.
3. The request is **fully bufferable**: not a WebSocket upgrade or tunnel, a body no larger
   than `approvals.park.max_body_bytes` (256 KiB) that the gateway can read completely within
   `PARK_BODY_READ_TIMEOUT` (10 s). Otherwise held (and clamped as above).
4. The decision **does not depend on who is speaking**: the evaluator is run a second time
   with no speaker and must give the same verdict from the same requiring rule and winning
   credential. The resume turn is a system turn without a speaker (below), so a request that
   was approvable only because of `user`/`group` identities (RFE-005, D-131) would not be
   approvable on retry; such requests are held (or, when the hold limit is shorter than the
   deadline, clamped), never parked.
5. The agent is under its limits: at most `approvals.park.max_pending_per_agent` (20) parked
   `pending` approvals and `PARK_PER_AGENT_HOUR` (60) new parked approvals per hour, else
   403 `approval_limit_reached` (`x-should-retry: false`): a prompt-injected agent cannot
   flood the approvers' queue. (Held approvals keep their existing behaviour.)

**What is persisted** (`03 §Planned changes`, columns of `approvals`): `parked`, a salted
request `fingerprint`, `resume_state`, `resume_expires_at`, `consumed_at`, `resume_turn_id`. The
fingerprint is `SHA-256(salt ‖ canonical)` over: agent id, conversation id, method, host:port,
the normalized path **and query** (exact), the SHA-256 of the complete body, the requiring
rule's `logical_id`, the winning credential's id and the tool id. The salt is random per row
(so the stored value is not a confirmation oracle for a guessable body and cannot be matched
across rows); the raw body, headers and query values are **never** stored (the redacted
summary is, as for every approval).

**What the agent sees.** The request is **not sent** and is answered at once with
**403 `approval_parked`** (`x-should-retry: false`; `details { approval_id, expires_at,
resume_window_secs, manage_url }`) and a guidance message written for an LLM: *"This request
needs a person's approval and that may take hours. It was not sent. Do not retry it now and do
not look for another way around it. Finish what you can without it, tell the user what you are
waiting for, and end your turn. Aurora will message you in this conversation when it is
decided; if approved, repeat exactly this request once within <N> minutes."* The agent
instructions (`10 §Agent instructions`) teach the same. Repeating the identical request while
the approval is pending answers the same 403 with the same `approval_id` (no new approval, no
new notification); the repeat counts against the API's usual limits.

**Decision** (same deciders, same `can_approve` and conversation-visibility rules, same
critical audit and first-decision-wins, through dashboard, API or channel card):

- **Approve** → `status = approved` and, in the same transaction, `resume_state = ready`,
  `resume_expires_at = now() + approvals.park.resume_window_secs` (default 1 800 s); after commit
  the API queues a **system follow-up turn** in the origin conversation (the mechanism of action
  approvals, `§Action approvals` step 4: a `joining` follow-up if the origin turn is still
  running and can steer, else a new `queued` turn with `speaker_kind = system`) whose text is
  **platform-composed** — approval title, "approved by <label>", the sanitized redacted request
  line and the deadline — never model or request text beyond the sanitized summary. A frozen
  agent gets no turn (creation is refused, `04 §Freeze`) and the grant is revoked.
- **Deny** → `status = denied`; a follow-up tells the agent "denied by <label>, do not retry". A
  retry of the same fingerprint within `approvals.park.deny_cooldown_secs` (default 900 s) is
  answered 403 `manual_approval_denied` without creating a new approval.
- **Expire** → the expiry job (`05 §Background jobs`) sets `expired` at the deadline (a parked
  approval has no gateway holding it); the transcript gets `approval.expired` and a platform
  `notice`, and **no turn is started** (waking an agent hours later to say "nothing happened" is
  not worth a sandbox).
- **Resume window passes unused** → `resume_state = expired` (audit best-effort, transcript
  notice, no turn).

**Consuming the grant** (gateway, step 8). When the verdict is `needs_approval` and parking is
enabled, before creating anything the gateway looks for an open approval of the same agent and
conversation (`parked`, `resume_state = ready`, unexpired), recomputes the fingerprint with that
row's salt and, on a match, **consumes it atomically** (`UPDATE … SET resume_state = 'consumed',
consumed_at = now() WHERE id = $1 AND resume_state = 'ready' AND resume_expires_at > now()
RETURNING`, so two concurrent retries cannot both pass) and continues the pipeline as if
approved; the request-log row records `approval_id` and `resumed = true`. The grant never
overrides policy: the verdict is re-evaluated on the retry, so a rule that has since become
`block`, a different requiring rule or a different winning credential makes the fingerprint not
match, and a rule that became `allow` simply allows. Consumption happens before credential
refresh and forwarding: a request that then fails upstream or at refresh has used its approval
(fail closed; the agent gets the error and a new approval is needed). Without a match, parked
approval or hold proceeds as above.

**What revokes a grant or approval.** Freezing the agent, its workspace or its org marks its
`pending` parked approvals `abandoned` (as for holds, `§Approvals`) and its `ready` grants
`resume_state = revoked`; unfreezing revives neither. Deleting the agent or conversation deletes
the rows (the approval history survives as before, `03 §approvals`). A grant is bound to the
conversation and agent, single use, short-lived, and dies with its window.

**Sandbox lifecycle.** A parked approval is **not** a keep-awake reason (`11 §Keep-awake`): the
turn ends, the sandbox idles out after `IDLE_STOP` and parks (home volume kept) like any other.
The resume turn wakes it through the normal due-work arms, in a new boot with the harness
session restored from `harness_session_ref`; processes of the old boot are gone. This is the
point of parking: a pending approval costs a database row, not a container.

**Interaction with other rules.** RFE-011a deadlines, e-mail, cards, SSE and Web Push are
unchanged and apply to parked approvals (the e-mail and push text says "the agent has stopped
waiting and will resume when you approve"); a **reminder** is sent once, to deciders with a
push subscription or e-mail enabled and no live watcher, when half of a parked approval's time
has elapsed and it is still `pending`. Action approvals already work this way and are unaffected.
Batches (`x-aurora-batch-id`) park per request. The app-turn cap (`APP_TURN_CAP`) does not count
system turns, and each resume needs a human decision, so a retry loop is metered by people.
Limits and honesty: parking suits requests the agent can repeat **byte-identically**; a client
that puts a timestamp, nonce or idempotency token in the body will produce a different
fingerprint on retry and must be approved again (documented in the guidance and `17`).

### Web Push (phase 5 (D-138, moved from phase 4), RFE-011b, D-134, SEC-75, SEC-76)

RFE-011a notifies by dashboard SSE (an open tab) and e-mail (when *nobody* has a live
surface). Web Push (RFC 8030, 8291, 8292) reaches a **person's device** when their browser is
closed, so a decider can act without polling a mailbox. It is optional, per user, and carries
almost nothing.

**Opt-in at three levels.** (1) The operator: `push.enabled` (default `false`; `15`). (2) The user:
in the dashboard, by a deliberate gesture, grants the browser's notification permission and
registers **this browser** (`05` routes); there is no default registration and nothing is
migrated from the phase 2 in-tab notification. (3) Per device: a subscription can be paused,
given a label, set to show names, or removed. API keys cannot register a browser.

**A push is delivered only to a browser that could act on it.** A subscription is bound to the
**session** that created it (`push_subscriptions.session_id`, `03`): when the session is revoked
(sign-out, "sign out other sessions", deactivation), expires or is pruned, the subscription
stops receiving and is deleted. Deciding needs a signed-in session, so a push to a device
without one is noise at best and a leak at worst; this also fixes shared computers. Registering
an endpoint that already exists under another user **moves** it to the registering user's
current session (the browser profile that holds the endpoint is the one signed in now).

**Who receives what, and when.** On `aurora_approval created` (gateway hold, parked approval or
action approval) the API picks the users with an unpaused, unexpired subscription whose session is
live, who are **deciders of that approval at send time** (effective `can_approve` through
`aurora-db::authz`, groups included, intersected with conversation visibility exactly like the
e-mail and the SSE feed) and who have **no live watcher** (`approval_watchers`) in that workspace.
It waits `APPROVAL_PUSH_DELAY` (5 s, so a tab that is open can show its own notification first),
re-checks that the approval is still `pending` (decided, expired, frozen → nothing is sent) and
the user's authority, claims `(approval, user, stage)` in `approval_push_claims` (idempotent
across replicas) and sends one push per live subscription. Approvals sharing a `batch_id` produce
one push; each user receives at most `PUSH_PER_USER_HOUR` (30) per hour, the rest folded into one
"N approvals waiting" digest push (same `Topic`, so it replaces); a parked approval also gets the
one reminder of *Park and resume*. The e-mail path is **independent**: a user may get both, and
turning one off leaves the other (`approval_email`, subscriptions). A push is not retracted when
the approval is decided (Chrome requires every push to display something); clicking it opens the
Approvals page, which shows "already decided by X".

**What a push contains.** The payload is end-to-end encrypted to the browser (RFC 8291
`aes128gcm`) and at most 1 KiB of JSON: `{ v: 1, type: "approval.pending" | "approval.reminder" |
"approval.digest" | "test", approval_id, workspace_id, expires_at, parked, url, count?, agent?,
workspace? }` where `url` is the dashboard **path** `/o/<org>/w/<ws>/approvals?focus=<id>`.
**Never** in a push, whatever the setting: an approval title, summary field, rule name, host,
path, method, query, body or any text derived from a request or from the model, a token, a
credential, an approve/deny link, an e-mail address or a user's name. By default (`show_details =
false`) the notification reads "Aurora — an approval needs you / Open Aurora to review it"; with
`show_details = true` the service worker also shows the **agent and workspace names** (sanitized,
each ≤ 40 characters; the same information the e-mail already carries). The notification has no
action buttons: deciding needs the dashboard and its session.

**VAPID keys** (RFC 8292; ES256 on P-256). Instance-wide. Normally **generated** by the first API
replica that finds none (under an advisory lock) and stored in `webpush_vapid_keys` with the private
key **sealed** through the SecretVault (purpose `webpush.vapid`, `08 §1`) and the public key in
clear; only the API ever opens it (never the gateway, a runner, a sandbox or a response; the public
key is served to signed-in users). An operator who prefers to manage the key sets
`push.vapid.private_key_env` (the name of an environment variable holding it); Aurora then
generates and rotates nothing. **Rotation** (`aurora-api push rotate-vapid`) makes a new key
`active` and the old one `retired`; a browser subscription is tied to the application-server key it
was created with, so each subscription keeps its `vapid_key_id` and is signed for with *its* key
while the dashboard silently re-subscribes on the user's next visit (the permission persists) under
the new key and deletes the old subscription; a retired key and its secret are destroyed when no
subscription references it or after `PUSH_VAPID_RETIRE` (30 days). With an operator-managed key
rotation is a redeploy that invalidates every subscription (users re-subscribe). The VAPID subject
(`push.vapid.subject`, default the external URL) is the contact the push service may use.

**Delivery and failure.** The API sends from a bounded pool (`push.concurrency`, 16) through the
**guarded resolver and destination guard** used for every untrusted destination (public addresses
only, resolution pinned for the connection, TLS to system roots plus `tls.extra_ca_bundle`, **no
redirects**, `push.timeout_secs` 10 s), and only to an endpoint whose host matches
`push.allowed_endpoint_hosts` (validated at registration *and* before every send, `15`). Headers:
`TTL` = the approval's remaining time capped at `PUSH_TTL_MAX` (24 h) and at least 60 s, `Urgency:
high` for a new approval, `Topic` (≤ 32 URL-safe characters derived from the approval id) so a later
push for the same approval replaces the earlier. Outcomes: 201 → success; 404/410 → the subscription
is deleted; 413/400/403 → failure (a 403 after rotation means a key mismatch); 429/5xx → at most 3
tries within 60 s. Five consecutive failures set `disabled_at` and the dashboard shows "not
receiving — re-enable". A push failure never affects an approval, a decision or the e-mail. Metrics
`aurora_push_sent_total{result}`, `aurora_push_duration_seconds`, `aurora_push_subscriptions`.

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
