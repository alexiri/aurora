# P5-MEMORY-1: Memory backend: where a memory came from, when it was used, readable revisions, safe search snippets, and save events in the chat (owner, 2026-11-27)

## Goal

The Skills & Memory review (D-496) found that nothing says who wrote a memory or whether the agent uses it (M-3),
History shows e-mails and raw sources (M-5), search snippets show raw `<b>` tags (M-1, a bug), and when the agent saves
a memory the chat says only "Used 1 tool · Deny … done" (M-6). The web pages that fix this (P5-MEMORY-2, -3, -4) need
data the API does not have today. This brief adds it: **server and API only**, no page changes beyond keeping the
current pages working.

Decisions: **D-496** (the review is the target), **D-497** (provenance, "used", and conversation names only for people
who may read them). Contracts: **C-1169, C-1170, C-1171, C-1172** (`docs/API_CONTRACTS.md` §P5-MEMORY and P5-SKILLS).
Security: **SEC-157** (add its row to `docs/17-security.md`: a conversation reference never names a conversation the
caller may not read; memory events are readable only by the conversation's readers).

## Linked requirements

- **Design package (read first):** `docs/design/skills-memory/README.md`, then `review.md` §Memory (M-1, M-3, M-5,
  M-6; proposals mem-1, mem-3, mem-4, mem-5). Look at `after/mem-1.png` (the list's source lines "Added by you · 2
  days ago", "Learned by Daneel in “Release prep” · today", and the reading view's line "Added by Bob Builder on 25 Nov ·
  Changed twice · Daneel last used it in “Weekly report” yesterday"), `after/mem-4.png` (the timeline's names and
  contexts), `after/mem-5.png` (the chat card's data: title, one line of content, the refusal reason and what the agent
  wanted to add). Before: `before/M01-memory-list.png`, `before/M05-memory-history.png`, `before/M06-memory-search.png`,
  `before/C01-chat-memory-saved.png`.
- Contracts C-1169..C-1172; decisions D-497; `docs/10-agents-conversations.md` §Memory (line ~883) and §Search and
  recall (~946); `docs/03-data-model.md` §7 `memories` (~1191) and `memory_revisions` (~1207); `docs/05` route table
  (~888); `docs/16-wire-protocols.md` for transcript events.

## Wave and order

First of the memory set; P5-MEMORY-2, -3 and -4 start when it is merged. Can run in parallel with P5-SKILLS-1 (other
domain; both add migrations — yours is `20261201000288`, theirs `000289`/`000290`; if a guard demands ascending order at
merge time, the second to merge renumbers). **Security review before merge** (SEC-157), by the coordinator.

## Context (what exists today)

- Routes `services/aurora-api/src/domains/memory/routes.rs`, DTOs `dto.rs`, rules `service.rs` (`Author` ~17, `Source`
  ~38, `recall_snippets`), repo `crates/aurora-db/src/repo/memory.rs`, tests `services/aurora-api/tests/it/memory.rs`.
  Migrations `20261105000007_memory_skills.sql`, `20261201000277_memory_locks.sql`.
- `Memory` has `key, title, description, content, index_line, revision, updated_at, created_at, locked, locked_at,
  locked_by` — **no** creator or changer. Revisions store `author_kind` (`user`|`agent`), `author_user_id`,
  `author_label` (= the **e-mail**), and `source` jsonb (`dashboard`, `api`, `tool` with `conversation_id`/`turn_id`,
  `harvest`, `template`, `restore` with `restored_from`); the DTO drops the ids. An API key's id is not recorded. 50
  revisions are kept (`MEMORY_REVISIONS_KEPT`), so revision 1 may be gone.
- Nothing records agent reads (turn-start recall, `memory_get`/`memory_list`/`memory_search` in
  `services/aurora-api/src/domains/tools/memory.rs`).
- Search: `repo/memory.rs:~345` uses `ts_headline` with the default `<b>…</b>` markers; the web shows them as text. The
  conversation search already solved this: sentinels and `snippet()` returning plain text plus UTF-16 ranges in
  `crates/aurora-db/src/repo/search.rs` (~30, ~654, ~729). The same markers reach the agent through `memory_search` and
  the recall.
- Tools: `memory_save` returns `{"key","revision","created"}`; a locked refusal becomes a bare `ToolError::Failed`
  (`registry.rs:~72`), the `memory_locked` code is lost (`mcp_bridge.rs:40-60`). The transcript shows the harness's
  tool name (`mcp__aurora__memory_save`, `aurora__memory_save`, `aurora_memory_save`) with `summary` "(N args)".
  Server-originated transcript events already exist (approval events); find how they are appended and ordered and use
  the same path.

## Scope

1. **Provenance on the memory (C-1169).** Add what is needed to `memories` (migration `20261201000288`): the adder and
   the last changer as `MemoryActor` (person id; API key id; agent id with conversation id; template name and version;
   import) — columns or a jsonb, your call, documented in `03`. Write them on every create/save/restore/template/import
   path. **Back-fill** existing rows from the oldest and newest kept revision; unknown stays `kind: "unknown"`.
   Return `added_by`, `changed_by` on `Memory` (list and get).
2. **Last use (C-1169, D-497(b)).** `last_used_at` and `last_used_conversation_id` on `memories`, written by
   `memory_get`, by the turn-start recall for each memory it includes, and by `memory_search` only for a hit the agent
   then reads with `memory_get` (so: recall and `memory_get`); at most one write per memory, conversation and hour
   (a conditional `UPDATE`; never a write per turn when nothing changed). Dashboard and API reads never count. Return
   `last_used`.
3. **Conversation references (D-497(c), SEC-157).** One helper that turns a conversation id into `ConversationRef`
   `{id, title}` **only if the caller may read that conversation** (the same check the conversation routes use, for
   people, API keys and emergency readers), else `null`. Used by `added_by`/`changed_by`/`last_used` here and by
   P5-SKILLS-1's usage (put it where both domains can call it). Prove with tests: Bob's agent learned a memory in Bob's
   direct chat; Carol, a co-owner who cannot read Bob's chat, sees `conversation: null`; an emergency reader sees what
   the emergency rules allow.
4. **Revisions (C-1170).** `MemoryRevision` gains `author` (`MemoryActor`), `via`, `restored_from`; `author_label` becomes
   the display name (never an e-mail). Record `author_api_key_id` on new revisions written through an API key.
5. **Search (C-1171).** Use the sentinel pattern of `repo/search.rs`: `snippet` plain, `highlights` UTF-16 ranges, and
   `title_highlights` for a title or key match. The tool `memory_search` and the recall get plain text (no markers, no
   sentinels). Add an IT test with `"rotation"` against the review's on-call memory: no `<`, correct ranges.
6. **Save events (C-1172).** After `memory_save` saves or is refused, append `memory.saved` / `memory.refused` to the
   turn's transcript (fields in C-1172; `preview` = the content's first 200 characters on one line; `message` in plain
   words, e.g. "“Jira project keys” is locked, so <Agent> can't change it."). `memory_save`'s result gains `title`; a
   refusal keeps its code in the tool error text. Works for every harness (the event is written by the API, not the
   adapter). Events go to the conversation's readers only (the transcript's rules).
