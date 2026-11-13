# 02 — Engineering Conventions

This document tells implementers *how* to build Aurora: repository layout,
crates, libraries, coding rules and the registry of constants.

## Repository layout (implementation repository `aurora/`)

```
aurora/
  Cargo.toml                  # cargo workspace; shared lints and dependency versions
  rust-toolchain.toml         # pinned stable toolchain; wasm32-wasip2 target listed
  crates/
    aurora-core/              # ids, time, error envelope, sanitizers, limits registry, token gen/hash
    aurora-config/            # layered config loading (TOML file + AURORA_* env), typed per service
    aurora-db/                # sqlx pool, migrations (embedded), repositories, claim helpers, NOTIFY bus
    aurora-crypto/            # AEAD envelope, keyring, HKDF purpose keys, ed25519 signing/verify
    aurora-secrets/           # SecretStore trait, store registry, SecretRef, inline store (+ vault in phase 3)
    aurora-policy/            # host/path matchers, GraphQL classifier, rule evaluator, injection selection
    aurora-registry/          # plugin manifest types, validator/linter, package format, snapshot compiler + types
    aurora-llm/               # LLM provider registry (litellm, anthropic, …)
    aurora-blobstore/         # (phase 5 (D-138, moved from phase 4), P4-S3) `BlobStore` trait with the `postgres` and `s3` backends, startup probe, orphan listing; `aurora-api blobs migrate` uses it
    aurora-counters/          # (phase 4, P4-CNT) `CounterStore` trait, `memory` and `redis` backends, breaker/fallback wrapper; the `postgres` backend is in aurora-db
    aurora-summary/           # approval summary templates, decoders, generic redacting summarizer
    aurora-protocol/          # every wire contract (runner, supervisor, connector, hookhost, gateway-internal) as serde types + JSON Schemas
    aurora-channels/          # neutral content, card schema, action tokens, capability model
    aurora-client/            # typed HTTP client for the management API (used by CLI, tests)
    aurora-hook-wit/          # WIT package `aurora:plugin@1.0.0` (source of truth for hook bindings)
    aurora-plugin-sdk/        # guest-side SDK for plugin authors (wasm32-wasip2): bindings, fetch/log helpers, test utils
    aurora-connector-sdk/     # helper crate for connector authors: contract client, register/config/ops loops, conformance runner
    aurora-testkit/           # shared test utilities: ephemeral Postgres, mock upstreams, mock OAuth server, fixtures
  services/
    aurora-api/               # bin: control plane (subcommands: serve, migrate, bootstrap-admin, secrets, plugins, openapi; phase 2, RFE-007: keys {generate,status}; phase 4, P4-S3: blobs migrate)
    aurora-gateway/           # bin: gateway (subcommands: serve, generate-ca, check-snapshot)
    aurora-hookhost/          # bin: hook host
    aurora-runner/            # bin: runner (backends: podman, kubernetes, fake)
    aurora-supervisor/        # bin: in-sandbox supervisor (subcommands: run, mcp-bridge)
    aurora-cli/               # bin `aurora`: login, run, agents, approvals, plugin {build,validate,compile,test,sign,docs}
  connectors/
    fake/                     # bin: aurora-connector-fake (phase 0, tests and conformance)
    mattermost/               # bin: aurora-connector-mattermost (phase 1)
  plugins/
    fixtures/                 # phase 0 fixture packages used by tests (see 09 §Fixture packages)
    builtin/                  # phase 2: real app packages
  web/                        # Next.js dashboard (pnpm)
  images/
    sandbox/                  # Containerfile for the agent sandbox image (base + app layer)
    services/                 # Containerfiles for service images
  deploy/
    dev/                      # host-process dev config, kind dev flow (kind-dev.sh), LiteLLM/VidaiMock mock (phase 1, OPS-017)
    kind/                     # kind cluster configs and Helm values profiles (e2e and dev)
    helm/aurora/              # Helm chart
  tests/
    e2e-gateway/              # black-box gateway suite
    e2e-hosted/               # black-box hosted-agent suite (real runner + podman)
    conformance/              # harness, connector and plugin conformance suites
  docs/                       # operator and developer docs (generated parts included)
```

Dependency rule: `services/*` depend on `crates/*`; crates never depend on
services. `aurora-protocol`, `aurora-core` and `aurora-channels` MUST compile to
`wasm32` where marked (they are shared with the plugin SDK only through
`aurora-hook-wit`, so in practice only `aurora-plugin-sdk` targets wasm).
`aurora-policy` MUST NOT depend on `aurora-db` (it evaluates in-memory inputs);
loaders live in `aurora-db`.

## Libraries (pinned choices)

