# P4-EVAL-1: eval questions, agent eval settings, fingerprint and the eval API surface

## Goal
Per-agent eval questions with expected answers (numeric, text, judged), the agent's eval settings (judge key, regression notice) and the configuration fingerprint function, behind contracts C-861, C-863 and C-866, with no run yet. This is the first slice of P4-EVAL and the one the other three briefs build on.

## Linked requirements
- D-289 (model and scope), D-291 (fingerprint), D-292(a) (the judge key is named in the agent's settings and must be granted).
- Contracts C-861 (questions), C-862 (expected-answer kinds, storage part), C-863 (settings), C-866 (fingerprint and its preview route), C-872 (limits and error codes of these routes), C-873 (audit actions of these routes).
- SEC-116 (expected answers are agent-audience content: never in the sandbox, boot document, gateway, logs, request log or audit metadata); SEC-115 (the judge key must be granted and governance-admitted when it is set).
- `docs/10 §Evals`, `docs/03 §Planned changes` (`eval_questions`, the `agents` columns), `docs/05 §Evals`, `docs/04` matrix rows, `docs/02 §Limits` (`EVAL_*`), `docs/08 §LLM secrets`.
- Migrations: `20261201000236_eval_questions.sql`, `20261201000239_agents_eval_judge.sql` (`000237`, `000238`, `000240` belong to P4-EVAL-2).

## Context
- Agents: `services/aurora-api/src/domains/agents/{dto.rs,service.rs,routes.rs}`, repo `crates/aurora-db/src/repo/agents.rs`. The `agents` columns are in `docs/03 §5`; `effort`, `brief`, `harness`, `model_provider`, `model_endpoint_id`, `model_id` are the fingerprint's agent fields.
- Model readiness for a key: `services/aurora-api/src/domains/hosted/readiness.rs` (one function, `docs/08 §Model readiness`). Reuse it; do not re-derive the checks.
- Grants and effective rules: `services/aurora-api/src/domains/grants/{effective.rs,compile.rs}`, repo `crates/aurora-db/src/repo/grants.rs`, `repo/policy.rs`. Skills' effective set: `domains/skills` (`GET /v1/agents/{agent}/skills/effective`, `05`).
- Authority helpers: `aurora-db::authz` (`can_use`, `can_own`; locate the module path before coding, `docs/04 §The access law`).
- Limits: `crates/aurora-core` (the limits module; add the constants there).
- Audit: `crates/aurora-db/src/repo/audit.rs` (`docs/04 §Audit`, `C-873`).
- Routing and OpenAPI: the router's domain registration (`services/aurora-api/src/domains/mod.rs`) and `web/openapi.json` (regenerate with the repo's own command; check `scripts/extract-schema.py` and the docs-routes guard).

## Scope
### In scope
- Migrations 236 (`eval_questions`) and 239 (`agents.eval_judge_secret_id`, `agents.eval_judge_model_id`, `agents.eval_regression_notify default true`), with the matching `docs/03` table moves.
- `domains/evals/` (new): `questions.rs`, `settings.rs`, `fingerprint.rs` (pure function: canonical JSON, SHA-256, C-866 field list), `routes.rs`, `dto.rs`; repo `crates/aurora-db/src/repo/evals.rs` (questions and settings only).
- Routes of C-861 (question list/create/get/patch/delete, `position` ordering), C-863 (`GET` and `PUT /v1/agents/{agent}/eval-settings`), C-866 (`GET /v1/agents/{agent}/eval-fingerprint`).
- Hosted agents only (BYO: 422 `eval_not_supported`).
- Audit rows `eval.question.create|update|delete`, `eval.settings.update` (ids and counts only; never an expected answer or a secret value).
- Limits `EVAL_QUESTIONS_PER_AGENT`, `EVAL_QUESTION_CHARS`, `EVAL_EXPECTED_TEXT_CHARS`, `EVAL_CRITERIA_CHARS` in `aurora-core::limits`; remove their `doc-only` lines from `scripts/guards/limits-gaps.allow`.

### Out of scope
Runs, eval conversations, turns, scoring, the judge call, eval schedules, the dashboard (P4-EVAL-2..4). No agent tool of any kind (D-289(d)).

## Constraints / file ownership
- Owns: `services/aurora-api/src/domains/evals/**` (new), `crates/aurora-db/src/repo/evals.rs` (new), migrations `…000236`, `…000239`, the limits module (EVAL rows only), `web/openapi.json` (regenerated), `scripts/guards/limits-gaps.allow` (EVAL lines), `docs/03` and `docs/05` (the eval rows only).
- Must not touch: the run engine, gateway, tools, the schedules engine, `web/**`.
- Expected answers are stored as plain text like other agent-audience content; the responses of `GET` carry them only to `can_use` callers of a workspace the agent belongs to. A non-member or a caller without `can_use` gets 404, not 403.
- Fingerprint hashes the brief and skill contents (never secret values, never question text) and records hashes, not contents, in `eval_runs.fingerprint` later.