7. **Regenerate the client types:** `web/openapi.json` through the foundation test's `UPDATE_OPENAPI=1` mode
   (`services/aurora-api/tests/it/foundation.rs:~545`, run via `scripts/dev/test.sh`), then `cd web && pnpm gen:api`.
   Keep the current Memory page compiling and its tests green (it may ignore the new fields; P5-MEMORY-2 uses them).

## Out of scope

Any page or chat UI change (P5-MEMORY-2..4); opening search or revisions to emergency readers (unchanged; the
read-only page hides them); skills (P5-SKILLS-1).

## Constraints / file ownership

- Owns `domains/memory/**`, `domains/tools/memory.rs`, `repo/memory.rs`, the new migration, the new
  conversation-reference helper, and the transcript-append call for the two events. Touch `repo/search.rs` only to
  share its `snippet()` helper.
- Agent-facing text (tool results, refusal messages, recall) states facts in plain words and never cites docs, D-, C- or
  FIX- ids (D-483).
- Work in your own worktree; `.env` `DATABASE_URL` never points at port 55432; no co-author lines in commits.

## Acceptance criteria

- `GET …/memories` returns `added_by`, `changed_by`, `last_used` per C-1169 for the seeded data (`docs/design/skills-memory/seed.py`):
  memories added through Bob's API key show `kind: "api_key"` with the key's name and Bob; a memory saved by the agent
  through `memory_save` shows `kind: "agent"` with the conversation for readers and `null` for others.