| Concern | Crate(s) |
|---------|----------|
| Async runtime | `tokio` (multi-thread) |
| HTTP server | `axum` (+ `tower`, `tower-http`) |
| HTTP client / proxy internals | `hyper` 1.x, `hyper-util`, `reqwest` (non-hot-path clients only) |
| TLS | `rustls`, `tokio-rustls`, `rustls-native-certs`; certificate minting `rcgen` |
| DNS (guarded resolver) | `hickory-resolver` |
| Database | `sqlx` (Postgres, compile-time checked queries, `migrate`) |
| Serialization | `serde`, `serde_json`, `serde_yaml` (manifests only) |
| JSON Schema | `schemars` (generate), `jsonschema` (validate at receivers where serde alone is insufficient) |
| OpenAPI | `utoipa` (+ `utoipa-axum`); the API serves `/v1/openapi.json` |
| WebSocket | `tokio-tungstenite` |
| Crypto | `aws-lc-rs` (AES-256-GCM, HKDF, HMAC, SHA-256), `ed25519-dalek` (package/manifest signatures), `argon2` (passwords; phase 0 only, removed with password sign-in in phase 1, RFE-004) |
| OIDC | `openidconnect` |
| WebAssembly | `wasmtime` (component model, epoch interruption), `wit-bindgen` (guest SDK) |
| Kubernetes | `kube`, `k8s-openapi` |
| Podman | `bollard` against Podman's Docker-compatible API socket |
| Cron | `croner`; timezones `chrono-tz`; time `chrono` (UTC everywhere internally) |
| AWS signing (mechanism) | `aws-sigv4` |
| GraphQL classification | `graphql-parser` |
| Regex (path_regex recipes, validated, size-limited) | `regex` |
| Public suffix list (zone validation) | `psl` |
| Concurrency primitives | `arc-swap` (snapshot swaps), `dashmap`/`moka` (caches) |
| MIME decoding (summaries) | `mail-parser` |
| Postgres wire (phase 2 DB lane) | `pg_query` (statement classification), hand-written protocol framing |
| Shared counters (phase 4, P4-CNT) | `redis` (async, `tokio-rustls`; cargo feature `redis` of `aurora-counters`, off unless the backend is built in); the Postgres backend uses `sqlx` |
| LLM providers (phase 5 (D-138, moved from phase 4), P4-LLM) | SigV4 through the existing `aws-sigv4`; GCP service-account JWT (RS256) signed with `aws-lc-rs`/`rsa`, token exchange with `reqwest`; no cloud SDK (`aws-sdk-*`, `google-cloud-*`) in `aurora-llm` or the gateway |
| Blob store (phase 5 (D-138, moved from phase 4), P4-S3) | `aws-sdk-s3` + `aws-config` (provider chain), `default-features = false` with rustls; crate `aurora-blobstore` |
| Logging / tracing | `tracing`, `tracing-subscriber` (JSON), `opentelemetry-otlp` (optional) |
| Metrics | `metrics` + `metrics-exporter-prometheus` |
| Errors | `thiserror` in crates; `anyhow` only in `main` and tests |
| CLI | `clap` (derive) |
| Tests | `cargo nextest`, `insta` (snapshots), `wiremock`, `proptest` (matchers, sanitizers) |

Dashboard: Next.js (App Router), TypeScript `strict`, pnpm, Tailwind CSS,
shadcn/ui components, TanStack Query, `openapi-typescript` + `openapi-fetch`
generated from the API's OpenAPI document, Playwright for end-to-end tests.

## Coding rules

- `#![forbid(unsafe_code)]` in every crate except where a dependency boundary
  requires otherwise (none expected in phase 0). `clippy -D warnings`, `rustfmt`.
- **Secret material** uses a `Secret<T>` wrapper (zeroize on drop, redacted
  `Debug`/`Display`, no `Serialize`). Converting to bytes requires an explicit
  `expose()` call; grep for `expose()` is a review checklist item.
- **No panics on input.** All wire input paths return typed errors.
- **Every wire type** lives in `aurora-protocol` (or `aurora-registry` for
  manifests/snapshots), derives `Serialize`, `Deserialize`, `JsonSchema`, and
  uses `#[serde(deny_unknown_fields)]` only where the spec says the vocabulary
  is closed. Enumerations that are open vocabularies are modelled as
  `enum Known { … } + Other(String)` with bounded string length.
- **Database access** only through `aurora-db` repositories; SQL lives next to
  repositories; every multi-row lifecycle step is a single statement or a
  short transaction that never spans a network call.
- **Time**: the database clock (`now()`) is truth for anything stored. Services
  never write their own wall-clock into lifecycle columns.
- **Configuration**: see `15 §Configuration`. Every service validates its
  configuration completely at startup and refuses to start on any error.
- **Logging**: structured JSON; every log line carries `service`, `instance`,
  and when known `org_id`, `workspace_id`, `agent_id`, `conversation_id`,
  `turn_id`, `request_id`. URLs are logged only through the redactor (query
  values masked when the key looks credential-like: `token`, `key`, `secret`,
  `password`, `sig`, `code`, `auth*`).

## Error envelope (all HTTP surfaces, including gateway guidance)

```json
{ "error": { "code": "snake_case_code", "message": "Human/agent readable sentence.", "details": { } } }
```

- `code` is an open vocabulary string (≤64 chars, `[a-z0-9_]`).
- `details` is optional, code-specific, documented with the code.
- Validation errors use `code = "invalid_request"` and
  `details.fields = [{ "path": "/name", "message": "…" }]`.
- Status vocabulary: 400 validation, 401 unauthenticated, 403 forbidden,
  404 not found, 409 conflict, 410 retired, 413 too large, 422 domain-rule
  violation, 429 rate limited (with `retry-after`), 502/503/504 upstream or
  dependency failures.