## Acceptance criteria (failing-first: each new test fails before its code)
1. `services/aurora-api/tests/it/evals_questions.rs`: create one question of each kind → 201; each invalid shape → 422 `invalid` (judged without criteria; numeric without a number; negative tolerance; expected text over 1 000 characters; question over 2 000); the 101st question → 422 `eval_question_limit`; PATCH bumps `version`; DELETE; positions stay unique per agent.
2. Visibility: a caller without `can_use` gets 404 on every eval route; a `can_use` member gets expected answers in `GET`.
3. Confidentiality (SEC-116): a canary expected answer appears in no audit row's metadata, no request-log row and in no sandbox boot document (test the boot-document builder's output for an agent with questions).
4. Settings: `PUT eval-settings` with a judge secret not granted to the agent → 422 `eval_judge_not_granted`; not usable per readiness → 422 `eval_judge_not_usable`; scope not permitted by governance → 422 `eval_judge_not_permitted`; `null` clears; `can_own` required for the judge (403 for a plain `can_use` member); audit `eval.settings.update` carries no secret value.
5. Fingerprint unit tests: identical configuration gives identical hash; changing the brief, model, effort, a skill's content, or the set of granted rule ids changes it; question text and secret values do not affect it; field list equals C-866 exactly.
6. Migrations: existing agents read `eval_judge_secret_id = null` and `eval_regression_notify = true`; `scripts/extract-schema.py --check` passes after the table moves.
7. `cargo fmt --check`; `cargo nextest run -p aurora-api -p aurora-db -p aurora-core` (targeted: the new `evals` tests and the agent tests); `cargo clippy --workspace --all-targets --all-features -- -D warnings` at the batch gate; `bash scripts/guards/run-all.sh` (exit 0; no `EVAL_*` line left in `limits-gaps.allow` for a constant that now exists).

## Verification
Per-task gates of `docs/tasks/LANE_RULES.md`. Report exact commands and exit codes.

## Dependencies
- Blocked by: none for questions and fingerprint. The judge-key checks reuse `readiness.rs`; they are only as complete as the providers P4-LLM has landed (`docs/08 §Phase 4 providers`), so land P4-LLM-1..6 first or state the gap in the completion report.
- Unblocks: P4-EVAL-2 (runs need questions, the fingerprint and the settings), P4-EVAL-4 (dashboard reads these routes).

## Open questions for the owner
**Answered (owner, 2026-11-15, D-300): every recommendation below accepted.** Each question below carries the recommended answer, and the spec text in `docs/10 §Evals`, `docs/05`, `docs/04`, `docs/API_CONTRACTS.md` §Phase 5 — P4-EVAL follows the recommendation, marked "(owner answer Q-EVAL-n, D-300)".

- **Q-EVAL-1 — Who may create, edit and start evals.** Recommended: any member who `can_use` the agent's workspace creates and edits questions, starts runs and cancels them (the same authority as schedules); choosing the judge key and deleting a run are `can_own`. Alternative: `can_own` for everything.
- **Q-EVAL-2 — Side effects of an eval run (a run is a real turn).** Recommended: eval turns obey every existing rule (policy, approvals, egress, grants, freeze, governance), and in addition an eval turn refuses every write: a gateway request whose effect is a write (not a `read` app tool and not GET/HEAD), `send_message`, `send_file`, `send_as_me`, memory and skill writes, schedule, webhook and peer-task creation. A refused write returns `eval_read_only` to the agent, creates no approval, and is counted on the result. Alternative A: eval turns use the normal rules and approvals (writes happen, approvals are held as usual). Alternative B: a dry-run flag the tools honour (relies on the agent cooperating; not chosen).
- **Q-EVAL-3 — Cost limits.** Recommended: 100 questions per agent; one active run per agent; 20 runs per agent per UTC day (manual and scheduled together); 900 s per question (bounded by `TURN_CEILING`); judge input 8 KiB, output 512 tokens, 60 s; eval schedules at least one hour apart; no token budget (budgets are out of scope, `docs/10 §Usage and metering`), but token usage is recorded on the run. Alternative: a token budget per run once budgets exist.
- **Q-EVAL-4 — Judge model and whose key pays.** Recommended: an LLM secret of the agent's workspace or of the org, granted to the agent, named in the agent's eval settings with a model; it must be usable (`08 §Model readiness`) and governance-admitted; the key owner's account pays. No default: a judged question with no judge fails with `judge_not_configured`. Alternative: an org-wide judge key set by org owners and admins.
- **Q-EVAL-5 — Scoring scale and pass thresholds.** Recommended: numeric and text answers are exact, read from a final line `ANSWER: <value>` that the platform asks for (numeric with an optional absolute tolerance, default 0; text compared after trimming, whitespace collapse and case folding); judged questions are a binary pass or fail from the judge with a rationale of at most 300 characters; the pass rate is pass divided by (pass plus fail). Alternative: a 1 to 5 judge score with a threshold of 4.
- **Q-EVAL-6 — Retention of eval transcripts.** Recommended: eval conversations follow the org conversation retention; runs and their results are deleted after 90 days; deleting an agent or a run deletes its eval conversations through the normal delete path. Alternative: keep runs until a person deletes them.
- **Q-EVAL-7 — Regression alerts.** Recommended: when a run's pass rate is at least 10 points below the previous finished run with the same question-set version, the Evals page shows a notice to eligible viewers; the notice carries counts only. Nothing goes to chat platforms, e-mail or Web Push. Alternative: Web Push or e-mail to the agent's owners.
- **Q-EVAL-8 — Comparing configurations across runs.** Recommended: the run page shows a field-by-field diff of the fingerprint against the previous run and against any chosen run; attribution is that diff, not an automatic cause. Memory is not in the fingerprint in phase 5 (a known gap; the memory revision count is the cheap follow-up). Alternative: a fingerprint per turn.
- **Q-EVAL-9 — Who sees expected answers and answers.** Recommended: `can_use` of the agent's workspace (the agent's audience, like non-direct transcripts). Alternative: expected answers readable only by `can_own`.
- **Q-EVAL-10 — An instance switch.** Recommended: evals are available on every hosted agent with no instance switch. Alternative: an `evals` instance settings section (`SPEC-SETTINGS`, D-236/D-237), off by default like the terminal.

