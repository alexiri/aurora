# P6-TPL-3: Agent templates — create an agent from a template in one transaction (with library skills and helper files), provenance, update offers, apply and detach (owner, 2026-11-21 and 2026-11-22, phase 6, D-435, D-436, D-456, D-457, C-1111, C-1112, C-1113, C-1142, C-1143, SEC-146)

## Why
A person picks a template, fills its inputs, chooses an account and access for each app, and chooses which optional
library skills to add. Then they create the agent. The agent is theirs to change. Newer approved versions are offered
part by part, and the agent stays exactly as it is until the person applies one (`docs/tasks/P6-TEMPLATES.md`). This
brief builds those routes.

Read first:
- `P6-TEMPLATES.md`;
- `API_CONTRACTS` **C-1111, C-1112** (all of them), C-1113, **C-1142** (adding library references) and **C-1143** (the
  helper writer);
- C-1110 (the plan the person saw); C-504, C-980, C-957 (grants, levels, limits); C-1061/D-414 (starter grants on a
  new agent); C-572 and `10 §Git skill library` (the agent-tier `from_library` add);
- `10 §Agents`, `§Skills`, `§Memory`, `§Schedules`, `§Webhooks`; `04 §Fire-time authority`;
- `17` SEC-146, SEC-56, SEC-67, SEC-83.

## Scope