- Definite verdicts from the gateway carry `x-should-retry: false`.

## Identifiers and tokens

- Row ids: UUIDv7, generated by the application (so AAD for encryption can be
  computed before insert).
- Slugs: `[a-z0-9][a-z0-9-]{1,47}`; handles (agent): `[a-z][a-z0-9-]{0,39}`.
- Capability tokens: see `00 §Token prefixes`. Generation:
  `prefix + base64url_nopad(32 bytes from OS CSPRNG)`. Storage: `sha256(token)`
  as `bytea` plus a 12-char display prefix. Comparison by hash lookup only.

## Untrusted text

`aurora-core::sanitize` provides the only sanitizers. Use them whenever text from
a model, user, plugin, connector or upstream is placed into platform voice,
cards, one-line statuses, labels, notices or logs.

| Function | Behaviour |
|----------|-----------|
| `one_line(s, max)` | Strip C0/C1 controls, bidi controls (U+202A–202E, U+2066–2069, U+200E/F, U+061C), invisible format chars (Unicode `Cf` incl. zero-width), collapse whitespace to single spaces, trim, clamp to `max` chars with `…`. |
| `block(s, max)` | As `one_line` but keeps `\n` (collapses >2 consecutive), clamps by chars. |
| `label(s)` | `one_line(s, 80)`; used for connection labels, secret names shown to agents, plugin-provided names. |
| `markdown_as_data(s, max)` | `block` + escape so that the dashboard/connector renders it as text, never as active markup (no raw HTML, links shown with their target). |

Dashboard rule: model/user/plugin text is rendered through a markdown renderer
with raw HTML disabled and link targets displayed; never with
`dangerouslySetInnerHTML`.

## Limits registry

All constants live in `aurora-core::limits` (a dependency-free module) and are
referenced by name in code and in this spec. Values marked *config* can be
overridden by the named configuration key (`15`).