## Suggested agent/model and reason
`rust-developer` (Sonnet): multi-file API work with migrations, authorization and an audit trail. Opus reviews the settings authority and the fingerprint canonicalisation before merge. Size **M**.

## Completion report
- Files changed: migrations `20261201000236_eval_questions.sql`, `20261201000239_agents_eval_judge.sql`; `crates/aurora-db/src/repo/evals.rs` (+ `mod.rs`); `crates/aurora-core/src/{limits,audit}.rs`; `services/aurora-api/src/domains/evals/{mod,dto,fingerprint,service,routes}.rs` (+ `domains/mod.rs`, `app.rs`); `tests/it/{evals_questions,permission_matrix,main}.rs`; `web/openapi.json`, `web/src/lib/api/schema.d.ts` (generated); `docs/03` (real `eval_questions` section, the three `agents` columns; the planned rows removed), `docs/04` (`eval.settings.update` class); `scripts/guards/limits-gaps.allow` (4 EVAL lines); `crates/aurora-db/tests/fixtures/spec_schema.json` (generated).
- Adapted from the 2026-11-15 brief: routes register without `/v1` in the utoipa path (as master does); `domains/evals/` has no `questions.rs`/`settings.rs` (one `service.rs`); fingerprint inputs come from existing master services (`skills::effective`, `hosted::model::readiness` for the resolved roles of C-883, `repo/evals::granted_rule_ids`); the BYO check and the `can_use`→404 rule live in `service::visible_agent`. Migrations 236/239 sit below master's 245/246 (reserved numbers; `upgrade.sh` is the batch gate's check).
- Tests/checks (exact commands and results): `cargo fmt --all --check` 0; `cargo clippy -p aurora-api -p aurora-db -p aurora-core --all-targets --all-features -- -D warnings` clean; `evals_questions` it filter 9/9 pass; fingerprint unit tests (6) pass (`cargo test -p aurora-api --lib evals`); `permission_matrix` (both tests) pass with the new rows; `openapi_matches_checked_in` pass after `UPDATE_OPENAPI=1`; `pnpm -C web gen:api` run; `python3 scripts/extract-schema.py --check` up to date; `bash scripts/guards/run-all.sh` exit 0; `cargo test -p aurora-core -p aurora-db --lib` pass. **Gate level: per task only** — the batch gate (full it suites, workspace clippy, web typecheck/lint/vitest/build, Playwright, `upgrade.sh`, `gateway.sh`/`hosted.sh`) has not run. Fail-first was **not** observed: tests were written with the code (the routes did not exist before, so every test would fail trivially).
- Decisions taken on open questions (owner answers of D-300 followed): any `can_use` member creates, edits and deletes questions; a judge *change* needs `can_own`; a caller without `can_use` gets 404 (an API key confined to another workspace keeps its 403 `workspace_mismatch`); a question's kind is fixed (PATCH has no `expected_kind`; to change kind delete and re-create); PATCH also takes `position` (1-based, clamped, others shift; the list stays contiguous, a delete closes the gap); numeric questions store tolerance 0 when none is given; POST appends. `eval.settings.update` is critical on **every** write (one action name has one class in `SECURITY_CRITICAL_ACTIONS`; `metadata.judge_changed` tells the cases apart; docs/04 reworded). `granted_rule_ids` = logical ids of the derived rules compiled from the agent's grants (the policy rules "granted to the agent"). The fingerprint uses C-866's `model: {provider, endpoint_id, model_id}` (docs/10's flat field names differ; the contract wins). Skill content hash = SHA-256 over length-prefixed description, body and files (path, executable bit, bytes).
- Remaining risks: the IT covers the fingerprint's skills/granted-rule parts only through the unit tests (the route test uses an agent with none); the judge check treats a model outside a static endpoint list as `eval_judge_not_usable` and an org allow-list refusal as `eval_judge_not_permitted`; the 100-question cap and positions are serialised with an advisory lock per agent; P4-LLM provider completeness gap: the readiness function is master's (all providers it knows).
