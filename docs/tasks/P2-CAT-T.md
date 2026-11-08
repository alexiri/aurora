# P2-CAT-T: Token lifecycle in built-in package tests (phase 2)

## Goal
`21 §Phase 2 — App catalogue` requires every built-in package's fixtures to cover **the token lifecycle against recorded HTTP**. Today `aurora plugin test` replays recorded HTTP only for hooks (P2-CAT-H). For the declarative mechanisms (`oauth2_refresh`, and `mint` where it isn't a hook), `scripts/plugins/check_builtin.py` only checks that `reauth_codes` exist and that some fixture injects an already-refreshed token (FU-P2-CAT-0a). Make the lifecycle a real, replayed test, and bring the existing packages up to it.

## Linked requirements
- `docs/21-later-phases.md` §Phase 2 — App catalogue (first bullet: fixtures cover "the token lifecycle against recorded HTTP").
- `docs/09-app-plugins.md` §3.4 `token` (kinds `static` | `oauth2_refresh` | `hook`, `refresh_field`, `reauth_codes`, stale codes), §5 mechanisms, §12 developer experience (fixture format, `aurora plugin test`), §8 (the hook cassette from P2-CAT-H, D-77).
- `docs/06-gateway.md` refresh-on-401/stale and `invalid_grant` → `reauth_required` behaviour (what the replay must reproduce).
- Follow-ups: FU-P2-CAT-0a (lifecycle not replayable), FU-P2-CAT-0b (GitLab icon placeholder), FU-P2-CAT-5b (aws-keys endpoints from knowledge, links unchecked).

## Scope
1. **Replay in `aurora plugin test`.** A fixture case can declare recorded token-endpoint exchanges (same cassette idea as P2-CAT-H's hook fetches: request match + recorded response, credentials scrubbed). The runner then exercises, through the real gateway mechanism code path that `aurora plugin test` already uses:
   - **refresh**: an upstream answer matching a stale code (e.g. 401) → the refresh exchange against the recorded token endpoint → the retried request carries the new token;
   - **re-auth**: a refresh answered with a `reauth_codes` error (e.g. `invalid_grant`) → the connection is reported `reauth_required`, nothing is retried;
   - **mint** for declarative mint mechanisms, if any package uses one.
   Unused cassette entries fail the case, as for hooks.
   - If declaring these exchanges needs a new key in the fixture format of `09 §12`, that is a contract change: **stop and ask through Orca `ask`** with the proposed shape before implementing. Prefer reusing the P2-CAT-H cassette shape.
2. **Coverage rule** in `check_builtin.py`: every app whose `token.kind` is `oauth2_refresh` (or a declarative mint) has at least one replayed refresh case and one re-auth case; a package without them fails `check-builtin.sh`.
3. **Bring packages up to the rule**: `gitlab` (instance-bound since P2-INST: cover gitlab.com and one registered instance host), `microsoft-365` (Entra token endpoint on `common`), and any other OAuth package on `master` when you start. `aws-keys` is `static`: no lifecycle cases, but review its endpoints and IAM actions against AWS's official docs (fetch them) and fix what is wrong (FU-P2-CAT-5b).
4. **GitLab icon** (FU-P2-CAT-0b): replace the generated placeholder with a neutral, non-branded icon that meets `09 §6` icon rules (no vendor logo unless its trademark terms allow it; when in doubt, neutral).

## Constraints / file ownership
- You own `services/aurora-cli/src/plugin` (the test runner), `scripts/plugins/check_builtin.py` and `check-builtin.sh`, `plugins/builtin/{gitlab,microsoft-365,aws-keys}/{tests,assets}`, and fixture-format docs in `09 §12` only if the owner approves a change through `ask`.
- Don't change the gateway's refresh behaviour; the test must drive the existing code. If the replay shows a gateway bug, report it with a failing test case.
- Never commit live tokens; `scripts/guards/no-live-tokens.sh` stays green.
- Contract numbers only if a wire shape changes: C-280..C-289. Decisions: next free number.

## Acceptance criteria
- [ ] `aurora plugin test` replays refresh and re-auth cases; a deliberately broken recorded exchange makes the case fail (tested).
- [ ] The coverage rule is enforced by `check-builtin.sh`, and every built-in on `master` passes it.
- [ ] fmt, clippy `-D warnings`, `cargo test -p aurora-cli`, `scripts/fixtures/build.sh`, `bash scripts/plugins/check-builtin.sh` and `bash scripts/guards/run-all.sh` pass.

## Suggested agent/model
Sonnet, own worktree.
