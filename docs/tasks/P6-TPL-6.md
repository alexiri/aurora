# P6-TPL-6: Agent templates — helper files on an agent: storage, the `helpers` home-sync part and the `.aurora/helpers/` managed root, the instructions line, list/read/remove routes (owner, 2026-11-22, phase 6, D-456(e), D-457(d), C-1143, C-1113, SEC-146)

## Why
The owner wants templates to bring scripts and helper tools (D-456(e)). Scripts that one skill uses are already that
skill's files. Files the instructions use directly come in the template's `helpers/` folder (C-1141). This brief gives
an agent a place for them. They are stored with the agent, projected **read-only** into its computer at
`.aurora/helpers/`, and the instructions say where they are. People can list, read and remove them. P6-TPL-3 writes
them when an agent is created from a template, and when an update's helpers part is applied.

Read first:
- `P6-TEMPLATES.md` §Helper files;
- `API_CONTRACTS` **C-1143** (all of it), C-1141's `helpers/` rules, and C-903 (the content-safety headers you reuse);
- `12 §Home projection` and `§Sync protocol`: the managed roots, the `apps` part kind P5-GUIDE-2 added, which is the
  closest precedent (find its commit and follow it), pruning at final, and generations;
- `16` `HomeSyncPart`;
- `24 §7`: how `.aurora/apps/` is validated and installed;
- `17` SEC-117 (the `.aurora/apps/` managed-root invariant) and SEC-146.

## Scope

### 1. Migration and `03`
One migration (next free number when you start; update PROGRESS's §Number allocation line) for `agent_helper_files`,
exactly as C-1143 says. Move it out of `03 §Planned changes` in the same commit. `extract-schema.py --check` passes.

### 2. The writer and the routes (`services/aurora-api`)
- A new module `domains/agent_helpers`, with `replace_all(tx, agent_id, files)`. In the caller's transaction it:
  - validates paths with the crate's C-1141 helper rules (call `aurora_template`; do not copy them);
  - enforces the per-agent count and size caps;
  - computes `sha256`;
  - bumps the agent's home-sync generation the way a skill edit does (find it).
  It takes `&mut` of the repo's transaction type, because P6-TPL-3 calls it inside one transaction. Add
  `delete(tx, agent_id, path: Option<&str>)` on the same rules.
- The three routes of C-1143, with `can_use` on the agent's workspace (session or key, as the agent's other
  read/write routes), and `getAgentHelper` with the C-903 headers. Reuse the files feature's header helper (find it in
  `domains/files`). Audit `agent.helpers_remove`.
- No agent tool may write or delete helpers. The tool catalogue is unchanged.

### 3. Home sync (API side)
In the home-sync builder, add the `helpers` part kind between `apps` and `previews`. Its entries are the agent's helper
files at `.aurora/helpers/<path>`, in the skills file-entry shape (content encoding and executable bit exactly as
skills carry them). Add them to the `manifest` part's managed paths, and add
`render_inputs.helpers = { dir: ".aurora/helpers", count }` when the agent has any. Parts are split under
`HOME_SYNC_PART_MAX` as the other kinds are.

### 4. Supervisor (`services/aurora-supervisor`)
- Accept the `helpers` kind. `.aurora/helpers/` is a **managed root**:
  - an entry is accepted only below it, with every segment matching the C-1141 name rules (no `..`, no leading `.`,
    at most 4 segments below `helpers/`);
  - a refused entry is a warning, never a write elsewhere.
- Install files `0555` when executable and `0444` otherwise, in `0555` directories.
- Prune the root to the manifest at `final`, as for `.aurora/apps/`.
- The capability fragment gets the C-1143 sentence when `render_inputs.helpers` is set. A sync that changes only
  helper files, with the same count, leaves the instruction files byte-identical (so no idle session is released).
- An older API, which sends no `helpers` part and no `render_inputs.helpers`, renders exactly as before.
- Update `12 §Home projection` (the list of managed roots) and `§Sync protocol`, and `16` (`HomeSyncPart` kinds).

## Not in this brief
Creating agents from templates and applying updates, which call `replace_all` (P6-TPL-3); the screens that list and
remove helpers (P6-TPL-4).

## Security review (before merge)
A short review by the Orchestrator or `sonnet-integration-reviewer`:
- the managed root cannot be escaped (traversal is unrepresentable, and a hostile path from a crafted API message is
  refused);
- modes are as stated;
- pruning removes deleted helpers;
- helpers are never writable by the agent;
- the routes' authority;
- `getAgentHelper` serves bytes with the C-903 headers (never rendered inline).

## Acceptance criteria
- [ ] API tests:
  - `replace_all` inside a transaction that is then rolled back leaves nothing;
  - the caps are enforced;
  - list, read (headers included) and delete one or all;
  - the audit row is written;
  - the generation is bumped on every change;
  - permission-matrix rows for the three routes;
  - a member of another workspace gets 404.
- [ ] Home-sync builder tests: the `helpers` part (entries, executable bits, manifest paths), `render_inputs.helpers`
  set and absent, and splitting across parts.
- [ ] Supervisor tests:
  - files installed with `0555`/`0444` and directories `0555`;
  - a path outside the root, or with `..`, refused;
  - pruning after a helper is removed;
  - the instruction files byte-identical when only helper contents change;
  - the sentence present only with helpers;
  - an unknown kind still skipped by the old code path (keep that test).
- [ ] The hosted e2e smoke, if the repo has a cheap one for skills projection: a helper appears at
  `.aurora/helpers/x.sh` with mode `0555`, and disappears after delete. Otherwise, say in the report that it was not
  run.
- [ ] `cargo fmt --check`, clippy for `aurora-api` and `aurora-supervisor`, targeted
  `cargo nextest run -p aurora-api -p aurora-supervisor` filters, `extract-schema.py --check`,
  `scripts/guards/run-all.sh`, and the OpenAPI check, all through `heavy.sh`.

## Ownership and sequencing
- **Owns:** the migration; `domains/agent_helpers`; the `helpers` part in the home-sync builder; the supervisor's
  managed-root handling for `.aurora/helpers/`; the `12` and `16` paragraphs; the three routes' rows in `05` (agent
  section); the OpenAPI entries and `schema.d.ts` (regenerated).
- Starts after P6-TPL-1 is merged (it calls the crate's path rules and uses the `TEMPLATE_HELPERS_*` constants). It
  can run beside P6-TPL-2 in a second Rust lane: disjoint files. If both regenerate `schema.d.ts`, the second to merge
  regenerates again.
- P6-TPL-3 starts after this and P6-TPL-2 have merged.

## Numbers
D-456(e), D-457(d), C-1143, C-1113, SEC-146. Migration: next free when placed.

## Suggested agent/model and reason
`rust-developer` (Sonnet). It adds one table and three routes, and extends the home projection on both sides along a
path P5-GUIDE-2 already cut for `.aurora/apps/`.
