# P4-EVAL-3: judged questions through `PlatformLlm` with a granted key

## Goal
Judged eval questions are scored by a judge model called through the platform's own `PlatformLlm` service with the agent's granted judge key, under governance, with minimal input, strict output parsing, usage recorded on the run, and no fallback provider. A judge failure is an `error`, never a pass.

## Linked requirements
- D-292 (judge through `PlatformLlm`, the key named and granted, no fallback), D-289(b) (judged kind).
- Contracts C-862 (judged scoring), C-869 (`PlatformLlm` judge service: request, usage, errors), C-872 (codes), C-873 (audit: none new beyond usage in the run).
- SEC-115 (judge calls: granted and governance-admitted key; minimal input; permitted destination only; strict parse; failure is `error`; usage recorded).
- `docs/08 §Platform-internal LLM calls` (the single `PlatformLlm` service rule, which this brief implements for the first time), `docs/08 §LLM secrets`, `§Model readiness`, `§11 governance`, `docs/10 §Evals`.

## Context
- `PlatformLlm` does not exist yet. `docs/08` requires that internal calls go through one service that names the workspace secret it uses and records usage. Phase 0's model-catalogue listing is the only current internal call (API → endpoint, granted key, cached 10 min); find it (`grep` the model listing path under `services/aurora-api/src/domains/hosted/` or `domains/secrets/`) and move it behind the same service only if the change is small; otherwise leave it and say so.
- Provider request builders and the destination guard: `crates/aurora-llm` (`anthropic.rs`, `litellm.rs`, `registry.rs`, `governance/`), used by the P4-LLM validation path (`docs/08 §LLM secrets` step 2). Reuse them; do not add a second HTTP client.
- Grants and governance: `services/aurora-api/src/domains/grants/`, `domains/governance/`, readiness `domains/hosted/readiness.rs`.
- Scoring entry point from P4-EVAL-2: `services/aurora-api/src/domains/evals/scoring.rs`.
- Not `aurora-summary`: that crate is the template summarizer for approvals and calls no LLM.

## Scope
### In scope
- `services/aurora-api/src/domains/llm/platform.rs` (new): `PlatformLlm::complete(purpose, secret_ref, model, request)` for purpose `eval_judge` only in this brief (an enum so later purposes are explicit); checks, in order: the secret is `active`, granted to the agent that owns the eval run, admitted by governance for its endpoint and scope, usable per readiness; then the request through `aurora-llm` with the destination guard; no streaming, no tools, no system prompt from the agent, no memory, no other transcript; timeout `EVAL_JUDGE_TIMEOUT`; output cap `EVAL_JUDGE_MAX_TOKENS`; usage returned to the caller.
- Judge input (SEC-115): the question text, the criteria (judged questions), and the final answer text of that question's own turn, cut at `EVAL_JUDGE_INPUT_MAX` (8 KiB). Nothing else: no other conversations, no tool arguments or results, no expected answers of other questions.
- Judge output: strict JSON `{ "verdict": "pass" | "fail", "rationale": string ≤ EVAL_RATIONALE_CHARS }`. Anything else → `judge_unparseable` (an `error`).
- Scoring of judged questions in `scoring.rs` (C-862): binary verdict; the rationale is stored sanitised (one line, `EVAL_RATIONALE_CHARS`) and shown only to `can_use` viewers.
- Usage: token counts summed into `eval_runs.judge_usage` (never into `turns.usage`).
- Errors (C-869/C-872): `judge_not_configured` (no judge set), `judge_not_granted` (revoked since settings were saved), `judge_not_permitted` (governance now refuses), `judge_unavailable` (endpoint unreachable or non-2xx), `judge_timeout`, `judge_unparseable`. Each is an `error` result for that question; the run continues.
- Refusal when the judge key's endpoint is not permitted for the eval's destination (egress governance, `docs/08 §11.6`): the call is not made.

### Out of scope
New providers (P4-LLM), a fallback judge, an org-wide judge setting (Q-EVAL-4 alternative), 1-to-5 scoring (Q-EVAL-5 alternative), the dashboard.

