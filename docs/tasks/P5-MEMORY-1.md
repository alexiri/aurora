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

Commits; the final field names; the migration and back-fill behaviour; the privacy matrix tested; test files changed
and why; commands run with results; open questions.
