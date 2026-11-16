# P4-LLM-5: Harnesses on the new providers — Claude Code on Bedrock and Vertex, Codex on OpenAI and Azure, end to end — **medium, phase 5**

## Goal
Make the shipped harnesses actually use the providers P4-LLM-1..4 add, without any credential in the
sandbox: `claude-code` declares and speaks `anthropic_bedrock` and `anthropic_vertex` with the
"skip auth" environment, `codex` reaches `openai`, `azure_openai` and compatible endpoints through
its rendered provider block, both adapters classify provider errors with the shared table, and the
hosted e2e proves the sandbox never sees a key, a source key or a minted token.

## Linked requirements
- D-121(c), **D-260** (a), (d), D-261 (auth mode never reaches the harness); contracts C-808
  (adapter side), **C-809** — `docs/API_CONTRACTS.md` §Phase 5 — P4-LLM.
- `12 §Adapter claude-code` (P4-LLM bullet: dialects, env per dialect, same env for every auth
  mode), `12 §Harness selection` (*Provider reach per harness*), `12 §Adapter codex` (base URL per
  provider), `12 §Provider refusal classification` (`credential_unavailable` `llm_mint` →
  `provider_auth` when `mint_rejected`; errors inside a 200 stream), `08 §Phase 4 providers`
  (*Wire formats and harness reach*), `16` (`ModelChoice`).
- Security: SEC-64 (sandbox probe), SEC-66 (unchanged). Tests: `18 §Additional LLM providers`
  (sandbox isolation, harness reach, live suites), `18 §Harness adapters`.
- Migrations: none.

## Context
- Adapters: `services/aurora-supervisor/src/adapters/claude_code/{mod.rs,config.rs,tests.rs}`
  (env allow-list and placeholders, `ANTHROPIC_*`), `adapters/codex/config.rs`
  (`provider_base_url` reads `OPENAI_BASE_URL` from `client_env`), the adapter registry and
  `HarnessCapabilities.dialects`.
- API side: `services/aurora-api/src/domains/hosted/model.rs` (P4-LLM-1 already passes the real
  auth mode), providers' `client_env` (P4-LLM-1..4).
- Conformance: `services/aurora-supervisor/tests/conformance.rs` (live, env-gated),
  `tests/conformance/harness`; hosted e2e `tests/e2e-hosted/tests/{hosted.rs,llm.rs}`,
  `scripts/e2e/hosted.sh`, the SEC-01 secret probe.
- Pinned CLIs: the Claude Code and Codex versions pinned by sha256 in the sandbox image; their env
  names for Bedrock/Vertex (`CLAUDE_CODE_USE_BEDROCK`, `ANTHROPIC_BEDROCK_BASE_URL`,
  `CLAUDE_CODE_SKIP_BEDROCK_AUTH`, `CLAUDE_CODE_USE_VERTEX`, `CLOUD_ML_REGION`,
  `ANTHROPIC_VERTEX_PROJECT_ID`, `ANTHROPIC_VERTEX_BASE_URL`, `CLAUDE_CODE_SKIP_VERTEX_AUTH`) —
  confirm them against the pinned version's documentation and pin them in the adapter with a
  comment naming the version.

## Scope
### In scope
1. **`claude-code`**: dialects `[anthropic_messages, anthropic_bedrock, anthropic_vertex]` (declared
   for the pinned version; drop one if the version lacks its skip-auth switch); env per dialect from
   `client_env` through the adapter's allow-list (add the Bedrock/Vertex names, never any `AWS_*`
   credential variable, `AWS_BEARER_TOKEN_BEDROCK`, `GOOGLE_APPLICATION_CREDENTIALS` or profile
   variable — strip them if present); `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1` for all three;
   `ANTHROPIC_MODEL` as today. A supervisor that receives an unknown dialect refuses the turn with
   `harness_failed` (`16`).
2. **`codex`**: no new Codex settings; verify `provider_base_url` yields C-809's URLs for `openai`,
   `azure_openai` (`<base>/openai/v1`), `openai_compatible` and Bedrock's OpenAI surface; Azure's
   placeholder `Authorization` is replaced at the gateway (nothing rendered for `api-key`).
3. **Error classification in both adapters**: provider errors through `aurora_llm::classify` with the
   body's or stream event's error type (event-stream exception frames for Bedrock, SSE `error`
   events); `credential_unavailable` with `kind = llm_mint` → `gateway_refused`, except
   `mint_rejected` → `provider_auth` with the gateway code in the message.
4. **Mock-upstream conformance**: run `tests/conformance/harness` for `claude-code` against the
   Bedrock and Vertex mocks (through a gateway with the mocks behind `llm.dev_upstream_overrides`),
   and for `codex` against the OpenAI and Azure mocks.
5. **Hosted e2e** (`tests/e2e-hosted`): a claude-code turn on `bedrock` with each of
   `aws_assume_role`, `aws_sigv4`, `api_key` and on `vertex`; a Codex turn on `openai`, on
   `azure_openai` with `api_key` and with `azure_entra`, and on an `openai_compatible` endpoint
   declaring `openai_responses`; the SEC-01 probe extended with a seeded AWS source key, a session
   token, an Entra client secret, a service-account private key and a minted token — none found in
   sandbox env, files, `/proc`, transcripts, logs, request log or metrics; the sandbox environment
   is byte-identical across the three Bedrock auth modes.
6. **Live suites** (env-gated, not CI): `AURORA_TEST_LIVE_BEDROCK`, `…_VERTEX`, `…_OPENAI`,
   `…_AZURE`, `…_COMPAT` — one real turn per provider through a real gateway; they pin env names,
   the `anthropic` path set (Claude Code calls nothing outside it) and error types.
