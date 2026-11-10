# P4-GSL: Git-backed skill library

## Goal
Implement D-148/D-149 and SEC-83, SEC-84 (SPEC-P4c): an org connects a repository through an app connection of a git-hosting package (GitHub, GitLab) as a skill library; skills are added with the tier's existing authority, pinned to a commit, read-only; refresh only proposes, a person applies after reviewing the full diff.

## Spec (source of truth; read, don't re-decide)
- D-148, D-149; SEC-83, SEC-84 (+ residual) in `docs/17`.
- `docs/10` §Git skill library (connect, browse, add, budgets, read-only, refresh and proposals, untrusted text) and the Skills bullet; `docs/09 §3.4` manifest block `library` (data-only GET templates) + linter rules; `docs/06` §Library fetch (`POST /internal/v1/library-fetch` on the internal port; accepted by the coordinator); `docs/03 §Planned changes` (`skill_libraries`, `skills` library columns, `skill_library_updates`); `docs/05` (Skill libraries routes, two jobs); `docs/04` matrix + audit; `docs/12` skill tool; `docs/14` (Skill libraries page, Add from library, update review, workspace Skills); `docs/15` (config, §Git skill library); `docs/16` W4; `docs/01` boundary 5; `docs/02` limits.
- `docs/18` the git skill library tests.

## Scope
Migration + repos; API routes and jobs (refresh under a lease writes proposals only; apply quotes the content hash); gateway internal fetch route (approved manifest GET templates only, org-scoped connection, internal secret); registry/linter for the `library` block; the GitHub and GitLab built-in packages gain the `library` block (version bump, `check-builtin.sh`, signing by the release pipeline); web pages; operator doc. Contracts **C-570..C-579** only.

## Acceptance
Every test in `18` for the library; SEC-83, SEC-84 `proven`; full `aurora-api` (`--test-threads=8`), gateway tests, registry tests, `scripts/plugins/check-builtin.sh`, web checks + Playwright, fmt, clippy `-D warnings`, guards, OpenAPI drift test.

## Constraints
A repository push never changes what an agent is told without a person applying it. Library text is untrusted. Change files only with the Edit/Write tools. No `scripts/perf/*`; ask the coordinator before any `scripts/e2e/*`.