### 1. Migration and `03`
One migration (next free number when you start; update PROGRESS's §Number allocation line) for the `agents` columns
and `agent_template_parts` of C-1112 (with its four kinds). Move them out of `03 §Planned changes` in the same commit.
`extract-schema.py --check` passes.

### 2. `POST /v1/workspaces/{ws}/agents/from-template` (C-1111)
- **All checks first**, in C-1111's order, collecting field errors where the contract says so.
  - Use `aurora_template::check_inputs` and `render`.
  - Use P6-TPL-2's functions for the offered version, availability, levels and dropped tools, and
    `resolve_skill_refs` for skill references. One function decides both what the plan showed and what create
    accepts, and a test asserts that they agree.
- **Library skills: fetch before, write inside.** For each chosen `library` reference, call the library add path's
  fetch-and-validate step at the given `commit_sha` before the transaction opens. Network IO never runs inside it.
  Inside the transaction, call its write step at the agent tier. If the add path is one function that does both,
  split it into those two steps without changing its behaviour (its tests stay green), and say so in the report.
- **One transaction through the existing writers.** Find and call the functions behind:
  - agent creation, including the starter-grant step for a new hosted agent;
  - skill create;
  - the library add path's write step;
  - `agent_helpers::replace_all` (C-1143, P6-TPL-6);
  - memory save;
  - schedule create;
  - webhook create;
  - **the one grant writer** (`via: "template"`).
  Where a writer opens its own transaction, refactor it to take one (`&mut PgConnection`, or the repo's convention)
  rather than duplicating it. No rule is re-implemented here. The webhook secrets are returned once in the 201, as the
  webhook create route returns them.
- Provenance: the four `agents` columns, and an `agent_template_parts` row for each of these:
  - the instructions and each own skill, with the C-1104 part hash of what was written (after rendering and text
    cleaning);
  - the helpers (`helpers_hash`), when there are any;
  - each library reference taken (`skill_ref_key`).
- `Agent.template` on every route that returns an `Agent`.
- Audit: `agent.create` with `template: "<id>@<version>"`; the grant writer's and the library add's own rows.

### 3. Updates (C-1112)
- **`GET /v1/agents/{agent}/template`.** Compare the agent's current parts with the recorded hashes, and with the
  newer offered version rendered with the stored inputs, using the crate's part hashes. The current parts are: its
  instructions, its agent-tier skills that are not library-sourced, its helper files, and its library skills matched
  to references. Produce exactly the part states of C-1112, for all four kinds. A library skill's content is never
  compared.
- **`POST …/template/apply`.**
  - It changes the listed parts only, in one transaction (library fetches before it), through the same writers:
    brief update; skill create, replace or delete; `replace_all` for helpers; library add or delete for references.
  - It writes new parts rows.
  - `template_version` moves to the applied version, also with no parts ("skip").
  - Errors are as C-1112.
  - The agent's home sync follows exactly as for a person's brief, skill or helper edit. Find how those bump it and do
    the same.
- **`DELETE …/template`**: detach. Helpers and library skills stay.
- **Nothing changes an agent without apply.** Approving, withdrawing or deleting versions, sources or templates never
  writes to `agents`, its skills, its helpers or its parts. A test asserts this for each of those admin actions.

### 4. OpenAPI, `05` and types
Add the routes and the `Agent.template` field to the OpenAPI description and to `05`'s route tables (the workspace
and agent sections), and regenerate `schema.d.ts`.

## Not in this brief
Sources, ingest, catalogue and skill-reference resolution (P6-TPL-2); the helper table, projection and routes
(P6-TPL-6); screens (P6-TPL-4); Save as template (P6-TPL-5).

## Security review (before merge)
A separate reviewer reviews against SEC-146:
- Nothing from the template becomes access without the person's request: levels are capped by the limit and
  `custom_only`, accounts pass C-504, tools are filtered.
- Library skills are added only from a library that C-1142's resolution accepts, through the library add path with
  its budgets, pinned and read-only; the route never creates or changes a library.
- The route never writes rules, secrets, channels, presences, resource overrides or BYO settings.
- It is all-or-nothing on failure, the library skill and helpers included.
- No network IO runs inside the transaction.
- Schedule fire-time authority is the caller's, and webhooks are paused.
- Input values cannot inject placeholders.
- Apply changes only the parts the person listed, bound to the version's `content_sha256`.
- No admin action changes an agent.

## Acceptance criteria
- [ ] API tests for a full creation from the service-monitor example: the agent; its skills; the library skill (from
  a fixture library added to the organization, pinned at the commit, read-only); the helper files; memories;
  schedules on and off as chosen; a paused webhook with its secret shown once; grants with `via: "template"` and their
  audit rows; starter grants; provenance rows for all four kinds.
- [ ] Each refusal of C-1111:
  - a stale version → 409 with the plan;
  - a missing required account; an app not available; an account of another app; an inactive account; a C-504
    refusal of an account the caller may not give;
  - `above_account_limit` for an organization account; `full_grant_forbidden`;
  - bad input; `max_agents`; a handle taken; rendered instructions too long;
  - a missing required library reference; a `library_id` the resolution does not accept; `library_skill_invalid` at
    the commit; `library_budget_exceeded`; a missing required organization skill.
- [ ] **Rollback**: make the last write fail (for example a unique violation on the last schedule), and assert that
  no agent, skill, library skill, helper, memory, schedule, webhook or grant remains.
- [ ] Dropped tools are reported in `notes`; a custom choice with no tool left gives no access; an optional
  reference left out is reported as `skill_skipped`.
- [ ] Schedules created from a template fire with the creator's authority: a creator who loses access disables them,
  as for any schedule.
- [ ] Updates:
  - each part state for each kind (`unchanged_here`, `edited_here`, `removed_here`, `new`, `removed_upstream`);
  - applying a subset changes only those parts; edited parts are untouched unless listed;
  - helpers replaced as one part;
  - a new library reference added with its `skill_refs` item, and refused without it;
  - a removed reference deletes the library skill only while it is still library-sourced (a detached one is kept,
    `skill_kept`);
  - a needed input (missing → 422, given → stored); stale hash → 409; `template_part_unknown`; a library skill of the
    same name → 409;
  - skip moves the version; detach stops offers;
  - a source deleted and added again finds the agent by template id.
- [ ] **The agent is unchanged** after a newer version is approved, after withdraw, and after the source is deleted
  (assert on its brief, skills, helpers and parts).
- [ ] Permission-matrix rows for the four routes, and the audit rows.
- [ ] `cargo fmt --check`, clippy for `aurora-api`, targeted `cargo nextest run -p aurora-api` filters,
  `extract-schema.py --check`, `scripts/guards/run-all.sh`, and the OpenAPI check, all through `heavy.sh`. CI is the
  full gate.

## Ownership and sequencing
- **Owns:** the new migration; the from-template, provenance and update modules in `services/aurora-api`; the
  transaction-taking refactors of the writers you call, including the library add path's split into fetch and write
  (behaviour unchanged, their tests green); `03`; OpenAPI and `schema.d.ts` (regenerated).
- Starts after P6-TPL-2 and P6-TPL-6 are merged (same lane as P6-TPL-2). Tell the Orchestrator which writer modules
  you refactor, so no other lane edits them at the same time.

## Numbers
D-435, D-436, D-456, D-457, C-1111, C-1112, C-1113, C-1142 (adding), C-1143 (writer), SEC-146. Migration: next free
when placed.

## Suggested agent/model and reason
`rust-developer` (Sonnet). It is one transactional route over many existing writers, plus a diff and hash state
machine over four part kinds. The delicate parts are refactoring writers to share a transaction, and splitting the
library add into fetch and write. Escalate to Opus if a writer cannot take a transaction without changing its
behaviour.