7. Model readiness check: an `openai_chat`-only endpoint is `no_shared_dialect` for both harnesses
   (IT on the readiness function).

### Out of scope
Provider modules and gateway mechanics (P4-LLM-1..4), dashboard (P4-LLM-6), a new harness, Codex
chat wire API.

## Constraints / file ownership
Owns: `services/aurora-supervisor/src/adapters/{claude_code,codex}/**` (env and classification
parts), conformance tests, `tests/e2e-hosted/**` additions, `scripts/e2e/hosted.sh` wiring for the
mocks. No change to provider modules except a bug found by e2e (then coordinate with the owning
lane's merged code; fix in a separate commit naming it).

## Acceptance criteria (each test seen failing first)
1. Supervisor unit: env for each dialect equals C-809 and contains no credential-shaped value; an
   injected `AWS_SECRET_ACCESS_KEY`/`AWS_BEARER_TOKEN_BEDROCK` in `client_env` is dropped; unknown
   dialect → `harness_failed`.
2. Adapter unit: classification of a Bedrock `ThrottlingException` stream exception →
   `provider_rate_limited`; OpenAI 429 `insufficient_quota` body → `provider_quota`;
   `credential_unavailable`/`mint_rejected` → `provider_auth`; `llm_path_not_permitted` →
   `gateway_refused`.
3. Conformance (CI, mocks): claude-code on Bedrock and Vertex, codex on OpenAI and Azure — the
   existing suite passes unchanged.
4. Hosted e2e: every turn of scope 5 completes; the extended SEC-01 probe finds nothing.
5. fmt, clippy, supervisor and conformance suites, hosted e2e, guards exit 0.

## Dependencies
P4-LLM-1; P4-LLM-2 (Vertex), P4-LLM-3 (Azure), P4-LLM-4 (Bedrock) for the matching e2e cases —
the adapter work (scope 1–3) can start after P4-LLM-1.

## Suggested agent/model and reason
Sonnet (`rust-developer`): cross-component verification with real CLIs; the env pinning needs care
but little design.

## Completion report
Commits on `alexiri/p5-agent`: `0bb7fc77` (code, hosted e2e s25/s26, mocks), master merges `0a17730b` and `f1a6874a`, and the
docs commit carrying this report plus the `12`/`18` notes.
Gate level: **batch gate, fully run** (after `git merge master` at `4db07895`).
- **Ran green:** `cargo fmt --all --check`; workspace `clippy --all-targets --all-features -D warnings`; full suites `aurora-llm`
  (34+73), `aurora-core` (74), `aurora-config` (20+68), `aurora-gateway` (75+301), `aurora-testkit` (6+12), `aurora-supervisor`
  (98+2+4+104), `aurora-api` (all targets, `--test-threads=4`, after `scripts/fixtures/build.sh` 57 passed: it-suite 1145 passed,
  0 failed; no known load flake appeared); `scripts/guards/run-all.sh` exit 0; `extract-schema.py --check` up to date;
  `openapi_matches_checked_in` ok; `FRONTDOOR=1 bash scripts/e2e/gateway.sh` (22 and 4 passed, plus the smaller targets, all 0 failed; includes s24_bedrock).
- **Re-run 2026-11-16 after `git merge master` at `318f115c`** (merge `49658320`; the merge changed `aurora-config`, `aurora-db`,
  `Cargo.lock` dependency bumps; a stale `sha2` lock line was committed separately). `cargo check --workspace --all-targets --locked`
  ok; fmt ok; workspace clippy `-D warnings` ok; full suites (every target) `aurora-config`, `aurora-db` (38 it), `aurora-llm`,
  `aurora-core`, `aurora-gateway`, `aurora-supervisor`, `aurora-testkit` all green; guards exit 0; `extract-schema.py --check` 0.
  `aurora-api` it-suite: 1161 passed, **1 failed** (`settings_governance::the_reevaluation_job_marks_secrets_and_clears_overrides_once`,
  "lock free": a master test using a DB-global advisory lock, failed in two full runs under load 20+ with 4 threads; it passes
  alone and `settings_governance::` is 7/7 green; not touched by this lane).
- **e2e:** `FRONTDOOR=1 gateway.sh` green on the rerun (a first run failed `hooks::h2_mint_through_real_hookhost…` under load 20+, passed on
  rerun, all targets 0 failed); `llm.sh` exit 0 (4 passed); `AURORA_TEST_PODMAN=1 hosted.sh` 19 passed (s25/s26 skip without the
  full image); with `AURORA_E2EH_FULL_IMAGE=1` (own prefix/subnets, because another lane's default-prefix run tore down a first
  attempt's Postgres) **19 passed, 0 failed, including s25 (codex on Azure OpenAI, both auth modes) and s26 (claude-code on Bedrock,
  every auth mode)**, with the Azure/Bedrock/mint legs against the repo's mock servers (no live cloud).
- **What changed:** see the commit message of `0bb7fc77`: claude-code `anthropic_bedrock`/`anthropic_vertex` env (switches only,
  credential variables dropped, unknown dialect → `harness_failed`); shared classification table moved to `aurora-core`; both
  adapters classify through it, rejected mint → `provider_auth`; codex C-809 base-URL test; mocks extended.
- **Fail-before:** not re-checked in this session (done by the previous worker at commit time; not re-verified here).
- **Open:** live suites (scope 6, env-gated) not run; scope 7 (`no_shared_dialect` IT) is covered by the aurora-api it-suite run above.
