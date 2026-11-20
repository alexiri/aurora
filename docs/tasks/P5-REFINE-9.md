# P5-REFINE-9: Agent timeline backend, live: what an agent can do, is doing, did (owner, 2026-11-20)

## Goal

UX review 5 (recommendation 4, the owner's D-415) found that "what has my agent done today?" is answered in **six places
with six vocabularies**, and not at all for the agent's computer:

- the summary sentences on Welcome and Access (`web/src/lib/summary/agent.ts`);
- "What exactly?" per account row;
- the chat's folded "Used N tools" (`components/chat/tool-call-group.tsx`);
- the rail's Recent actions, with the apology "thinking isn't listed";
- Approvals › History;
- Activity › Requests, a nine-column table of `POST litellm…svc.cluster.local/v1/messages · default_denied`;
- schedule Runs that say "fired" and webhook Deliveries that say `rejected_signature · 31 B`.

Nothing shows what the agent did **on its computer**: the commands it ran and the files it wrote are invisible unless a
person opens Terminal or Files (review blind spot, theme "Nothing shows what the agent did on its computer").

The owner decided (D-415): **one live timeline per agent**, "what it can do, is doing, did", in sentences. It also feeds
the status rail and a Technical view. It is **pushed over a stream**, not polled: a new row appears at the top as soon
as the agent acts; an in-progress row ("Running “Morning summary”…", "Waiting for you") turns into its outcome **in
place**; the Technical view streams new requests the same way. The sentences are written by the web from `lib/labels.ts`;
the API returns **structured kinds and names**.

This is the backend brief: the feed, the stream, the privacy rules and the typed client. **No UI**: P5-REFINE-11 (the
agent rail's "What <Agent> did"), P5-REFINE-16 (the workspace Activity page, Technical view) and the agent's Settings ›
Apps & access ("What <Agent> can do") draw it.

## Linked requirements

- **Artifact (source of truth, owner's comments applied):** <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt>
  - recommendation 4 (<https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt#above>, `UXR5/layer-above.md` rec 4);
  - theme "Nothing shows what the agent did on its computer" and blind spot "What the agent did on its computer"
    (`UXR5/priorities-themes.md`);
  - `#f-workspace-2` (Activity as sentences a person reads).
- **Local reference folder (read-only):** `/home/alex/orca/workspaces/aurora/design-ux-review-5/` (`UXR5/`).
  - **workspace-2** (the row sentences, the filters in words, the raw request under Details):
    `UXR5/after/workspace-2.png`, `UXR5/after/workspace-2-lt.png`, `UXR5/after/src/workspace-2.html`,
    `UXR5/before/workspace-2-b1.png`; screen `UXR5/screens/B154-activity.png`.
    The mockup's three rows are the **shapes this feed must support**: "Dani answered you · Used Workspace default · 1 call",
    "Dani called GitHub · Blocked by your rule 'Block evil.example.org'", "Dani asked before changing docs.example.org ·
    Waiting for you in Approvals".
  - **Rec 4's own sketch** (`UXR5/layer-above.md` rec 4): rows such as "Answered you in Direct chat · thought with Claude ·
    3 tool calls", "Asked you before sending to Ops alerts → you allowed it once", "Ran “Morning summary” (schedule) ·
    answered · 2 min", "Ran `ls` and wrote notes.txt on its computer", "Was blocked by your rule …".
  - Other screens that show the six places today: `UXR5/screens/B091-access.png`, `UXR5/screens/B037-rail-computer.png`,
    `UXR5/screens/C010-schedule-runs.png`, `UXR5/screens/C025-webhook-deliveries.png`, `UXR5/screens/C058-chat-answer.png`,
    `UXR5/screens/B152-approvals.png`.
- **Decisions:** D-415 (this brief), D-409 (per-run threads; the privacy of direct chats is unchanged), D-383 (only real
  calls count), D-405 (precedence). Existing: D-230 (a direct chat is private to its person), RFE-009.
- **Contracts:** **C-1083** (the feed) and **C-1084** (the stream), drafted in
  `docs/API_CONTRACTS.md` › “Phase 5 — P5-REFINE” (merged with the briefs, 2026-11-20).
  They are already in `docs/API_CONTRACTS.md`; correct them there first if the code needs a different shape.
- **Security:** **SEC-143** (assigned by the coordinator, 2026-11-20; the row is in `docs/17-security.md`): the timeline
  shows a person only what they could already see. Add its coverage row to `docs/tests/security-coverage.md`; the test
  cases are listed under Tests. This brief needs a security review before merge.
- **Normative docs:** `10 §Transcript events` (the events this feed reads), `10 §Turns`, `06` (request log), `05` (route
  rows), `14 §8 What an agent can do, in one place` (the summary sentences) and `§5 Approvals` (the wording).

## Wave and order

- **Wave 2, lane E, after P5-REFINE-7** (it adds the per-run threads and the run and delivery outcome words this feed
  reads: C-1073..C-1079) and after P5-REFINE-4.
- Lane E: 4 → 7 → **9**.
- Unblocks: P5-REFINE-11 (rail "What <Agent> did", live header), P5-REFINE-16 (Activity), P5-REFINE-19 (Technical view
  shortcuts).
- `web/openapi.json` and `web/src/lib/api/schema.d.ts` are regenerated here.
- **No other lane edits `crates/aurora-db/src/bus.rs` or the gateway listener's channel list while this runs** (new
  channel).

## Context

- **Request log:**
  - table `request_log` (`crates/aurora-db/migrations/20261105000004_policy.sql:121`, partitioned; index
    `request_log_workspace_ts_idx (workspace_id, ts DESC)`);
  - read `crates/aurora-db/src/repo/request_log.rs:57` `list_activity` (keyset on `(ts, id)`; it already omits rows of
    a direct conversation the viewer cannot see);
  - route `services/aurora-api/src/domains/activity/routes.rs` (`RequestLogEntry` :17, `activity` :105, filters
    `agent_id`, `conversation_id`, `decision`, `host`, `app_id`);
  - gateway writer `services/aurora-gateway/src/reqlog.rs` (`DbWriter` :24, batches every 5 s or 500 rows) calling
    `crates/aurora-db/src/repo/gateway.rs:501` `insert_request_log`.
  - Decisions in the table: `allow`, `blocked`, `default_denied`, `egress_denied`, `governance_refused`, `rate_limited`,
    `approval_denied`, `approval_expired`, `approved`, `guard_refused`, `error`. `llm_model` is set for model requests (C-880).
- **Transcript events:** `crates/aurora-db/src/repo/transcript.rs` (`append_durable_event` :39 does the seq bump, the
  insert and the `aurora_conversation` NOTIFY `{c, s}`; `list_after` :87; `tool_events_of_turn` :106). Event vocabulary
  in `docs/10-agents-conversations.md:348` (`turn.queued/started/finished`, `tool.started { call_id, name, summary }`,
  `tool.finished { call_id, ok, summary? }`, `approval.pending/decided/expired`, `error`). The tool `summary` is
  sanitized and capped at 200 characters and **never raw arguments**. Adapter summaries:
  `services/aurora-supervisor/src/adapters/claude_code/stream.rs:264` (`summarize(name, input)`, e.g. `Bash: false`),
  `adapters/pi/stream.rs:72` (`tool_summary`: `bash: <command>`, `read: <path>`). Read the other adapters
  (`codex`, `goose`, `opencode`) for their tool names.
- **Approvals:** tables `approvals` (gateway holds) and `action_approvals` (`20261105000004_policy.sql:86`, `:158`);
  `domains/approvals/{routes,service,bus,enrich}.rs`. The approvals stream `GET /v1/approvals/stream`
  (`routes.rs:78`) and its bus (`domains/approvals/bus.rs`, `AppState.approval_bus` `app.rs:81`) are the **pattern to copy**
  for a bus-fed SSE stream.
- **Schedule runs, webhook deliveries:** tables `schedule_runs`, `webhook_deliveries`
  (`migrations/20261105000008_schedules_webhooks.sql:33,66`); inserts `crates/aurora-db/src/repo/schedules.rs:360` and
  `repo/webhooks.rs:309`; domains `domains/schedules/fire.rs`, `domains/webhooks/receive.rs`. P5-REFINE-7 adds the outcome
  words and the per-run conversation id; **read its merged contract (C-1073..C-1079) for the exact field names** and use them.
- **Peer tasks, access requests:** `domains/peer_tasks/` (table `peer_tasks`, `GET /v1/agents/{agent}/peer-tasks`),
  `domains/access_requests/` (`AccessRequest`, statuses `open|given|dismissed|expired`).
- **Conversation visibility:** `crates/aurora-db/src/authz.rs:504` `can_see_conversation(auth, user, kind, direct_user_id)`
  (a `direct` conversation: its person, or `can_own`); SQL twin at :518.
- **The conversation stream** (`domains/conversations/routes.rs:496`, `Stream_` :430ff, `RESYNC_EVERY`) and the bus
  (`conversations/bus.rs`): how a stream re-authorizes (`crate::http::auth::revoked`), coalesces and resyncs.
- **Bus channels:** `crates/aurora-db/src/bus.rs:20` (`Channel`, `ALL` :37, `as_str`, `parse`); the gateway listens to a
  fixed list in `services/aurora-gateway/src/invalidation.rs:298`.
- **Web today (read, do not change here):** `components/agents/status-rail/status-data.ts` (`useAgentStatus`, recent calls
  from the request log, `actionsOnly` :64), `parts.tsx` `RecentCalls` :188, `components/approvals/activity-view.tsx`,
  `lib/api/approvals.ts:140` (an `EventSource` hook to copy), `lib/chat/use-transcript.ts` (the chat's stream client).

## Scope

1. **Contracts first.** Land C-1083 and C-1084 (and the SEC row, once numbered) in the docs. Where the drafted shapes need
   to change after you read the code, change the draft in the doc and say so in the completion report.
2. **The `computer` classification (finding: theme "Nothing shows what the agent did on its computer").**
   - One module `domains/timeline/computer.rs` owns the table that says which harness tool names are *shell* and which
     are *file writes* per adapter (`claude-code`: `Bash`, `Write`, `Edit`, `MultiEdit`, `NotebookEdit`; `pi`: `bash`,
     `write`, `edit`; read `codex`, `goose` and `opencode` and add theirs). A read-only tool (`Read`, `Glob`, `Grep`,
     `read`) is **not** a `computer` row. Platform tools (`mcp__aurora__*` and the like) are not either.
   - `command` is the first line of the summary after `<Name>: `, at most 120 characters; `files` come from a write
     tool's summary path (at most 5 per row). A summary that does not parse yields `command: null`, `files: null` and the
     row still shows ("ran something on its computer").
   - A test per adapter fixture proves the classification; an unknown tool name is *not* classified (never a guess).
3. **The feed `GET /v1/agents/{agent}/timeline` (C-1083; rec 4, workspace-2).**
   - Sources, each read newest first with the keyset and `limit + 1`, then merged by `(at, id)` in Rust:
     `turns` (+ their `tool.*` events and the turn's request-log model name), `request_log`, `approvals` and
     `action_approvals`, `schedule_runs`, `webhook_deliveries`, `peer_tasks`, `access_requests`.
   - **Kinds** exactly as in C-1083 (`answered`, `computer`, `asked_before`, `called`, `ran_schedule`, `received_webhook`,
     `handed_task`, `got_task`, `asked_for_access`) with the outcomes and fields listed there. States: `running`,
     `waiting`, `done`, `failed`, `blocked`, `skipped`.
   - **Folding** rules of C-1083: model requests (`llm_model` set) never a row of their own (they become the `model` of
     the turn's `answered` row); consecutive gateway rows of one turn and app/host fold into one `called` row with
     `count`; at most 3 separate `computer` rows per turn, the rest fold into `count`.
   - **Names, not sentences.** `model` is the **display name** of the AI model (the same display name rule as the
     web's model labels: use the endpoint's `display_name` when known, else the model id; never the endpoint host).
     `app.name` from the plugin snapshot (`apps::find_app`), `rule.name` from the denormalized `rule_name`.
   - **Privacy (the rule of C-1083, SEC-143).** A row that comes from a conversation is returned only when
     `can_see_conversation` holds for the caller (another person's `direct` conversation: omitted entirely, `can_own`
     sees it). `command`, `files` and `host` are returned only with a visible conversation. A `called` row of a
     conversation the caller cannot see is omitted, **never** reduced to a count. Schedule runs, webhook deliveries and
     peer tasks are workspace-visible to `can_use`; their conversation `link` is omitted when the run's conversation is a
     direct one the caller cannot see (it is not, since D-409's per-run threads are `schedule`/`webhook` kinds; assert it).
     No e-mail, secret, header, credential-bearing path or request body ever appears in a row.
   - **Bounds.** Every source query is bounded by agent and time; add the index each one needs if `EXPLAIN` shows a scan
     (report them). `limit` 1..100. The merged page is stable under concurrent inserts (keyset, no offsets).
   - BYO agents: only request log, approvals, schedule runs, webhook deliveries and access requests (no transcript
     events); the route still answers.
4. **The stream `GET /v1/agents/{agent}/timeline/stream` (C-1084; D-415 "pushed, not polled").**
   - **New database channel `aurora_timeline`**: `Channel::Timeline` in `crates/aurora-db/src/bus.rs` (add to `ALL`,
     `as_str`, `parse`), and to the gateway listener's list if it enumerates channels. Payload `{ a, s, i, q? }` (hint
     only, no text, ≤ 200 bytes).
   - **Writers send the hint after the change commits**:
     - API: `schedule_runs`, `webhook_deliveries` (repo functions above), `peer_tasks` open/close, approval create/decide
       (the approval service already notifies `aurora_approval`; add the timeline hint beside it), access request
       create/resolve; in the same transaction, as `append_durable_event` does for `aurora_conversation`;
     - gateway: after each request-log flush, one hint per agent present in the batch (`reqlog.rs`/`status.rs`); it
       needs no new grant (NOTIFY needs none);
     - turn events: the stream also listens to `aurora_conversation` hints `{c, s}`, maps the conversation to its agent
       (cache the map for the stream's life) and reloads the turn's row.
   - **The stream** (copy `approvals/routes.rs` `stream` and `domains/approvals/bus.rs`): session only; `can_use`;
     `revoked()` ends it; keep-alive from `s.sse_keepalive`. On a hint it loads the item(s) by id, applies the privacy
     rules, and emits `event: row` `{ op: "upsert", item }`. With `?technical=true` it also emits `event: request` with
     the `RequestLogEntry` of each new log row of the agent. `event: resync` on a lagged receiver, a reconnected
     listener and every 30 s as a heartbeat check.
   - **In place:** the same `id` is reused for the same row's later states (`turn:<id>` goes `running` → `done`,
     `approval:<id>` goes `waiting` → `done`, `run:<id>` goes `running` → `done`).
   - **Limits (C-1084):** `TIMELINE_STREAMS_PER_USER = 5` (add to `aurora-core::limits` and `02 §Limits`; 429
     `rate_limited` beyond), coalescing 500 ms per row, queue 200 then `resync`.
   - Metrics `aurora_timeline_streams` (gauge) and `aurora_timeline_events_total{kind}`.
5. **Typed web client (no UI).** In `web/src/lib/api/`:
   - `timeline.ts`: `useAgentTimeline(agentId, filters)` (`useInfiniteQuery` on `next_cursor`) and `useTimelineStream(agentId,
     { technical })`, which opens an `EventSource` (copy `lib/api/approvals.ts:140`), merges `row` upserts into the query
     cache by `id` (a new `id` goes to the top; the same `id` replaces the row in place), counts rows that arrive while
     the list is scrolled (`pendingNew`) so the UI can say "3 new · Show", and refetches on `resync`;
   - `TimelineItem`, `TimelineKind` types from the generated schema;
   - `pnpm gen:api` regenerates `openapi.json` and `schema.d.ts`; the existing UI keeps compiling and behaving the same.
6. **Permission matrix and coverage.** Add both routes to `services/aurora-api/tests/it/permission_matrix.rs`. Add the
   SEC row's coverage entry in `docs/tests/security-coverage.md` once numbered.

## Out of scope

- All UI: the rail's "What <Agent> did", the Activity page, the Technical view, the "3 new · Show" pill, the sentences
  (P5-REFINE-11, -16; sentences in `lib/labels.ts` from P5-REFINE-2).
- Changing the request log, the approvals tables, the transcript events or the supervisor adapters (this brief reads
  them; if an adapter's summary is too poor to classify, report it, do not change the adapter).
- A per-workspace or per-organization stream (the workspace Activity page can merge the agents' feeds or read the
  existing activity route; only the per-agent stream is built).
- Retention of what the feed reads (unchanged).

## Constraints / file ownership

- **Owns:** a new `services/aurora-api/src/domains/timeline/{mod,routes,dto,feed,stream,computer}.rs`; the new repo
  queries in `crates/aurora-db/src/repo/timeline.rs`; `crates/aurora-db/src/bus.rs` (channel); the hint writes in
  `crates/aurora-db/src/repo/{schedules,webhooks}.rs` and the gateway's `reqlog.rs`/`status.rs` (one line each);
  `web/src/lib/api/timeline.ts`, `web/openapi.json`, `web/src/lib/api/schema.d.ts`.
- Do not change `domains/activity/routes.rs` (the Technical view still uses it). Do not edit `web/src/components/**`.
- Shared files: `app.rs` (register the router), `permission_matrix.rs`, `docs/tests/security-coverage.md`: append only,
  and check with the Orchestrator that no other lane touches them now.

## Acceptance criteria

1. `GET /v1/agents/{agent}/timeline` returns, for a dev fixture agent with one answered turn, one blocked request, one
   held-then-allowed approval, one schedule run and one computer step, five rows of the right kinds, newest first, each
   with structured names and no sentence. The three rows of `UXR5/after/workspace-2.png` can be written from the fields
   alone ("answered you" + model name + `count`; "called GitHub" + rule name; "asked before changing <host>" + `waiting`).
2. The `computer` row for `Bash: ls` and a write of `notes.txt` carries `command: "ls"`, `files: ["notes.txt"]` and a
   `link` to the file; a `Read` produces no row; an unknown tool name produces no row.
3. **Privacy:** Alice (not an owner) never receives rows, commands, hosts or counts that come from Bob's direct chat; the
   workspace owner does; schedule and webhook rows reach every `can_use` caller.
4. **Live:** with the stream open, a gateway request, a turn's `tool.started`, an approval request and its decision, and
   a schedule run each arrive as a `row` event within 2 s; the approval's row **keeps its `id`** and goes from `waiting` to
   `done` in place; `?technical=true` adds `request` events and without it none arrive; revoking access ends the stream;
   the 6th stream of a person is 429.
5. Paging: 3 pages of 50 over 150 mixed rows give every row exactly once with a concurrent insert in between.
6. The web client merges an upsert by id into the cache (vitest), counts rows that arrive while the list is not at the top,
   and refetches on `resync`.
7. Gates: `cargo fmt --check`; clippy `-D warnings` for aurora-api, aurora-db, aurora-gateway; targeted `cargo test`
   (below); `scripts/guards/run-all.sh`; `python3 scripts/extract-schema.py --check` (no schema change expected; run it);
   OpenAPI check and `pnpm gen:api` with no diff; web `pnpm typecheck` and the vitest file of the client. Report the counts.
   GitHub CI is the full gate.

## Tests

Targeted only. Heavy commands through `/home/alex/orca/workspaces/aurora/heavy.sh <lane> <command>`. Never run perf
budgets while other lanes run (record "not yet measured").

- **API integration** (`services/aurora-api/tests/it/timeline.rs`, new):
  - each kind from a fixture (a turn with tool events, request-log rows, an approval, a schedule run, a webhook delivery,
    a peer task, an access request);
  - folding (model rows never their own; 5 gateway rows of one app in one turn give one `called` with `count: 5`);
  - privacy cases (another person's direct chat omitted for a member, visible to the owner; `command`, `host`, `files`
    absent without a visible conversation; no e-mail or secret in any field: scan the JSON);
  - paging stability; filters `kind`, `outcome`; BYO agent; 404 for an agent the caller cannot see;
  - the stream: events for each hint type, in-place update by `id`, `technical` on and off, `resync`, revoked access ends
    it, the per-user limit;
  - `permission_matrix.rs` rows.
- **Unit:** the computer classifier (fixtures per adapter), the command and file extraction, the folding function, the
  hint payload size.
- **Gateway:** the request-log flush sends one hint per agent (`services/aurora-gateway` test beside `reqlog.rs`).
- **Web:** `web/src/lib/api/timeline.test.ts` (merge by id, new row to the top, `pendingNew`, refetch on `resync`).

## Docs to update

- `docs/API_CONTRACTS.md` › "Phase 5 — P5-REFINE": C-1083, C-1084.
- `docs/05-control-plane-api.md`: two route rows; `docs/10-agents-conversations.md` (a short § "The agent timeline" under
  Transcript events: which events feed it, the computer classification, the privacy rule).
- `docs/02-engineering-conventions.md` §Limits (`TIMELINE_STREAMS_PER_USER`), `docs/15` metrics list if one exists.
- `docs/17-security.md` and `docs/tests/security-coverage.md` once the SEC number exists.
- `docs/14-web-dashboard.md` §8 ("What an agent can do, in one place"): note the feed is the source of "did"; the screen
  is P5-REFINE-11/16.
- `docs/PROGRESS.md` and `docs/FEATURES.md` rows for P5-REFINE-9.

## Suggested agent/model and reason

Sonnet (`rust-developer`). A union feed over eight sources, a new bus channel and a stream with a privacy rule. Hard to
verify without the integration tests listed; not Haiku.

## Completion report

(Fill in when done; P5-REFINE-20 reads this.)

- **Items done** (scope items 1–6, with commit ids):
- **Contract text changed from the draft, and why:**
- **Deviations from this brief, and why:** (for example any source that could not be read cheaply)
- **Computer classification:** the tool-name table per adapter, and any adapter whose summary was too poor to parse:
- **Indexes added or needed** (with the `EXPLAIN` result):
- **SEC number used for the visibility rule:**
- **Performance:** p50/p95 of a 50-row page; events per second the stream sustained in the test:
- **Screenshots compared:** none (backend brief); attach the three rows of `UXR5/after/workspace-2.png` as JSON from the
  API:
- **Gate counts:**
- **Open questions for the owner:**
