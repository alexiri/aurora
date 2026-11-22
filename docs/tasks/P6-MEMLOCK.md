# P6-MEMLOCK: Locked memories — a person locks a memory so the agent can read it but not change or delete it; people still edit it; export and templates carry the lock (owner, 2026-11-22, phase 6, D-463, D-464, C-1148..C-1151, SEC-151)

## Why
The owner wants memories the agent cannot change (2026-11-22: "it may be useful to have 'read-only' memories that
the agent can't modify"). Today every memory is the agent's to rewrite. `memory_save` and `memory_delete` work on any
key, the projected `memory/<key>.md` is `0644`, and the harvest uploads any edited file. A person can see from the
revision author that the agent overwrote their text, but nothing prevents it.

The brief and skills are already read-only to the agent, but they are a different thing. The brief is always shown in
full. A memory appears as one index line, plus a snippet when the incoming message matches it. A **locked** memory is
a fact a person keeps for the agent, recalled when it is relevant. A turn (or a message injected into one) cannot
rewrite or delete it.

The owner's answers (D-463):
- the lock applies **only to the agent**: people can still edit, restore, redact and delete a locked memory;
- people turn it on and off in the dashboard;
- locked memories count toward the 100-per-agent cap;
- template memories can arrive locked.

This lands **just before the agent templates** (P6-TPL-1..6), so they carry the lock from their first version.

Read first:
- `10 §Memory` and `§Search and recall`;
- `12 §Home projection`, `§Sync protocol` and `§Memory harvest`;
- `03 §memories`, `§memory_revisions`, and `§Planned changes` › Locked memories;
- `05 §Memory, skills, schedules, webhooks, links`;
- `14`: the Memory page (`§Workspace`, Behaviour › Memory; **Memory.** in `§Agent settings pages`), `§Vocabulary`,
  and §Export and import an agent;
- `API_CONTRACTS` **C-1148..C-1151** (all of them), and C-1141's `memory/<key>.md` line;
- `17` **SEC-151**;
- `90` D-463, D-464.

## Part A — API, agent tools, home sync, supervisor (Rust)

### A1. Migration and `03`
One migration (the next free number when you start; update PROGRESS's §Number allocation line) adds three columns to
`memories`, exactly as C-1148 says: `locked`, `locked_at` and `locked_by`. Move the rows from `03 §Planned changes`
into `03 §memories` in the same commit. `extract-schema.py --check` passes.

### A2. One rule in the memory service (`services/aurora-api/src/domains/memory/service.rs`)
- `save` refuses with 409 `memory_locked` when `author.kind == "agent"` and the row exists and is locked (C-1149). The
  check goes where `save` reads `existing`, after `repo::lock_agent` and inside the write's transaction, so a lock that
  commits first always wins. `set_locked` takes the same agent row lock.
- `delete` takes the `&Author` too, with the same rule. Every caller passes its author: the route passes the person,
  the `memory_delete` tool passes the agent.
- `restore` and `redact` are person-only today. Leave them so, and add a comment saying that no agent path may call
  them.
- New `set_locked(pool, agent, key, locked, by: &Author)`. It updates the three columns and bumps the agent's sync
  generation (`repo::bump_agent_sync`). It writes no revision: a lock is not a content change. Setting the value the
  row already has changes nothing and bumps nothing.
- `save` and `create` keep the lock as it is. The only exception is `create` with `locked: true` (C-1148).
- The cap (`MEMORY_MAX_PER_AGENT`) counts every row, locked or not. Nothing to change: add a test.

### A3. Routes (`domains/memory/routes.rs`, `dto.rs`)
- `Memory` gains `locked`, `locked_at` and `locked_by`. `CreateMemoryRequest` gains `locked` (C-1148).
- `PUT /v1/agents/{agent}/memories/{key}/locked` (`setMemoryLocked`). Same authority as the other memory routes
  (`visible_agent`, session or key). It writes the audit row `memory.lock` or `memory.unlock`, but only when the value
  changed.
- `05` rows and the OpenAPI entries; regenerate `web/src/lib/api/schema.d.ts`.

### A4. Agent tools (`domains/tools/memory.rs`)
- `memory_save` and `memory_delete` on a locked key return a tool error with C-1149's exact text. It is the
  `memory_locked` error passed through `fail`; check what the agent sees.
- `memory_get` returns `locked`. `memory_list` and `memory_search` items carry `locked`.
- Both descriptions get C-1149's added sentence. Update the snapshot tests that pin tool descriptions.
- No tool and no platform-tool family (including `aurora-agent call aurora …`) can reach `setMemoryLocked`, `restore`
  or `redact`. Add a registry test that lists the memory family's tools and asserts exactly the five names.

### A5. Home sync (`domains/hosted/home.rs`)
- A locked memory's entry carries `locked: true`. It is omitted when false, so an unlocked memory's entry is
  byte-identical to today's.
- `memory_file` writes `locked: true` as the last front-matter line of a locked memory.
- `index_line` appends ` (locked)` after the clamped line. The recall index and `memory_list` use the same function,
  so they stay identical. Keep the overall `MEMORY_INDEX_MAX` bound.
- `render_inputs.memory_locked = true` when the agent has at least one locked memory; otherwise absent.

### A6. Supervisor (`services/aurora-supervisor`)
- `home.rs`: `FileEntry` gains `#[serde(default)] locked: bool`. It does not deny unknown fields, so an older
  supervisor simply ignores it; keep it that way. A locked memory file is installed `0444`, other memory files `0644`
  as now. At `final`, the projector records the set of locked keys and the exact bytes it installed for each. The
  bytes are bounded by the memory limits; hold them in memory, never on disk.
- `harvest.rs`: a file whose key is in the locked set is **never uploaded**. When its checksum fails, or the file is
  missing or has been replaced, the harvester writes the recorded bytes back with mode `0444` and pushes one notice per
  changed digest. The notice is C-1149's text. This uses the `uploaded` map pattern the harvester already has.
- The API's 409 is still the authority. It covers an edit made before the lock reached the sandbox, and an older
  supervisor. That path already turns into a notice through `MemoryWriteResult` (`supervisor.rs`); check that the
  notice carries the API's message.
- `instructions.rs`: the memory capability fragment gets C-1149's sentence only when `render_inputs.memory_locked` is
  set. A sync that changes neither it nor the instructions leaves the instruction files byte-identical.
- Update `10 §Memory` (C-1148/C-1149 in prose), `12 §Home projection` (modes) and `§Memory harvest`, and `16`
  (`FileEntry.locked`, `render_inputs.memory_locked`).

### Part A acceptance criteria
- [ ] API tests (`tests/it/memory.rs`, `tests/it/tools.rs`):
  - lock, then unlock, through the route (audit row each time; none when the value is unchanged);
  - generation bumped on a change, not on a no-op;
  - no revision written by a lock;
  - a person's `PUT`, `restore`, `redact` and `DELETE` on a locked memory succeed, and `PUT` keeps it locked;
  - `memory_save` on a locked key → the C-1149 tool error, content unchanged, no revision;
  - `memory_delete` on a locked key → the same, row still there;
  - `POST /v1/runner/memory-writes` on a locked key → 409 `memory_locked`, no revision;
  - `memory_save` of a **new** key still works, and a new key at the cap is refused even when the existing 100 are all
    locked;
  - `create` with `locked: true` sets `locked_at`/`locked_by`;
  - a member of another workspace gets 404 on the lock route;
  - permission-matrix rows for the new route;
  - a race test: lock commits while a tool save waits on the agent row lock → the save is refused.
- [ ] Home-sync builder tests:
  - `locked` present only for locked entries;
  - the front-matter line;
  - ` (locked)` in `memory/index.md` and in the recall index;
  - `render_inputs.memory_locked` set and absent;
  - an agent without locked memories gets a sync byte-identical to before (snapshot).
- [ ] Supervisor tests:
  - mode `0444` for locked, `0644` for others;
  - an edited locked file is restored, not uploaded, with one notice;
  - a deleted locked file is restored;
  - a `chmod`-ed and rewritten locked file is restored with `0444`;
  - an unlocked file harvests as before;
  - unlocking (a sync without `locked`) makes the file `0644` and harvestable again;
  - the fragment sentence present only with `memory_locked`;
  - an entry without `locked` parses (old API).
- [ ] `cargo fmt --check`; clippy for `aurora-api`, `aurora-db` and `aurora-supervisor`; targeted
  `cargo nextest run` filters for memory, tools, home and harvest; `extract-schema.py --check`;
  `scripts/guards/run-all.sh`; the OpenAPI check. All of them through `heavy.sh`.

## Part B — dashboard and export (Next.js; after Part A is merged)

### B1. The Memory page (`web/src/components/memory/memory-tab.tsx`, `web/src/lib/api/memory.ts`)
C-1150 has the exact texts and placement. In short:
- `useSetMemoryLocked(agent, key)` calls `PUT …/locked` and invalidates the list and the memory.
- **List:** a locked memory shows the lock icon and a `Locked` badge after its title, using the shared `Badge`
  primitive with the icon from the shared icon set. Nothing else changes.
- **Editor:** the switch **Locked**, above Save, with the help text. It saves on its own when toggled, not with Save.
  It shows the toast "Locked" or "Unlocked". Under the switch, when the memory is locked: "Locked by <name> on <date>".
- **New memory:** the checkbox **Lock it**, unticked by default. It is sent as `locked` with the create.
- Tier: Use (no Expert gate). Both themes; no horizontal scroll at 390 px. Tokens and primitives only (`14 §Visual
  design`). If the switch or checkbox row needs a pattern the shared set lacks, add it there.
- Add **Locked** (a memory) to `14 §Vocabulary` (instead of: read-only, pinned, protected), and to the vocabulary
  test's lists if the word needs it. Update the **Memory.** paragraph of `14 §Agent settings pages`.

### B2. Export and import (`web/src/lib/export/agent-export.ts`, `agent-import.ts`)
- `ExportMemory` gains `locked`. The export writes `locked: true` in a locked memory's front matter. It writes no line
  for an unlocked one, so existing exports are unchanged.
- Import reads `locked` (a boolean; anything else is refused with the file's name, like the other front-matter errors)
  and sends it with the create. The checklist line says *3 memories (1 locked)* when any are locked.
- Update `14 §Export and import an agent` (the front-matter list).

### Tests move with the UI
Every test that sees the Memory page, the memory editor or the export format is updated in the same commit
(vitest, Playwright mocked, the export round-trip fixture). Do not skip or loosen any test.

### Part B acceptance criteria
- [ ] vitest:
  - the badge only on locked rows;
  - the switch calls the route with the right body and shows the toast;
  - "Locked by …" shown;
  - the create sends `locked` only when ticked;
  - export writes the line only for locked memories;
  - import of `locked: true` sends it;
  - import of `locked: yes` is refused with the file name;
  - the round trip keeps the lock.
- [ ] Playwright (mocked): lock and unlock a memory in the editor, and see the badge in the list. Light and dark at
  1280 px; at 390 px, the editor with the switch has no horizontal scroll.
- [ ] TypeScript check, lint, `vitest`, the production build, the touched Playwright specs. All through `heavy.sh`.

## Not in this brief
Templates carrying `locked`. Only C-1151's contract text is written now; P6-TPL-1 (parse), P6-TPL-3 (create), P6-TPL-4
(review mark) and P6-TPL-5 (Save as template) build it, and their briefs say so.

Also out of scope: locking skills or the brief (already read-only to the agent); a lock that also binds people; a
filter for locked memories.

## Security review (before merge of Part A)
A review by the Orchestrator or `sonnet-integration-reviewer` against SEC-151:
- every agent-authored write path is refused on a locked memory: tool save, tool delete and harvested file. List the
  callers of `save` and `delete` and show each one passes the real author;
- the check is inside the write's transaction, after the agent row lock;
- no agent-reachable tool or route can lock, unlock, restore or redact;
- the supervisor never uploads a locked key, restores it with `0444`, and holds its bytes only in memory;
- an older supervisor is still safe, because the API refuses.

## Ownership and sequencing
- **Owns (A):** the migration; `domains/memory` (service, routes, dto); `domains/tools/memory.rs`; the memory parts
  of `domains/hosted/home.rs`; `aurora-supervisor` `home.rs` (memory entries), `harvest.rs`, the memory fragment in
  `instructions.rs`; the `03`, `05`, `10`, `12` and `16` paragraphs; the OpenAPI entries and `schema.d.ts`.
- **Owns (B):** `web/src/components/memory/`, `web/src/lib/api/memory.ts`, `web/src/lib/export/agent-export.ts` and
  `agent-import.ts`, their tests, and the `14` paragraphs.
- Sequencing:
  - this task starts after the phase-6 all-clear, after P6-WARMPOOL Parts 1 and 2 (D-379: first in phase 6);
  - Part A, then Part B, in one lane or two;
  - P6-TPL-1 starts after Part A is merged (the template format reads the `locked` front-matter line);
  - P6-TPL-5 starts after Part B (it reuses the export's memory writer).

## Numbers
D-463, D-464, C-1148..C-1151, SEC-151. Migration: next free when placed.

## Suggested agent/model and reason
- Part A: `rust-developer` (Sonnet). One column set, one route, one rule in the memory service, and the projection
  and harvest change, along paths the memory feature already has.
- Part B: `nextjs-developer` (Sonnet). One switch, one badge, one checkbox, and the export field.