- `last_used` is set by `memory_get` and recall and not by dashboard reads; a second `memory_get` in the same hour and
  conversation does not write (proved by a test).
- Revisions carry names and `via`, never an e-mail; API-key revisions carry the key.
- `GET …/memories/search?q=rotation` returns plain `snippet` and correct `highlights`; the agent's tool output has no
  `<b>`.
- A `memory_save` that saves appends `memory.saved`; one refused by a lock appends `memory.refused` with
  `code: "memory_locked"`; both appear in the conversation's transcript API in order after the tool call.
- `web/openapi.json` and `schema.d.ts` regenerated; CI's drift check passes.

## Tests

IT tests in `services/aurora-api/tests/it/memory.rs` (provenance per path, back-fill, last-use throttle, revisions,
search highlights, the two events, the conversation-reference privacy matrix), repo unit tests, a migration test for
the back-fill. Database tests take their database from `aurora_testkit::TestDb` (`TestDb::with_pool()`).

## Tests move with the UI (owner, 2026-11-20 — binding)

1. Before each commit that changes an API field, an error code or message, a tool result, a route or visible text:
   `rg -n "<old text>|<old field>" web/tests web/e2e scripts/e2e services crates tests` and update every hit **in the
   same commit**: `web/tests/**` (vitest), `web/e2e/**` (mocked **and** production projects, `support/mock-api.ts`,
   fixtures), `scripts/e2e/*.sh` (e.g. `upgrade.sh` reads memories, `llm.sh`/`llm-smoke.sh` the memory tool),
   `tests/e2e-hosted/**` (`llm.rs` asserts the saved row), Rust tests asserting messages or shapes, `web/openapi.json`.
2. New expectations use the new words and fields exactly; never loosen an assertion to accept old or new; never skip,
   `fixme` or delete a test to get green (a test goes only with its feature, named in the report).
3. Run tests **only** through `scripts/dev/test.sh` (D-491): `scripts/dev/test.sh <lane> rust -p <crate>` runs the
   **whole** test set of every crate you touched or whose generated schema you changed (aurora-api, aurora-db, …);
   `scripts/dev/test.sh <lane> web vitest <files>` for web tests you touched. Never `cargo test` by hand, never your
   own Postgres, never port 55432. Other heavy commands (cargo build/check/clippy, pnpm build) through
   `scripts/dev/heavy.sh <lane> <command>`.
4. Production specs that need a real stack: update by reading; list them in the report.
5. A merge that turns master's CI red because a test expects the old shape goes back to this lane.

## Docs to update

`docs/API_CONTRACTS.md` C-1169..C-1172 (complete in place with the final names), `05` (route/DTO notes), `03` §7 (the
new columns), `10` §Memory (provenance, "used", the events), `16` (the two transcript events), `17` (SEC-157 row),
PROGRESS and FEATURES rows at every commit.

## Suggested agent/model and reason

Sonnet `rust-developer` (multi-file Rust, a migration with back-fill, a privacy rule); the coordinator reviews SEC-157
on Opus before merge.

## Completion report

Built 2026-11-27 on `alexiri/p5-memory` over master `d96d979f` (one commit, "P5-MEMORY-1: …"). Not pushed. Master
has since gained P5-SKILLS-2 (web) but not P5-SKILLS-1, so **this branch adds the shared conversation-name helper**
(`services/aurora-api/src/domains/conversations/reference.rs`, `conversation_refs(state, caller, ids)` →
`ConversationRef`); P5-SKILLS-1 should reuse it. Expect a `PROGRESS.md`/`FEATURES.md` merge by hand.

