# TEST-012: e2e-gateway — M2 acceptance (black box: real API + gateway + Postgres + mock upstreams)

## Linked requirements
- `19 §M2` acceptance: "e2e-gateway: a BYO client calls a mock LiteLLM endpoint and a mock API through the gateway with injected keys; block, default-deny, rate limit and approval (approve/deny/expire) flows; guidance codes; SEC-03..08, 10–12, 28 … Two gateway replicas share approvals and invalidations"; governance SEC-35..39 already proven at service level (GW-005, TEST-020) — add one black-box `sanctioned_only` + foreign-key scenario here. `18 §Layers` End-to-end: gateway.

## Design
- Crate `tests/e2e-gateway` (add to workspace members — orchestrator-approved edit of root `Cargo.toml` `members` only). Tests spawn the **real binaries** (`target/debug/aurora-api`, `target/debug/aurora-gateway` ×2 on different ports) as child processes against a fresh database (`aurora_testkit::TestDb` URL), with a generated config (temp TOML + env: keyring, signing key, internal secret, `gateway.allowed_private = ["127.0.0.1/32"]`, `gateway.ca_dir` with a CA from `aurora-gateway generate-ca`, LLM endpoint `litellm` → wiremock base URL, `tls.extra_ca_bundle` for HTTPS mocks if used).
- Drive everything through **public interfaces only**: API over HTTP (sign up via `/v1/auth/signup` in `open` mode, create workspace/BYO agent, store secrets/LLM key, grant, edit + publish policy, decide approvals) and the proxy port with `Proxy-Authorization` (agent token / context credential), HTTP client = reqwest with proxy + gateway CA. Mock upstreams with wiremock (HTTP is fine via absolute-form; at least one HTTPS via CONNECT with an rcgen upstream cert trusted via `tls.extra_ca_bundle`).
- Scenarios: (1) LiteLLM key injected as Bearer, agent `x-api-key` stripped; (2) generic secret injected on mock API; (3) block rule → 403 `blocked_by_policy`; (4) default `block` → `blocked_by_default_policy`; (5) rate limit 2/min → 429; (6) require-approval: approve via API → 200, deny → 403, expire (short `approvals.timeout_secs`) → 403; decision on API while the request is held on **gateway B** works; (7) invalidation: publish a block rule while gateway A has a cached allow → next request blocked without waiting; (8) context credential via `POST /v1/agent/context-credentials` → approval row carries `conversation_id`=null/label and request log attribution; (9) secret value never appears in any API response or request-log row (SEC-01/12 spot check); (10) `sanctioned_only` org governance: agent-supplied LLM key refused (`llm_key_required`).
- `scripts/e2e/gateway.sh` wrapper (build binaries, run `cargo test -p aurora-e2e-gateway -- --test-threads=1`).

## Acceptance
- [ ] All scenarios pass locally (paste summary); processes and DB cleaned up on failure.
- [ ] No production-code changes; report defects found with evidence.

## Dependencies: BE-030 (approvals API), BE-031..033 (done), GW-001..011 (done). Agent: rust-developer (Sonnet).