## Constraints / file ownership
- Owns: `services/aurora-api/src/domains/llm/**` (new `platform.rs` and `mod.rs`), `services/aurora-api/src/domains/evals/{judge.rs (new), scoring.rs (judged branch only)}`, judge usage column write in `repo/evals.rs` (the `judge_usage` field only), limits rows (`EVAL_JUDGE_*`, `EVAL_RATIONALE_CHARS`, `EVAL_JUDGE_INPUT_MAX`) and their `limits-gaps.allow` lines.
- Must not change: `crates/aurora-llm` provider code (P4-LLM's), the gateway's credential injection, `turns.usage`.
- A judge call is never a turn and never writes a transcript event.

## Acceptance criteria (failing-first)
1. Unit and IT tests with a mock provider: a judged question with a `pass` verdict → `pass`; `fail` → `fail`; prose instead of JSON → `error` `judge_unparseable`; extra keys rejected; rationale over the cap truncated, never a failure.
2. SEC-115 input test: the request body contains the question, criteria and that turn's answer only; a canary in another conversation and a canary tool argument appear in no judge request.
3. Governance and grants: judge secret revoked after settings saved → `judge_not_granted` with no request sent; endpoint not permitted for the scope → `judge_not_permitted` with no request sent; key not active → `judge_not_usable`.
4. No fallback: when the judge endpoint is down, the question is `error` `judge_unavailable`; the run does not retry on another key or provider (asserted by the mock having a second, healthy provider that is never called).
5. Timeout and output cap: a judge that hangs → `judge_timeout` at `EVAL_JUDGE_TIMEOUT`; the request carries `max_tokens = EVAL_JUDGE_MAX_TOKENS`.
6. Usage: `eval_runs.judge_usage` sums input and output tokens across questions; `turns.usage` of the eval turns is unchanged by judge calls (test).
7. Secrets: the key value and the judge request's headers appear in no log line, audit row or result (grep-based test over captured logs).
8. `cargo nextest run -p aurora-api` (targeted), `cargo clippy … -D warnings`, `bash scripts/guards/run-all.sh` (exit 0).

## Verification
Per-task gates of `docs/tasks/LANE_RULES.md`. Opus reviews `platform.rs` (the single internal-LLM choke point) before merge.

## Dependencies
- Blocked by: P4-EVAL-1 (settings and judge key) and P4-EVAL-2 (scoring entry point, `eval_runs`); P4-LLM providers merged (the judge may use any granted provider; the request builders are P4-LLM's).
- Unblocks: the judged-question rows of P4-EVAL-4's results page (they render whatever the run stores).

## Suggested agent/model and reason
`rust-developer` (Sonnet): a new internal service, provider calls and governance checks. Opus reviews `platform.rs` and the egress decision. Size **M**.

## Completion report
**Not started (2026-11-17, worker 25 stopped at the context budget after P4-EVAL-2 and P4-EVAL-4). Handoff for the next worker:**
- Findings on master (checked 2026-11-17): (1) `crates/aurora-llm` has **no completion call**: `LlmProvider` offers `list_models*`, `injection_for(ep, mode)` (the header recipe), `base_url`, `dialects(ep)`, `inference_path_permitted`, `classify_error`; `PlatformLlm::complete` must therefore build the request itself from the provider's dialect (Anthropic Messages, OpenAI chat completions) and inject the key with the provider's own recipe; providers whose auth is not a plain header (Bedrock SigV4, Vertex mint, `aws_assume_role`, Azure variants) should be refused at settings time as `eval_judge_not_usable` (extend `evals/service.rs::judge_check`, which already walks `model::readiness`) and, if one slips through, answered `judge_unavailable` — say which dialects/modes are supported in the report. (2) The shared HTTP client is `AppState.llm_http` (`app.rs:79`) and the registry is `s.llm()`; secret values are opened the way `domains/secrets/llm.rs`/`hosted` do (find the existing decrypt helper; never log it). (3) `scoring.rs::score` returns `error`/`judge_not_available` for `judged` (line ~100); `engine.rs::score_turn` (line ~178) calls it inside the tick's transaction — **the judge call (up to `EVAL_JUDGE_TIMEOUT`, 60 s) must not run while the engine holds the conversation lock or an open transaction**: add a result state between the turn finishing and the score (for example score judged answers in a second tick step after commit, or make the arm release the lock before the call) and keep the lease alive meanwhile. (4) Already present from P4-EVAL-2: `repo::evals::add_judge_usage` (the `eval_runs.judge_usage` sum), `EvalResultJudge {verdict, rationale}` on results, the settings and `judge_check`, the web page's wording for `judge_not_available`, `judge_not_configured`, `judge_not_granted`, `judge_not_permitted`, `judge_unavailable`, `judge_timeout`, `judge_unparseable` (`web/src/components/agents/evals/copy.ts`). (5) Still to add: `EVAL_JUDGE_TIMEOUT`, `EVAL_JUDGE_MAX_TOKENS`, `EVAL_JUDGE_INPUT_MAX`, `EVAL_RATIONALE_CHARS` in `aurora-core::limits` (their `doc-only` lines are `scripts/guards/limits-gaps.allow` 111–114; the guard wants a reader for each), `domains/llm/{mod,platform}.rs`, `domains/evals/judge.rs`; the integration tests can use `wiremock` (already a dev-dependency of `aurora-api`) as the provider with a `litellm` or `openai_compatible` endpoint whose `base_url` is the mock. The acceptance list above stands; the C-869 error table is in `docs/API_CONTRACTS.md`. Also remember C-883 (the fingerprint holds `judge` already).
- Files changed: none.
- Tests/checks (exact commands and results): none run for this task.
- Remaining risks: all of it; plus the batch gate below is still to run **after** this task lands (see the lane report).