### Final field names

- `Memory.added_by`, `Memory.changed_by`: `MemoryActor { kind: MemoryActorKind (person|api_key|agent|template|
  import|unknown), person: {id, display_name}|null, api_key: {id, name}|null, agent: {id, name}|null,
  conversation: ConversationRef|null, template: {name, version}|null }`.
- `Memory.last_used`: `MemoryLastUsed { at, conversation: ConversationRef|null }|null`.
- `ConversationRef { id, title: string|null }` (null title: an untitled conversation the caller may read).
- `MemoryRevision`: `author: MemoryActor`, `via: MemoryVia`, `restored_from: int|null`; `author_label` is a name
  (derived at read time); `provenance` and `author_kind` stay.
- `MemorySearchHit`: `highlights`, `title_highlights` (offsets into `title`, or `key` when there is no title).
- Events `memory.saved { turn_id, key, title, preview, revision, created, previous_revision }`,
  `memory.refused { turn_id, key, title, preview, code, message }`. `memory_save` result: `{ key, title, revision,
  created }`. Refusals read `tool_failed: memory_locked: …` / `memory_limit_reached: …` (`memory_delete` too, same
  code path).

### Migration `20261201000288` and back-fill

`memories` gains `added_by`, `changed_by` (jsonb, ids only), `last_used_at`, `last_used_conversation_id`,
`last_used_marks`; `memory_revisions` gains `author_api_key_id`. Back-fill: `added_by` from revision 1 **only while
it is kept** (when it was pruned, the oldest kept revision is not the adder, so it stays `unknown`), `changed_by`
from the newest revision; agent (+ conversation for tool writes), template, API (`api_key` without a key id),
person, else `unknown`. A temporary SQL function does it and is dropped. No stored row is rewritten otherwise
(older e-mail labels stay in the table and are never returned).

### Decisions taken inside the brief (please check)

1. **"A co-owner who cannot read Bob's chat".** By the access law the conversation routes use (RFE-009), an own-right
   workspace **owner can** read another person's direct chat, so the helper names it to one, as the brief's rule
   ("the same check the conversation routes use") requires. The "may not read" case is proved with a workspace
   **member** (and their API key); an organization owner without own access gets 403 on the memory routes until an
   emergency grant.
2. **Throttle.** "At most once per memory, conversation and hour" needs more than the last conversation: two
   conversations used in turn would write at each switch. `last_used_marks` keeps the hour's marks, and one
   conditional `UPDATE` checks them. So within the hour `last_used` can name the other, later-recorded conversation.
3. **Recall** is recorded after the delivery transaction commits (best effort), so it takes no lock inside the
   delivery and a rolled-back delivery records nothing.
4. **`import`** stays in the enums but no server path produces it: the dashboard's import uses the ordinary create
   route, which cannot tell. Marking it would need a new request field (not in C-1169).
5. **Events.** A `memory_save` whose arguments do not fit the tool schema gets no event (nothing reliable to show);
   a server error gets none; an evaluation turn's read-only refusal gets `not_allowed`. The event is appended in its
   own transaction right after the write, best effort. It is **not** strictly ordered against the harness's own
   `tool.started` report, which travels through the runner's report queue while the call travels through
   `/tool-calls` (approval events have the same property); in practice `tool.started` is posted first. P5-MEMORY-4
   should not depend on strict adjacency.
