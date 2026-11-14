# P5-CHAT-4: Message search — API and chat UI — **medium–large, phase 5, Rust then Next.js**

## Goal
The owner (2026-11-14): "we're probably going to need that search API too, right? add it as another phase 5
task." Today the conversation list filters loaded titles on the client and nothing searches message text
(`14` "Agent chat": "there is no search API"). Decisions D-229, D-230; contracts C-727 (route), C-728 (agent
tools).

People search what was said in the conversations they can see: words, phrases, files, narrowed by agent,
conversation, author and date, with results that open the message in place (P5-CHAT-2 permalinks).

## Contract (normative: `docs/API_CONTRACTS.md` C-727 — read it whole)
`GET /v1/workspaces/{ws}/messages/search` — query `q`, optional `agent_id`, `conversation_id`, `author`
(`me` | `agent` | `others`), `has` (`file`), `after`, `before`, `archived`, `sort` (`recent` default |
`relevance`), `limit` (≤ 50, default 20), `cursor`. Results carry plain-text snippets with highlight
ranges (never HTML). Visibility is part of the query (`05 §Conversation visibility`: "applies the same
predicate inside the query, never as a post-filter").

## Part A — API (Rust, `rust-developer`)
1. **Index table** (new migration; next free number from PROGRESS): `transcript_search(conversation_id,
   seq, workspace_id, agent_id, author_kind, speaker_user_id null, has_file bool, at timestamptz, body text,
   doc tsvector)`, pk `(conversation_id, seq)`, FK to `transcript_events` **on delete cascade**; GIN index on
   `doc`; btree `(workspace_id, at desc)`, `(agent_id, at desc)`. `doc = setweight(to_tsvector(<workspace
   memory language>, body), 'A') || setweight(to_tsvector('simple', body), 'B')` — stemmed matches in the
   workspace's language plus exact words in any language; no Postgres extension (the chart's least-privilege
   roles cannot create one). **No rewrite or long lock of `transcript_events`.**
2. **What is indexed**: durable `turn.queued` (`text`), `text` (agent messages; the D-227 Markdown reduced
   to plain text: code kept, link text kept, URLs kept), `attachment` (`filename` + `caption`), and the
   filenames of a person's attachments on `turn.queued`. Not indexed: tool events, approvals, notices,
   errors, thinking, deltas. `body` ≤ 32 KiB (the rest of a longer message is not searchable; say so in
   `10`).
3. **Write path**: the row is inserted in the same transaction as the transcript event (repo function used
   by every insert path of those types; grep them all and list them in the commit).
4. **Removal**: redaction deletes the row in the redaction transaction; conversation delete and retention
   purge cascade; an `attachment_purged` file keeps its filename row (the name was said in the conversation)
   unless the event itself is redacted.
5. **Backfill**: a resumable background job (due-work arm or a one-shot task — follow how the repo runs
   background work) indexes existing events in batches of ≤ 1 000 by `(conversation_id, seq)` order, with a
   persisted cursor; the route answers with `index_complete: false` while it runs. `aurora-api` admin CLI or
   a config key is **not** needed — it starts on deploy and finishes on its own; log progress.
6. **Query**: parse `q` (≤ 200 chars, ≤ 12 terms) into a tsquery ourselves — words (prefix match on the last
   word), `"phrases"`, `-excluded`, `OR`; never pass user text to `to_tsquery` raw. Predicate: workspace,
   the visibility law (`can_see_conversation` as SQL, the same expression the conversation list uses), the
   filters, `doc @@ query`. `sort=recent`: `at desc, seq desc` keyset cursor; `relevance`:
   `ts_rank_cd` then `at`. Snippet: `ts_headline` with sentinel markers converted to `highlights` ranges
   server-side (text returned plain; no markup survives).
7. **Limits and rate**: read class; one query ≤ 2 s statement timeout (bounded by indexes); `422
   search_query_invalid` for an empty or over-long `q`.
8. **Spec**: `05` (route, errors), `10` (§Search: what is indexed, removal rules, language), `03` (table),
   `14` line "there is no search API" removed, `17`/security coverage (visibility inside the query, redacted
   text never returned), OpenAPI + `schema.d.ts`.

## Part C — Agent tools (Rust, `rust-developer`, after Part A; may share Part A's lane)
The owner: "shouldn't the agent be able to search its own conversations? that would improve memory"
(D-230; contract C-728 — read it whole).
1. Two `PlatformTool` registry entries, family "Conversations": `conversation_search` and
   `conversation_read`, on Part A's index and query builder (one query module, two callers: the route and
   the tools).
