# P2-CAT-0: Built-in app catalogue — framework and the GitLab reference package (phase 2)

## Goal
Phase 0 ships no built-in packages. Build the machinery that every catalogue package needs (`21 §Phase 2 — App catalogue`) and prove it end to end with one complete package, **GitLab**, which the other catalogue lanes (`P2-CAT.md`) then copy.

## Linked requirements
- `docs/21-later-phases.md` §Phase 2 — App catalogue (the MUST list); `docs/19-implementation-plan.md` §Phase 2 (package list).
- `docs/09-app-plugins.md`: §2 package format, §3 manifest contract v1, §5 mechanisms, §6 validation, §10 package lifecycle (`builtin` source: `/usr/share/aurora/plugins`, activated on first sight and on upgrade unless permissions widened), §12 developer experience (`aurora plugin validate|compile|test|docs|sign`, fixture format), §13 fixture packages (as examples of the format).

## Scope
1. **Layout and packaging.** `plugins/builtin/<package>/` (one directory per package: `plugin.yaml`, `guide.md`, icons, `tests/*.yaml`, optional `hooks/`). The API image ships them at `/usr/share/aurora/plugins` (`images/services/Containerfile`, `api` target only), and the API loads them through the existing `builtin` source. Confirm that path end to end with the dev stack.
2. **Checks for every built-in package**, wired into `scripts/guards/` (so they run locally and in CI's `guards` job) or a new small CI step, whichever matches the repo:
   - `aurora plugin validate` passes;
   - `aurora plugin test` passes, and the fixtures cover every host entry, every tool (at least one positive and one negative match), every summary template and the token lifecycle against recorded HTTP;
   - `aurora plugin docs` renders the setup guide and permissions table, and the rendered output is checked in or regenerated deterministically (pick one and say which);
   - the `about.api_docs` / `api_spec` URLs resolve. Network checks must not make CI flaky: run the link check in a separate, non-blocking CI job (or a scheduled one), and say which.
   - Narrowest scopes by default; write scopes are marked.
3. **Signing.** Built-ins are "signed by the project key in the release pipeline". No project key exists yet, and creating one is the owner's decision. Make the pipeline step ready: a `sign` step that runs only when a key secret is configured, otherwise skips with a clear log line. Document what the owner must provide in `docs/ops/`. Do **not** generate or commit a key. Dev and e2e load the packages unsigned through the existing dev path; check how `builtin` + unsigned behaves and keep production fail-closed.
4. **GitLab package** (the reference). gitlab.com and self-managed instances (instance URL as connection metadata). Auth: personal/project access token (bearer) and OAuth (authorization code with PKCE where supported); tools over the REST v4 API (projects, issues, merge requests, pipelines, repository files, and git over HTTPS if the manifest contract covers it), read tools by default and write tools marked. Clamped summaries for write operations. Recorded-HTTP fixtures; no live calls in tests.
5. **Template for the other lanes.** A short `plugins/builtin/README.md`: how to add a package, what the checks require, how to record HTTP for fixtures without real secrets.

## Constraints / file ownership
- You own `plugins/builtin/**`, the packaging lines in `images/services/Containerfile` (`api` target only), the new guard/CI check, `docs/ops/` (signing note), and any small CLI fix needed in `aurora plugin` (report it).
- Don't change the manifest contract (`09 §3`) or the gateway mechanisms. If GitLab needs something the contract doesn't have, stop and ask through Orca `ask`.
- Never commit real tokens. Recorded HTTP must have credentials scrubbed; add a guard that fails on anything that looks like a live token in `plugins/builtin/**/tests`.
- Contract numbers only if a wire shape changes: C-210..C-219. Decisions: next free number.

## Acceptance criteria
- [ ] The API image contains `/usr/share/aurora/plugins/gitlab`, and the dev stack shows GitLab as a built-in package (activated, or pending approval if unsigned — say which, and why).
- [ ] All four checks of scope 2 run for every package under `plugins/builtin/` and pass for GitLab.
- [ ] The signing step is ready and skips cleanly without a key; the owner's to-do is documented.
- [ ] fmt, clippy `-D warnings`, `cargo test` for touched crates, guards and the web checks (if touched) pass.

## Suggested agent/model
Sonnet, own worktree.
