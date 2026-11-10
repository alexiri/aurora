# P4-HAR: Harness adapters and selection (Codex)

## Goal
Implement D-122 (SPEC-P4a): a second harness, `codex` (the pinned OpenAI Codex CLI), selectable per agent among `hosted.enabled_harnesses` (default `["claude-code"]`, so nothing changes until an operator opts in). D-138: no longer waits for P4-LLM, because `litellm` endpoints already serve `openai_chat` and `openai_responses`.

## Spec (source of truth; read, don't re-decide)
- D-122 in `docs/90-decisions.md`; SEC-66 in `docs/17-security.md`.
- `docs/12` (adapter contract, harness registry, dialect preference, refusal classification), `docs/11` (sandbox image, `SANDBOX_HARNESSES` build argument, `org.aurora.harnesses` label), `docs/10` (agent `harness`, switching bumps the session epoch, purge), `docs/05` (`/v1/harnesses`, agent PATCH, `harness_unsupported` / `harness_unavailable`), `docs/14` Instructions harness picker, `docs/15` config, `docs/16` boot document, `docs/03 §Planned changes` (`agents`, `conversations` harness rows).
- `docs/18` §Harness adapters (the tests).

## Scope
1. Supervisor: the `codex` adapter (`codex exec` JSON events → the adapter contract; `AGENTS.md` instructions; state under `CODEX_HOME` in the home; configuration rendered per start with placeholders only; `steer = false`; Codex's own sandbox and approvals off); compile-time registry; boot fails `harness_unsupported` for an unknown or missing runtime (no fallback).
2. API: `agents.harness` validated against `hosted.enabled_harnesses`; `GET /v1/harnesses`; dialect selection = the first of the harness's list that the endpoint serves; model/dialect mismatch dropped with a notice; switching bumps every conversation's session epoch, purges old sessions, refused during a turn (409); placement refuses a runner without the harness (`harness_unavailable`).
3. Runner: read `org.aurora.harnesses` from the image (absent = `claude-code,fake`).
4. Sandbox image: Codex CLI pinned (version + checksum), `SANDBOX_HARNESSES` build argument, the label.
5. Web: the Instructions harness picker per `14` (shown only when more than one harness is enabled).
6. Contracts: write `docs/API_CONTRACTS.md` entries **C-420..C-429** only (section "Phase 4 — P4-HAR"). OpenAPI + `schema.d.ts` regenerated. Operator doc (`docs/operator/runners.md` or a new page) on enabling Codex.

## Acceptance
- Every test listed in `18` for this item (conformance for `codex` against the mock Responses upstream, rendering golden tests with no secret, boot refusal, placement refusal, switch test, e2e-hosted SEC-01 probe on a Codex sandbox — ask the coordinator before `hosted.sh`); SEC-66 row `proven`.
- fmt, clippy `-D warnings`, supervisor/runner/api tests (full `aurora-api` with `--test-threads=8`), web checks for the picker, guards, OpenAPI drift test.

## Constraints
- No credential in rendered configuration or environment (placeholders only); all model traffic through the per-session proxy.
- Disk: the sandbox image grows; `df -h /home` before image builds, stop below 50 GB.
- Change files only with the Edit/Write tools; never python, sed, awk, perl, `cat >>` or heredocs. No `scripts/perf/*`.
