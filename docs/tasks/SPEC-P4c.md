# SPEC-P4c: Normative spec for port previews and the git skill library

## Goal
Last phase-4 spec batch (D-138 moved SSH and evals to phase 5). Write the **normative spec** for port previews (P4-PRV-1, P4-PRV-2) and the git-backed skill library (P4-GSL), so implementation can be briefed from it. Specification only; no code. Same shape and rules as `docs/tasks/SPEC-P4a.md` and `docs/tasks/SPEC-P4b.md` (read both and how their items landed: `git log --oneline --grep 'SPEC-P4[ab]'`).

Items (one commit each, prefix `SPEC-P4c: <item>`):
1. **P4-PRV-1 — preview shares.** `21 §Phase 4 — Port previews`: supervisor port discovery already reports `ports.state`; shares per (agent, port) with a stable hostname under `previews.domain`; visibility `private` (workspace members after sign-in; specify whether any other visibility exists, defaulting to none); who may create/delete a share (permission matrix); wake on a preview request relaunches the share's start command; the share lifecycle across park/wake/agent delete/freeze (RFE-006); tables, routes, limits, audit, dashboard (agent page), Helm/DNS/TLS requirements for the wildcard domain, and tests.
2. **P4-PRV-2 — the relay.** `aurora-relay` (a new Rust service) resolves preview hostnames through the API, authenticates the browser (session cookie scoped to the preview domain or a signed short-lived grant; specify the CSRF/cookie isolation between the dashboard origin and preview origins — previews must never receive the dashboard session), opens byte streams through the **runner's port-stream API multiplexed over the supervisor channel** with credit-based flow control (`16` wire changes), allow-list enforced at both runner and supervisor (only shared ports), limits (concurrent streams, bytes, idle), WebSockets, NetworkPolicy, Helm, metrics, and tests. Note P4-PRV-2 no longer waits for SSH (D-138).
3. **P4-GSL — git skill library.** `21 §Phase 4 — Git skill library`: an org connects a repository through an **app connection of a git-hosting package** (GitHub or GitLab built-ins) as a skill library; only skills added to agents are stored (with commit sha); background refresh under a lease; byte budget per agent; how library skills compose with the agent/workspace/org tiers (RFE-020, SEC-56: instruction content changes only by humans with authority — a library refresh is a change: specify review/pinning so a repository push cannot silently change what agents are told); tables, routes, jobs, dashboard (skills pages), and tests.

## Rules
As SPEC-P4a/b: phase-tag "(phase 4, P4-xxx)"; update every owning doc (01, 02 §Limits, 03 §Planned changes with level-4 headings, 04 permission matrix, 05, 06, 07, 08, 09, 10, 11/12, 14, 15, 16, 17 SEC + residual risks, 18, 21); no future columns in machine-checked tables; most secure reasonable default for open choices, listed under "Choices made"; ask through Orca `ask` for consequential choices (a security property, new required infrastructure such as a wildcard DNS/TLS requirement for every install — previews must be opt-in, a breaking change). Existing configs keep working.

## Numbers reserved
Decisions **D-144..D-149**; SEC invariants **SEC-80..SEC-85** (in `17` and new `docs/tests/security-coverage.md` rows, status `pending`); contracts **C-540..C-579** for implementation (do not write `API_CONTRACTS.md`; name routes and shapes in `05`). A new token prefix only if needed (pick one not in `00`'s table and add it).

## Constraints
- You own `docs/0*.md`, `docs/1*.md`, `docs/21-later-phases.md`, `docs/90-decisions.md` (D-144..D-149), `docs/tests/security-coverage.md` (new rows only). Don't edit `docs/80-design-review.md`, `docs/API_CONTRACTS.md`, `docs/PROGRESS.md`, `docs/FEATURES.md`, `docs/operator/**`, `docs/audit/**`, or code.
- Other lanes run code tasks in parallel and may touch `docs/04`, `05`, `14`, `17`; edit only your own sections and expect the coordinator to merge.
- Change files only with the Edit/Write tools; never python, sed, awk, perl, `cat >>` or heredocs. Don't run `scripts/e2e/*` or `scripts/perf/*`.

## Acceptance
- One commit per item; `bash scripts/guards/run-all.sh`, `python3 scripts/extract-schema.py --check`, `cargo test -p aurora-db --test it schema` pass.
- Report in the `worker_done` message itself (plain text): per item every doc and section changed, "Choices made", open questions.