2. **Scope predicate** (C-728): current conversation; the speaker's own direct conversation with this agent;
   the agent's non-direct conversations the speaker can see; never another person's direct conversation,
   even for an owner speaker; speakerless turns get the current conversation and non-direct ones. Built
   from the call's stamped agent and turn (`12 §Platform tools`: identity is attached downstream, never from
   arguments).
3. Results wrapped as untrusted quoted data (C-728 wording); descriptions repeat the `10` preamble rule;
   bounds as C-728.
4. Spec: `12 §Phase 0 catalogue` gets the "Conversations" family; `10` (§Search: the agent's scope, why
   owners' wider dashboard view does not extend to the agent); `17` row (agent search cannot cross direct
   conversations).
Acceptance (in addition to the list below):
- [ ] IT scope matrix: speaker A (member) finds A's direct + non-direct, not B's direct; speaker O (owner)
      also does **not** find B's direct; schedule turn finds non-direct only (+ current); another agent's
      conversations never; a redacted message never; `conversation_read` on an out-of-scope id → `not_found`
      identical to an unknown id.
- [ ] IT: tool result carries the untrusted wrapper; ≤ 64 KiB; `index_complete` passes through.
- [ ] Access change: speaker loses `can_use` → the next call no longer returns that workspace's non-direct
      conversations.

## Part B — Web (Next.js, `nextjs-developer`, after Part A and P5-AGENT-2, P5-CHAT-2)
Designed on the P5-CHAT-0 canvas as a **Search** board (added to P5-CHAT-0's list).
1. The conversation-list filter becomes a search box: typing filters titles as today; **Enter** (or "Search
   messages for …" as the first row) runs a message search and the list pane shows results.
2. Search syntax the box understands and turns into parameters: `from:me` / `from:agent`, `in:<conversation
   title>` (picked from a suggestion list), `after:`/`before:`/`on:` dates, `has:file`; the rest is `q`.
   Chips show the active filters; each removable.
3. A result row: conversation title and kind tile, author, time, snippet with highlights (`mark`, tokens),
   "archived" chip when relevant. Click/Enter opens `?c=&m=` (P5-CHAT-2 jump + highlight). Infinite scroll
   on the cursor. `index_complete: false` → a quiet line "Older messages are still being indexed".
4. Scope switch: **This agent** (default, `agent_id`) / **All agents** (workspace).
5. Empty, error and loading states per `14`; 390 px; keyboard: `/` or Ctrl/Cmd+F inside the chat focuses the
   box (Ctrl/Cmd+F only when focus is in the chat panel).

## Owns
Part A: new migration, `crates/aurora-db/src/repo/{transcript,search}.rs`, the transcript insert paths,
redaction/retention/delete paths, new `services/aurora-api/src/domains/conversations/search.rs` (+ route
wiring), the backfill job, `web/openapi.json`, `schema.d.ts`, docs `03`/`05`/`10`/`17`. Part B:
`components/chat/conversation-list.tsx` (after P5-AGENT-2 releases it), new `components/chat/search/**`,
`lib/api/search.ts`, tests. Part C: new tool registry entries under the API's tool registry
(`domains/tools/**` or wherever `memory_search` lives), docs `12`/`10`/`17`. Part A not parallel with P5-CHAT-1 (same domain and migration range; run after
it); Part B not parallel with P5-CHAT-2/3 (same panel).

## Acceptance criteria
- [ ] IT visibility: a member never finds another member's direct conversation; an owner does (as the list
      shows); a hidden conversation's text never appears in `snippet`; a `conversation_id` the caller cannot
      see → 404 like every `conversation=` filter.
- [ ] IT removal: redact → gone from results in the same transaction; delete conversation → gone;
      retention purge → gone.
- [ ] IT query: words, prefix on the last word, phrase, `-exclude`, `OR`, author/has/date filters, both sorts,
      cursor stability across inserts, stemmed match in the workspace language and exact match in another
      language; hostile `q` (tsquery operators, backslashes, 10 000 chars) → handled or 422, never a 500.
- [ ] IT backfill: resumes after a restart from its cursor; `index_complete` flips; inserts during the
      backfill are not double-indexed.
- [ ] Perf note (run only when the machine is idle): 1 M indexed messages, p95 of a two-word query in one
      workspace < 300 ms; record the number or "not yet measured".
- [ ] Web: unit tests for the syntax parser and result row; Playwright: search → open result → message
      highlighted; scope switch; 390 px; axe clean in both themes.
- [ ] `cargo fmt --check`, clippy, targeted ITs; `pnpm typecheck`, `lint`, `test`, `build`.

## Suggested agent/model and reason
Opus reviews the visibility predicate and the tsquery builder (security); Sonnet `rust-developer` for Part A,
Sonnet `nextjs-developer` for Part B.