6. **Snippets** are plain text of at most 800 characters (`MEMORY_SNIPPET_CHARS`, so the recall is not shortened to
   the message search's 300). A headline starts at the first match, as before.
7. **The refused preview** is the first 200 characters of the whole content the agent sent: `memory_save` replaces
   the content, so the server has no "added line" (mem-5 shows only that line).
8. An evaluation turn's `memory_get` and recall count as uses like any other (D-497(b) names no exception).

### Comparison with the mockups (server and API only: no page or chat change)

Looked at `after/mem-1`, `-3`, `-4`, `-5`, dark and light, before coding and again before handing over. No screen was
built, so there is no 1280/390 px capture to compare. Data check:

| Mockup text | Field |
|---|---|
| "Added by you · 2 days ago" (also memories made through Bob's key) | `added_by.person.id` (set for `person` and `api_key`), `created_at` |
| "Learned by Daneel in “Release prep” · today" | `added_by.kind = agent`, `.agent.name`, `.conversation.title` |
| "Added by Bob Builder on 25 Nov · Changed twice · Daneel last used it in “Weekly report” yesterday" | `added_by.person`, `created_at`, `revision - 1`, `last_used` |
| mem-4 "changed it · through an API key", "Daneel changed it … in “Release prep”", "created it" | revision `author`, `via`, revision 1 |
| mem-3 highlighted "Rotation" in the snippet and title | `highlights`, `title_highlights` |
| mem-5 card title and line; refusal sentence and wanted text | event `title`, `preview`, `message`, `code` |

Differences for the page lanes: decision 7 above; a readable but untitled conversation has `title: null` (the page
needs its own fallback); `revision - 1` counts restores as changes and not locks.

## Security notes (SEC-157)

| Item | How | Test (`services/aurora-api/tests/it/memory.rs` unless said) |
|---|---|---|
| A conversation is named (id and title) only to its readers, on list, get and revisions | one helper, the routes' own check per row: `Caller::workspace_read` (key confinement) + `can_see_conversation` | `a_conversation_is_named_only_to_its_readers`: the chat's person and a workspace owner see it, a member does not (no id, no title anywhere in the body), their API keys follow them; each caller is compared with `GET /v1/conversations/{id}` |
| Emergency reader: session only, and recorded | a `ReadAuth::Emergency` naming calls `record_emergency_view` first (fails the request with 503 if it cannot) | same test: 403 before the grant, name after it, exactly one `conversation.emergency_view` row; the opener's `auo_` key gets 403. The 503 path is `record_emergency_view`'s own, not re-tested here |
| Deleted or unknown conversation | absent from the map → null | same test (deleted) |
| Stored actors hold ids only; no e-mail leaves | names resolved at read; stored label of a person is the display name; a name equal to the address shows its local part | `revisions_carry_names_and_via_never_an_email` (a planted e-mail label is never returned) |
| Events only to the conversation's readers; redaction clears them | ordinary `transcript_events`; only `turn_id` is structural | `memory_events_follow_the_tool_call_and_the_transcript_rules`: a member gets 404 on another person's direct chat; after the turn redaction neither the content nor the key remains |
| The agent never receives conversation names or people | tool results unchanged apart from `title` and plain snippets | `memory_tools_via_tool_calls`, `search_snippets_are_plain_text_with_ranges` |
| Content cannot forge highlight markers | the two sentinels are removed from stored text in SQL before `ts_headline` | `search_snippets_are_plain_text_with_ranges` (planted `U+0001`/`U+0002`) |
| People's reads never count as use | only `memory_get` and the delivered recall call `mark_used` | `last_use_is_written_by_the_agent_only_and_at_most_hourly`, `the_turn_start_recall_records_a_use` |
| Lock refusal unchanged (SEC-151) | the one check in `save_in_tx`; only the tool's error text gains the code | `locked_memory_routes_and_the_agent_refusal` (existing, message updated), `a_lock_committing_first_wins_over_a_waiting_agent_write` |

For the reviewer: (a) an own-right owner reading another's direct chat title writes no `conversation.owner_view`
(the conversation list names it the same way without one); (b) an event's `preview` stays in the transcript after the
memory is deleted or its revision redacted; redacting the transcript event or turn removes it; (c) `memory.refused`
shows the existing memory's title to the conversation's readers, who can already read the agent's memories.

### Test files changed

- `services/aurora-api/tests/it/memory.rs`: 8 new tests (provenance per path; revisions; the privacy matrix; use
  throttle; search ranges; events; event order, readers and redaction; recall use). Changed for the new shapes:
  `recall_composition` reads `Recalled.snippets`; the tool refusal expects `memory_locked: …`; one `Author` gains
  `api_key_id`.
- `services/aurora-api/tests/it/agent_templates.rs`: asserts a template memory's `added_by`/`changed_by`.
- `crates/aurora-db/tests/it/memory_provenance_migration.rs` (new, the back-fill);
  `repo/memory.rs` unit tests for the plain-text snippet and substring ranges.
- Web mocks given the new required fields: `tests/memory-skills.test.tsx`, `tests/memory-one-box.test.tsx`,
  `tests/agent-export.test.ts`, `e2e/agent-tabs.spec.ts`. No assertion changed.
- Read, no change needed (they create or read content only): `scripts/e2e/upgrade.sh`, `llm.sh`, `llm-smoke.sh`,
  `tests/e2e-hosted/tests/llm.rs`. No production Playwright spec reads memory shapes. None of these were run.

### Commands and results

- Fail first: the new tests on master's server code with the migration removed (`memory::`, `--no-fail-fast`;
  the one `mark_used` call stubbed in that throwaway copy): **8 new failed, 10 old passed** (no `memory.saved`,
  `<b>` in the snippet and recall, null provenance and use, missing columns). The migration test was not run
  without the migration (its first assertion is that the migration exists).
- `test.sh rust -p aurora-api`: `memory::` + `openapi_matches_checked_in` +
  `agent_templates::a_full_creation_writes_every_kind…` **20/20**; `conversation_lifecycle`, `chat_search`,
  `home_checksum`, `apps_tools`, `route_inventory` + the template test **37/37**; `hosted::`,
  `emergency_access::` **42/42**. `web/openapi.json` written with `UPDATE_OPENAPI=1`, then checked without it.
- `test.sh rust -p aurora-db` (memory, search, schema, migration order): see the last line of this report.
- `pnpm gen:api`; `pnpm typecheck` and eslint on the touched files: clean. `test.sh web vitest` (3 files) **44/44**;
  `test.sh web playwright e2e/agent-tabs.spec.ts --project=mocked` **13/13** (includes the Memory page at 390 px).
  No page changed, so no new Playwright or axe run was added.
- `cargo fmt --all --check` 0; `scripts/guards/run-all.sh` passed; `extract-schema.py --check` up to date.
- Not run: whole-crate suites, clippy (CI's), the e2e shell scripts and `tests/e2e-hosted`.
- `aurora-db` run: **16/16**, among them
  `memory_provenance_migration::the_backfill_reads_the_oldest_and_newest_kept_revision`, the two `repo::memory`
  unit tests, `repo::search`'s, `schema_matches_spec` and `migration_order`.

### Open questions

Decisions 1 and 4 above are the two the owner or reviewer may want to change.

### SEC-157 review and the master merge (2026-11-27)

The review accepted the design and decisions 1 (an owner reads a direct chat's title by the access law, with no
owner-view row) and 4 (`import` is in the schema only). Master was then merged in:

- **FIX-262:** the memory list and get record an emergency reader's read (`record_read`, `ReadKind::Memory`, the
  memory id on get) before the body, as on master. **Revisions and search still need own access** (a grant alone
  gets 403, as `route_inventory` requires; not opened, per "Out of scope"). A caller with own access **and** an
  active grant reads them under the grant too, so both now record the read (`record_own_read`: revisions with the
  memory id, search as a list read; 503 `audit_unavailable` if it cannot be written, now in OpenAPI).
- **FIX-270:** the shared `aurora_core::memory_index::index_line` (`<key> (locked): <line>`) is used by the DTO and
  `memory_list`.
- New test `an_emergency_readers_revisions_and_search_are_recorded` (grant alone: 403, no row; own access and grant:
  one `workspace.emergency_read` per route; a member: none). Without the two calls it fails with no row.
- After the merge: `memory::`, `emergency_access::`, `route_inventory::` and `openapi_matches_checked_in`
  **58/58**; `web/openapi.json` and `schema.d.ts` regenerated; `pnpm typecheck` clean (after removing stale route
  types in the ignored `web/.next*` build output). No Playwright, vitest or `aurora-db` re-run after the merge.