| Name | Value | Config key | Used in |
|------|-------|-----------|---------|
| `GATEWAY_RESOLVE_CACHE_TTL` | 60 s | `gateway.resolve_cache_ttl_secs` | 06 |
| `GATEWAY_DB_PROBE_INTERVAL` | (phase 3, RFE-014) 5 s, probe timeout 2 s, 2 consecutive failures = unreachable | — | 06 |
| `GATEWAY_LISTEN_GRACE` | (phase 3, RFE-014) 30 s of LISTEN loss (database reachable) before the gateway reports `degraded`; reconnect backoff 0.5 s → 10 s | — | 06 |
| `REQUEST_LOG_OUTAGE_BUFFER` | (phase 3, RFE-014) 10 000 request-log rows buffered in memory during a database outage | — | 06 |
| `APPROVAL_TIMEOUT` | 300 s (the default of rules without their own timeout) | `approvals.timeout_secs` | 06, 07 |
| `GRANT_BULK_MAX` | (phase 4, FIX-020) 50 agents per credential-side bulk grant request | — | 05 |
| `APPROVAL_TIMEOUT_MIN` / `APPROVAL_TIMEOUT_MAX` | (phase 2, RFE-011a) 60 s / 3 600 s default (configurable up to 14 400 s, always < `TURN_CEILING − TURN_CEILING_WARNING`) | `approvals.max_timeout_secs` | 07 |
| `APPROVAL_NOTIFY_DELAY` / `APPROVAL_WATCHER_TTL` | (phase 2, RFE-011a) 20 s / 45 s | — | 07 |
| `APPROVAL_EMAIL_PER_USER_HOUR` | (phase 2, RFE-011a) 20 e-mails per user per hour (rest digested) | — | 07 |
| `APPROVAL_BODY_PEEK` | 16 KiB | — | 06 |
| `CONDITION_BODY_BUFFER` | 256 KiB (4 KiB–8 MiB) | `gateway.condition_body_bytes` | 07 |
| `UPSTREAM_HEADER_TIMEOUT` | 300 s | `gateway.upstream_header_timeout_secs` | 06 |
| `WEBSOCKET_IDLE_TIMEOUT` | 600 s | — | 06 |
| `KEYRING_MAX` | (phase 2, RFE-007) 4 entries per signing-key / internal-secret ring | — | 08 §4 |
| `FREEZE_REPLY_INTERVAL` | (phase 2, RFE-006) 10 min per (space/thread, person) | — | 04, 13 |
| `OIDC_AUTH_RATE` | (phase 1, RFE-004) 20 / min / IP for each of OIDC `start` and `callback` | — | 04 |
| `PROXY_TLS_LEAF_VALIDITY` | (phase 1, RFE-002) 30 days; renewed when < 10 days remain | — | 06 |
| `BYO_CIDR_MAX` | (phase 2, RFE-025) 32 CIDRs per agent / org / instance list | — | 08 §11.9 |
| `AGENT_TOKEN_MAX_AGE_MIN` / `_MAX` | (phase 3, RFE-013) 1 / 3 650 days (optional agent-token maximum age; default none) | `governance.agent_tokens.max_age_days` | 08 §7 |
| `AGENT_TOKEN_RETROFIT_GRACE` | (phase 3, RFE-013) 7 days minimum remaining life for existing tokens when a maximum age is introduced or tightened | — | 08 §7 |
| `AGENT_TOKEN_STALE_DAYS` / `AGENT_TOKEN_EXPIRING_DAYS` | (phase 3, RFE-013) 30 days unused / 14 days to expiry | — | 08 §7, 14 |
| `BYO_REFUSAL_LOG_INTERVAL` | (phase 2, RFE-025) one log line and one request-log row per credential per minute for gateway BYO refusals | — | 06 |
| `REQUEST_LOG_FLUSH` | 5 s or 500 rows | — | 06 |
| Request-log retention | 30 days (key default; rows are deleted when older than the period, checked daily: aged rows of the oldest kept monthly partition and of the default partition are deleted in batches of 10 000, whole partitions past the period are dropped) | `retention.request_log_days` | 15 |
| `WEBHOOK_DELIVERY_RETENTION` | (FIX-111) 30 days, and at most 10 000 rows per webhook (`WEBHOOK_DELIVERY_MAX_PER_WEBHOOK`), removed by the daily retention pass | — | 05, 10 |
| `SESSION_PRUNE_GRACE` | (FIX-111) 7 days after expiry or revocation before the daily retention pass removes a session or an expired context credential | — | 03, 05 |
| `RETENTION_PRUNE_BATCH` | (FIX-111) 1 000 rows per statement in the daily retention pass (`SKIP LOCKED` batches; no long lock) | — | 05 |
| `REACH_EXPIRES_IN` | (FIX-111) `expires_in` on a reach-grant approval: 60 s to 365 days (seconds); without it a grant does not expire | — | 05, 13 |
| `SANDBOX_START_LOG_MAX` | (phase 4, FIX-029, C-612) 30 entries and 8 KiB per boot's start log ring (`sandboxes.start_log`; oldest dropped, detail ≤ 300 chars) | — | 11 |
| `BATCH_HEADER_MAX` | 128 chars | — | 06 |
| `CONNECTION_CHOICE_MAX` | 20 entries | — | 06 |
| `REFRESH_MARGIN` | 60 s | — | 08 |
| `LLM_ORIGIN_RECHECK` | 1 h | `governance.llm.origin_recheck_secs` | 08 §11 |
| `LLM_ORIGIN_CHECK_TIMEOUT` | 10 s | — | 08 §11 |
| `EGRESS_ALLOWLIST_MAX` | 500 entries per org / workspace | — | 07, 08 §11 |
| `REFRESH_LOCK_WAIT` | 10 s | — | 08 |
| `OAUTH_STATE_TTL` | 10 min | — | 08, 09 |
| `IDLE_STOP` | 30 min | `hosted.idle_stop_secs` | 11 |
| `BACKGROUND_CAP` | 12 h | `hosted.background_cap_secs` | 11 |
| `TURN_CEILING` | 6 h | `hosted.turn_ceiling_secs` | 10 |
| `TURN_CEILING_WARNING` | 15 min | — | 10 |
| `TURN_STALL` | 10 min | `hosted.turn_stall_secs` | 10 |
| `TURN_PROGRESS_INTERVAL` | 60 s | — | 12 |
| `STALE_DISPATCH` | 90 s | — | 10 |
| `TOOL_CALL_TIMEOUT` | 25 s | — | 12 |
| `TOOL_CONSECUTIVE_FAILURES` | 2 | — | 12 |
| `WS_FRAME_MAX` | 256 KiB | — | 16 |
| `HOME_SYNC_PART_MAX` | 200 KiB (UTF-8 bytes) | — | 12 |
| `CONTEXT_CREDENTIAL_GRACE` | 5 min | — | 08 |
| `TURN_TEXT_MAX` | 100 000 chars | — | 10 |
| `MEMORY_MAX_PER_AGENT` | 100 | — | 10 |
| `MEMORY_REVISIONS_KEPT` | 50 | — | 10 |
| `MEMORY_TOOL_CONTENT_MAX` | 12 000 chars | — | 10 |
| `MEMORY_CONTENT_MAX` | 100 000 chars | — | 10 |
| `MEMORY_INDEX_MAX` | 6 000 chars | — | 10 |
| `MEMORY_SNIPPETS` | 3 × 800 chars, score ≥ 0.1 | — | 10 |
| `MEMORY_SEARCH_HITS` | 8 | — | 10 |
| `SKILL_NAME_MAX` / `SKILL_DESCRIPTION_MAX` | 64 / 500 chars | — | 10 |
| `SKILL_BODY_MAX` / `SKILL_TOTAL_MAX` | 24 000 / 32 000 chars | — | 10 |
| `ATTACH_IN_FILE_MAX` / `ATTACH_IN_COUNT` | 10 MiB / 10 per message | — | 10 |
| `ATTACH_OUT_FILE_MAX` / `ATTACH_OUT_COUNT` | 25 MiB / 10 per turn | — | 10 |
| `ATTACH_OUT_DAILY_PER_CONVERSATION` | 200 MiB | — | 10 |
| `ATTACH_CAPTION_MAX` | 500 chars | — | 10 |
| `ATTACH_PENDING_TTL` | 24 h | — | 10 |
| `LLM_SIGN_BODY_MAX` | (phase 5 (D-138, moved from phase 4), P4-LLM) 8 MiB request body buffered to SigV4-sign a Bedrock request (larger → 413 `llm_request_too_large`) | — | 06, 08 |
| `LLM_TOKEN_MINT_TIMEOUT` / `LLM_TOKEN_SKEW` | (phase 5 (D-138, moved from phase 4), P4-LLM) 10 s per OAuth2 token mint (Vertex) / tokens cached until 300 s before expiry, at most 55 min | — | 08 |
| `S3_UPLOAD_CONCURRENCY` | (phase 5 (D-138, moved from phase 4), P4-S3) 16 uploads in flight per API replica (1–64); wait ≤ 10 s then 503 `blob_store_busy` | `blobs.s3.upload_concurrency` | 10, 15 |
| `S3_CONNECT_TIMEOUT` / `S3_REQUEST_TIMEOUT` / `S3_RETRIES` | (phase 5 (D-138, moved from phase 4), P4-S3) 5 s / 60 s (idle body 30 s) / 2 retries | — | 15 |
| `S3_ORPHAN_GRACE` / `S3_ORPHAN_SWEEP_MAX` | (phase 5 (D-138, moved from phase 4), P4-S3) 2 h minimum object age before the sweep may delete an unreferenced object / 50 000 keys per org per run | — | 10 |
| `S3_PROBE_INTERVAL` | (phase 5 (D-138, moved from phase 4), P4-S3) 30 s | — | 05, 15 |
| `ATTACH_ORG_QUOTA_DEFAULT` / `ATTACH_ORG_QUOTA_MAX` | (phase 2, RFE-015) 10 GiB / 50 GiB per org (`0` = unlimited) | `attachments.default_org_quota_mib` / `attachments.org_quota_mib_max` | 10 |
| `ATTACH_QUOTA_NOTICE` | (phase 2, RFE-015) notices at 80 % and 100 % of the org quota | — | 10 |
| `RETENTION_MIN_DAYS` / `RETENTION_MAX_DAYS` | (phase 2, RFE-010) 7 / 3 650 days (org conversation and attachment windows) | `retention.conversation_days_max` caps the upper bound | 10 |
| `RETENTION_SWEEP_BATCH` | (phase 2, RFE-010) 200 conversations per transaction, ≤ 5 000 per org per run | — | 10 |
| `EXPORT_MAX` | (phase 2, RFE-010) 256 MiB and 200 000 events per export | — | 10 |
| `SCHEDULE_FIRE_LEASE` | 5 min | — | 10 |
| `SCHEDULE_FAILURE_DISABLE` | 5 consecutive | — | 10 |
| `GATE_TIMEOUT` | 30 s | — | 12 |
| `WEBHOOK_PAYLOAD_MAX` | 1 MiB | — | 10 |
| `PROCESS_TAIL` | 4 KiB | — | 12 |
| `WATCH_PATTERN_MAX` | 256 chars | — | 10 |
| `RUNNER_POLL_WAIT` | 25 s | — | 11 |
| `REPLICA_ONLINE_THRESHOLD` | 90 s | — | 11, 13 |
| `RECONCILE_INTERVAL` | 60 s | `runner.reconcile_interval_secs` | 11 |
| `ORPHAN_GRACE` | 1 h | `runner.orphan_grace_secs` | 11 |
| `SANDBOX_DEFAULTS` | 2 GiB RAM, 1 CPU, 512 PIDs, 10 GiB home | `runner.sandbox_*` | 11 |
| `RUNNER_MAX_SANDBOXES` | 4 | `runner.max_sandboxes` | 11 |
| `CONNECTOR_OPS_WAIT` | 25 s | — | 13 |
| `CONNECTOR_HOST_STALE` | (phase 4, P4-MCON) 90 s without a host `register` heartbeat: the host leaves the catalogue and `host_running`; assignments long poll ≤ 30 s; managed settings ≤ 16 KiB, secret value ≤ 8 KiB, host status message ≤ 300 chars | — | 13 |
| `INBOX_LEASE` / `OP_LEASE` | 60 s / 60 s | — | 13 |
| `OP_MAX_ATTEMPTS` | 10 (exponential backoff 2 s → 10 min) | — | 13 |
| `CHANNEL_LEDGER_RETENTION` | 7 days | — | 13 |
| `DEPROVISION_RETRY_WINDOW` | (phase 3, RFE-024) 24 h of retries for a self-contained `presence.deprovision` op (then `failed`, residual) | — | 13 |
| `ACTION_TOKEN_TTL` | approval deadline, else 24 h | — | 13 |
| `APP_TURN_CAP` | 5 consecutive bot-authored turns | — | 07 |
| `PEER_TASK_BUDGET` / `PEER_TASK_TTL` | 20 messages / 24 h | — | 07 |
| `SESSION_IDLE` / `SESSION_ABSOLUTE` | 14 days / 30 days | `auth.session_*` | 04 |
| `HOOK_MEMORY` / `HOOK_OUTPUT` | 64 MiB / 256 KiB | — | 09 |
| `HOOK_WALL` | 15 s (connect, mint) / 1 s (`approval.summarize`) | — | 09 |
| `HOOK_FETCH_MAX` | 16 calls, 1 MiB response each, 10 s each | — | 09 |
| `SUMMARY_LIMITS` | title 120, ≤12 fields, value 200, text 1 000 chars | — | 09 |
| `PACKAGE_MAX` | 20 MiB archive, icon PNG ≤ 256 KiB, setup guide ≤ 64 KiB | — | 09 |
| `AGENT_SNIPPET_MAX` | 2 000 chars per app | — | 09 |
| `AVATAR_MAX` | 512 KiB (PNG/JPEG/WebP) | — | 10 |
| `SSE_KEEPALIVE` | 15 s | — | 05 |
| `API_RATE_*` | (phase 2, RFE-017) buckets of `05 §Conventions`: general 600/min (burst 120), write 120/min (30), message 30/min (10), upload 20/min (5), simulate 20/min (5), search 30/min (10), export 6/h (2), org 3 000/min (600) | `ratelimit.api.*` | 05 |
| `API_RATE_UNAUTH` | (phase 2, RFE-017) 120 / min / IP public GETs; 30 / min / IP for requests with a rejected bearer | — | 05 |
| `COUNTER_TIMEOUT` | (phase 4, P4-CNT) 50 ms per shared-counter store call (10–2000 ms) | `ratelimit.counters.timeout_ms` | 05, 06, 15 |
| `COUNTER_BREAKER_FAILURES` / `COUNTER_BREAKER_COOLDOWN` | (phase 4, P4-CNT) 3 consecutive failures open the breaker; one probe after 5 s | — | 06, 15 |
| `COUNTER_PRUNE_INTERVAL` / `COUNTER_PRUNE_BATCH` | (phase 4, P4-CNT) 60 s / 5 000 rows (`postgres` backend) | — | 03, 15 |
| `GROUPS_PER_ORG` / `GROUP_MEMBERS_MAX` | (phase 4, P4-GRP-1) 500 groups per org / 5 000 members per group (422 `group_limit_reached` / `group_member_limit_reached`; SCIM: 400 `invalidValue` with a `detail`, `04 §SCIM`) | — | 04, 05 |
| `GROUPS_PER_USER` / `GROUP_BINDINGS_PER_GROUP` | (phase 4, P4-GRP-1) 100 groups per user / 200 workspace bindings per group | — | 04 |
| `GROUP_NAME_MAX` / `GROUP_DESCRIPTION_MAX` | (phase 4, P4-GRP-1) 80 / 280 chars | — | 03, 04 |
| `RULE_GROUP_IDENTITIES_MAX` | (phase 4, P4-GRP-1) 20 `group` identities per policy rule | — | 07 |
| `GROUP_IMPACT_LIST_MAX` | (phase 4, P4-GRP-1) 200 users listed in a group impact view (the total is always given) | — | 05 |
| `ACCESS_NOTIFY_BULK` | (phase 4, P4-GRP-1) above 1 000 users changed in one transaction a single org-wide `access` notification replaces per-user ones (**not implemented** in phase 4, FIX-074: the constant is defined but no threshold is applied; `notify_access` takes either a user list or an org-wide `None`, and callers choose; no task is scheduled) | — | 04, 05 |
| `PARK_THRESHOLD` / `PARK_MAX_TIMEOUT` | (phase 5 (D-138, moved from phase 4), RFE-011b) 900 s: effective timeouts above it park, when parking is enabled / 86 400 s (at most 604 800) longest parked deadline | `approvals.park.threshold_secs` / `approvals.park.max_timeout_secs` | 06, 07, 15 |
| `PARK_RESUME_WINDOW` / `PARK_DENY_COOLDOWN` | (phase 5 (D-138, moved from phase 4), RFE-011b) 1 800 s a granted approval can be consumed (60–86 400) / 900 s a denied fingerprint is answered `manual_approval_denied` without a new approval | `approvals.park.resume_window_secs` / `approvals.park.deny_cooldown_secs` | 07 |
| `PARK_BODY_MAX` / `PARK_BODY_READ_TIMEOUT` | (phase 5 (D-138, moved from phase 4), RFE-011b) 256 KiB request body that may be parked (larger → held) / 10 s to read it | `approvals.park.max_body_bytes` | 06, 07 |
| `PARK_PENDING_PER_AGENT` / `PARK_PER_AGENT_HOUR` | (phase 5 (D-138, moved from phase 4), RFE-011b) 20 parked pending approvals per agent / 60 new parked approvals per agent per hour (403 `approval_limit_reached`) | `approvals.park.max_pending_per_agent` | 06, 07 |
| `APPROVAL_PUSH_DELAY` / `PUSH_PER_USER_HOUR` | (phase 5 (D-138, moved from phase 4), RFE-011b) 5 s grace before a push / 30 pushes per user per hour (rest digested) | — | 07 |
| `PUSH_SUBSCRIPTIONS_PER_USER` / `PUSH_TEST_PER_HOUR` | (phase 5 (D-138, moved from phase 4), RFE-011b) 10 devices per user / 3 test pushes per user per hour | — | 05, 07 |
| `PUSH_TTL_MAX` / `PUSH_PAYLOAD_MAX` | (phase 5 (D-138, moved from phase 4), RFE-011b) 24 h push-service TTL cap (≥ 60 s) / 1 KiB payload before encryption | — | 07, 16 |
| `PUSH_VAPID_RETIRE` | (phase 5 (D-138, moved from phase 4), RFE-011b) 30 days a retired VAPID key stays usable for the subscriptions that reference it | — | 07, 08 |
| `SCIM_RATE` | (phase 4, P4-GRP-2) 600 requests / min per token, burst 120 (class `management`; `scim.rate_per_minute`) | `scim.rate_per_minute` | 04, 05, 16 |
| `SCIM_BODY_MAX` / `SCIM_MAX_PATCH_OPS` / `SCIM_MAX_MEMBERS_PER_REQUEST` | (phase 4, P4-GRP-2) 1 MiB / 100 operations / 1 000 members | — | 04, 16 |
| `SCIM_PAGE_MAX` / `SCIM_PAGE_DEFAULT` | (phase 4, P4-GRP-2) 100 / 50 resources per page | — | 16 |
| `SCIM_TOKENS_PER_ORG` / `SCIM_TOKEN_DEFAULT_DAYS` / `SCIM_TOKEN_MAX_DAYS` | (phase 4, P4-GRP-2) 5 active / 365 days / 730 days (no non-expiring token) | `scim.token_max_days` | 04, 15 |
| `SCIM_ERROR_LOG` | (phase 4, P4-GRP-2) last 20 sanitized errors kept per org for the Directory sync page | — | 14 |
| `PREVIEW_SHARES_PER_AGENT` / `PREVIEW_SHARES_PER_ORG` | (phase 4, P4-PRV-1) 10 / 200 shares | `previews.shares_per_agent`, `previews.shares_per_org` (lower only) | 11, 05 |
| `PREVIEW_START_COMMAND_MAX` | (phase 4, P4-PRV-1) 1 024 chars, one line | — | 11 |
| `PREVIEW_WAKE_WINDOW` / `PREVIEW_WAKE_WAIT` / `PREVIEW_LISTEN_WAIT` | (phase 4, P4-PRV-1) 60 s a wake demand stays due / 45 s a browser request waits for a woken port / 5 s for a port of a running sandbox (`PREVIEW_LISTEN_WAIT` is **defined but not applied** as a wait, FIX-074: the supervisor dials loopback with a 5 s connect timeout, so a closed port is refused at once and answers 502 `preview_unreachable`) | — | 11, 05 |
| `PREVIEW_WAKES_PER_SHARE_HOUR` | (phase 4, P4-PRV-1) 12 wakes per share per hour (429 `preview_wake_limited`) | — | 11 |
| `PREVIEW_START_ATTEMPTS` / `PREVIEW_START_WINDOW` | (phase 4, P4-PRV-1) 3 start-command launches per share per 10 min | — | 11 |
| `PREVIEW_AWAKE_GRACE` / `PREVIEW_AWAKE_CAP` | (phase 4, P4-PRV-1) 120 s after the last stream activity / 4 h per boot of preview-held wakefulness | `previews.awake_cap_secs` (≤ `BACKGROUND_CAP`) | 11 |
| `PREVIEW_SESSION_TTL` / `PREVIEW_SESSION_IDLE` | (phase 4, P4-PRV-2) 8 h absolute (never past the dashboard session) / 1 h idle | `previews.session_ttl_secs`, `previews.session_idle_secs` (lower only) | 04 |
| `PREVIEW_GRANT_TTL` / `PREVIEW_STREAM_GRANT_TTL` | (phase 4, P4-PRV-2) 60 s single-use authorization grant / 30 s single-use stream grant | — | 04, 16 |
| `PREVIEW_AUTH_CACHE` | (phase 4, P4-PRV-2) 15 s positive, 5 s negative relay cache of a resolve; also the open-stream revalidation interval | — | 04, 15 |
| `PREVIEW_STREAMS_PER_SHARE` / `_PER_SANDBOX` / `_PER_USER` | (phase 4, P4-PRV-2) 32 / 64 / 64 concurrent streams; the per-share and per-user values are enforced per relay replica (429 `preview_streams_limit` or close 4409, FIX-072, C-636), the per-sandbox one in the supervisor | `relay.max_streams_per_user` (lower only) | 16 |
| `PREVIEW_STREAM_WINDOW` / `PREVIEW_CHUNK` | (phase 4, P4-PRV-2) 128 KiB credit window per direction per stream / 16 KiB data chunk | — | 16 |
| `PREVIEW_IDLE_TIMEOUT` / `PREVIEW_WS_IDLE_TIMEOUT` / `PREVIEW_STREAM_LIFETIME` | (phase 4, P4-PRV-2) 120 s plain HTTP stream / 600 s upgraded stream / 4 h | — | 16 |
| `TERMINAL_GRANT_TTL` / `TERMINAL_CHUNK` / `TERMINAL_STREAM_WINDOW` | (phase 4, P4-TERM) 30 s single-use `auy_` grant / 16 KiB PTY data chunk / 128 KiB credit window per direction per PTY | — | 16 |
| `TERMINALS_PER_SANDBOX` / `_PER_AGENT` / `_PER_USER` | (phase 4, P4-TERM) 2 / 4 / 2 concurrent terminal sessions (per user per API replica) | — | 16 |
| `TERMINAL_IDLE_TIMEOUT` / `TERMINAL_MAX_LIFETIME` | (phase 4, P4-TERM) 600 s idle (WebSocket pings count) / 4 h per session | — | 16 |
| Stream-listener bounds | (FIX-092) request head within 10 s in total; 256 unauthenticated connections per runner replica; live legs per sandbox 64 streams + 2 PTYs; browser text frame ≤ 256 bytes; stalled write 30 s; runner outbound queue 512 frames per supervisor connection; `pty.resize` coalesced to the latest size | — | 16 |
| `TERMINAL_AWAKE_GRACE` / `TERMINAL_AWAKE_CAP` | (phase 4, P4-TERM) 120 s after the last session byte / 4 h per boot of terminal-held wakefulness | `terminal.awake_cap_secs` (≤ `BACKGROUND_CAP`) | 11 |
| `TERMINAL_AUTH_CACHE` / `TERMINAL_WAKE_WINDOW` / `TERMINAL_WAKES_PER_AGENT_HOUR` | (phase 4, P4-TERM) 15 s revocation bound (freeze/disable closes sessions; implemented as the API's 10 s access re-check `ACCESS_RECHECK` in `terminal/service.rs`, so sessions close within 10 s and the 15 s constant is never reached, FIX-074) / 60 s wake demand window / 12 wakes per agent per hour | — | 11 |
| `PREVIEW_CONNECT_TIMEOUT` / `PREVIEW_HEADER_TIMEOUT` | (phase 4, P4-PRV-2) 10 s to open a stream (supervisor connect 5 s) / 60 s for upstream response headers | — | 16 |
| `PREVIEW_REQUEST_BODY_MAX` | (phase 4, P4-PRV-2) 100 MiB (the ceiling and the binary's default; the Helm chart ships 32 MiB, FIX-074) | `relay.request_body_max_bytes` (lower only) | 15, 16 |
| `PREVIEW_BYTES_PER_SHARE_DAY` | (phase 4, P4-PRV-2) 5 GiB per share per UTC day, one shared counter across relay replicas (429 `preview_quota` with `Retry-After` to the next UTC midnight; FIX-072, C-637, `16 §W18`) | `previews.bytes_per_share_day` | 16 |
| `PREVIEW_API_RATE` | (phase 4, P4-PRV-2) 100 relay-plane requests/s per relay replica, burst 200 | — | 16 |
| `PREVIEW_GRANT_RATE` | (phase 4, P4-PRV-2) 30 grant redemptions per minute per client address (and 10 per minute per share) at the relay, per replica, fixed window (429 `grant_rate_limited`; FIX-072, C-636) | — | 04 |
| `LIBRARIES_PER_ORG` | (phase 4, P4-GSL) 10 skill libraries | — | 10, 05 |
| `LIBRARY_BYTES_PER_AGENT` / `_SKILLS_PER_AGENT` | (phase 4, P4-GSL) 1 MiB / 25 library-sourced skills at the agent tier (workspace tier 2 MiB / 50, org tier 4 MiB / 100: `LIBRARY_BYTES_PER_WORKSPACE`, `LIBRARY_SKILLS_PER_WORKSPACE`, `LIBRARY_BYTES_PER_ORG`, `LIBRARY_SKILLS_PER_ORG`) | `skills.library.bytes_per_agent` (lower only) | 10 |
| `LIBRARY_FILE_MAX` / `LIBRARY_TREE_MAX` / `LIBRARY_BROWSE_MAX` | (phase 4, P4-GSL; D-154: also at most 50 files per skill directory) 64 KiB per file / 20 000 tree entries (a truncated or longer tree is `library_tree_too_large`: narrow `path`) / 200 listed skills | — | 09, 10 |
| `LIBRARY_PENDING_BYTES_PER_ORG` | (phase 4, P4-GSL) 4 MiB of open proposed updates per org | — | 10 |
| `LIBRARY_REFRESH_INTERVAL` / `LIBRARY_LEASE` / `LIBRARY_REFRESH_CONCURRENCY` | (phase 4, P4-GSL) 15 min (minimum 5 min) per library / 5 min lease / 4 refreshes per API replica | `skills.library.refresh_interval_secs` (≥ 300) | 05, 10 |
| `LIBRARY_CHECK_NOW_INTERVAL` | (phase 4, P4-GSL) 60 s per library | — | 05 |
| `LIBRARY_FETCH_TIMEOUT` / `LIBRARY_FETCH_RATE` | (phase 4, P4-GSL) 20 s per fetch / 20 per second per gateway, 5 per second per org | — | 06 |
| `API_STREAMS_PER_PRINCIPAL` | (phase 2, RFE-017) 20 concurrent SSE connections + long-polls | — | 05 |
| `DELTA_COALESCE` | (phase 1, RFE-016) 100 ms or 2 000 chars per turn per delta | `events.delta_coalesce_ms` (100–500) | 05 |
| `LONG_POLL_MAX` | 30 s | — | 05 |
