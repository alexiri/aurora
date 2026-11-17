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
    aurora-blobstore/         # (phase 6 (D-318, moved from phase 5), P4-S3) `BlobStore` trait with the `postgres` and `s3` backends, startup probe, orphan listing; `aurora-api blobs migrate` uses it
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
    aurora-api/               # bin: control plane (subcommands: serve, migrate, bootstrap-admin, secrets, plugins, openapi; phase 2, RFE-007: keys {generate,status}; phase 6 (D-318), P4-S3: blobs migrate)
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
| SSH keys and certificates (phase 5, P4-SSH-1, D-239) | `ssh-key` 0.7 (RustCrypto, Apache-2.0 OR MIT; features `ed25519`, `std`, `alloc`, `rand_core`): parses OpenSSH public keys, computes `SHA256:` fingerprints, builds and signs user certificates (`certificate::Builder`) in the API, and parses them in `aurora-cli`. Pinned to the `0.7.0-rc.12` release candidate because 0.6.x would add a second `ed25519-dalek`/`sha2`/`rand_core` line beside this workspace's; revisit when 0.7 is final |
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
| Blob store (phase 6 (D-318, moved from phase 5), P4-S3) | `aws-sdk-s3` + `aws-config` (provider chain), `default-features = false` with rustls; crate `aurora-blobstore` |
| Logging / tracing | `tracing`, `tracing-subscriber` (JSON), `opentelemetry-otlp` (optional); (phase 5, FIX-146, D-232) the OpenTelemetry SDK tracer provider and W3C propagator are always installed, only the OTLP exporter is optional |
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
  `password`, `sig`, `code`, `auth*`). (phase 5, FIX-146, D-232) **`request_id` is the W3C
  trace id** (32 lowercase hex) of the current span's context, added by the JSON formatter to
  every line emitted inside a span, also when no OTLP exporter is configured; it is the
  `x-request-id` of the public API's response (C-732 (1)). Never invent a second request
  identifier. One `info` access line per request (`15 §Observability`). Span attributes and
  span events follow the SEC-88 allowlist: ids, route templates, methods, statuses, hosts,
  model names, decisions, error codes, durations, counts and parameterized SQL — never raw
  paths, query strings, headers, bodies, message text, prompts, tool arguments or results,
  secrets, tokens, e-mail addresses, display names or bind values. Calls to another Aurora
  service go through the one outgoing-call helper (it injects `traceparent` and records the
  client span); calls to third parties never carry an Aurora `traceparent`.

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
| `markdown_safe(s, max)` | (phase 5, FIX-142, D-227) controls, bidi and format characters stripped and line endings normalised (spaces, tabs and newlines kept: indentation is Markdown syntax; unlike `block` it does not collapse whitespace), then minimal edits for D-227's security exceptions only (images → links, link scheme allowlist, host after a URL-looking label of another host, HTML outside GitHub's allowlist, typed neutral tokens broken with a zero-width space, bare URLs and autolinks written `[url](url)` (D-270), `![` and `](` and definition look-alikes broken where the parser formed no image/link), verified by re-parsing (a block that fails falls back to `markdown_as_data`, counted by `markdown_fallbacks()`); everything else kept byte for byte. `broadcast_here` turns a standalone `@here` (that exact spelling, prose only, not in a link label; D-271) into `<@here>`; `markdown_chunks` splits a long answer at block boundaries. Only for an agent's own message body on a chat platform (`13 §4.3`, `Content::agent_markdown`); all other text uses `markdown_as_data`. |

Dashboard rule: (phase 5, FIX-137, D-227, D-245) model/user/plugin text is rendered
through the one Markdown renderer (`web/src/components/ui/markdown.tsx`), which
renders like github.com (CommonMark 0.31, GFM, footnotes, alerts, emoji, highlighted
code, math, GitHub's raw-HTML allowlist) except for D-227's security exceptions:
other-origin images become links, only `http`/`https`/`mailto` and same-origin
relative links are links, a URL-looking label of another host shows the real host,
raw HTML outside the allowlist is dropped (text kept; a `<script>`'s content with it),
KaTeX `trust: false`, Mermaid shown as code (D-245). Output is React elements, parse →
hast → sanitize → React; never `dangerouslySetInnerHTML`.

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
| `TUNNEL_HANDSHAKE_TIMEOUT` / `TUNNEL_HEADER_TIMEOUT` / `TUNNEL_IDLE_TIMEOUT` | (FIX-106) 10 s inner TLS handshake and open-lane dial / 30 s request head inside a tunnel / 600 s idle, every tunnel kind | — | 06 §Tunnel limits |
| `TUNNEL_PER_CREDENTIAL_MAX` / `TUNNEL_TOTAL_MAX` | (FIX-106) 64 concurrent tunnels per credential / 4 096 per gateway | — | 06 §Tunnel limits |
| `NARR_MIN_INTERVAL` | (phase 5, P4-NARR, D-280) 5 s between narration-card updates of one turn | — | 10 §Live narration cards, 13 §5.12 |
| `NARR_SPACE_UPDATES_PER_MINUTE` | (phase 5, P4-NARR, D-280) 10 narration writes per space per minute (a third of the connector's `writes_per_space_per_minute`, 30) | — | 13 §5.12, 20 |
| `NARR_CARDS_PER_SPACE` | (phase 5, P4-NARR, D-280) 3 narration cards at a time per space; a further turn gets no card | — | 13 §5.12 |
| `NARR_MAX_STEPS` | (phase 5, P4-NARR, D-280) 12 steps listed per card, then "N earlier" | — | 10 §Live narration cards |
| `NARR_STEP_CHARS` | (phase 5, P4-NARR, D-280) 120 characters per step label and per detail | — | 10, 20 |
| `USER_TOKENS_PER_PERSON` | (phase 5, P4-UTOK, D-284) 5 user tokens per person | — | 05, 13 §5.11 |
| `USER_TOKEN_PENDING_MAX` | (phase 5, P4-UTOK, D-284) 5 pending `send_as_me` approvals per token | — | 13 §5.11 |
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
| `TYPING_REFRESH_SECS` | (phase 5, FIX-158, D-319) 4 s between the Mattermost connector's `typing` calls for a running turn (clients drop the indicator after about 5 s). A refresher whose `status.clear` was lost stops after `TURN_CEILING_WARNING` | — | 13, 20 |
| `RECEIPT_ORPHAN_SECS` | (phase 5, FIX-158, D-319) 600 s: an open chat receipt of a conversation with no active turn and no pending follow-on work is cleared by the channel-op sweep | — | 13 |
| `TURN_STALL` | 10 min | `hosted.turn_stall_secs` | 10 |
| turn progress interval | **not implemented as a limit** (FIX-119): the supervisor emits progress every 10 s (`SupervisorConfig.progress_interval`, `services/aurora-supervisor/src/supervisor.rs`); the 60 s draft constant (`turn progress interval`) was never read and was removed. A configurable interval, if wanted, is a new task | — | 12 |
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
| `SKILL_PER_AGENT_MAX` / `SKILL_FILES_MAX` | 50 own skills per agent / 50 files per skill (409 `skill_limit_reached`, `skill_files_limit_reached`; checked under the agent / skill row lock; FIX-110). The other per-agent caps: 50 schedules, 20 webhooks, 100 memories | — | 10 |
| `PEER_TASK_QUEUE_MAX` | 10 queued tasks per ordered (sender, target) pair; the 11th `message_agent` is refused (FIX-110) | — | 10 |
| `ATTACH_IN_FILE_MAX` / `ATTACH_IN_COUNT` | 10 MiB / 10 per message | — | 10 |
| `ATTACH_OUT_FILE_MAX` / `ATTACH_OUT_COUNT` | 25 MiB / 10 per turn | — | 10 |
| `ATTACH_OUT_DAILY_PER_CONVERSATION` | 200 MiB | — | 10 |
| `ATTACH_CAPTION_MAX` | 500 chars | — | 10 |
| `ATTACH_PENDING_TTL` | 24 h | — | 10 |
| `LLM_SIGN_BODY_MAX` | (phase 5 (D-138, moved from phase 4), P4-LLM) 8 MiB request body buffered to SigV4-sign a Bedrock request (larger → 413 `llm_request_too_large`) | — | 06, 08 |
| `LLM_TOKEN_MINT_TIMEOUT` / `LLM_TOKEN_SKEW` | (phase 5 (D-138, moved from phase 4), P4-LLM) 10 s per OAuth2 token mint (Vertex) / tokens cached until 300 s before expiry, at most 55 min; (P4-LLM spec batch, D-261) the same for Entra tokens and STS sessions (an STS session never past its `Expiration`) | — | 08 |
| `LLM_TOKEN_MINT_FAILURE_HOLD` | (phase 5, P4-LLM spec batch, D-261) 5 s: after a failed mint the gateway answers 502 `credential_unavailable` for the same `(secret_id, value_version, mode, region)` without calling the token endpoint again | — | 06, 08 |
| `LLM_MINT_CACHE_MAX` | (phase 5, P4-LLM spec batch, D-261) 10 000 minted entries per gateway replica (least recently used evicted, then minted again on demand) | — | 06, 08 |
| `LLM_ASSUME_ROLE_DURATION` | (phase 5, P4-LLM spec batch, D-261) `aws_assume_role` session length: 900–43 200 s, default 3 600 (`session_duration_secs` in the secret value; the role's own maximum also applies) | — | 08 |
| `S3_UPLOAD_CONCURRENCY` | (phase 6 (D-318, moved from phase 5), P4-S3) 16 uploads in flight per API replica (1–64); wait ≤ 10 s then 503 `blob_store_busy` | `blobs.s3.upload_concurrency` | 10, 15 |
| `S3_CONNECT_TIMEOUT` / `S3_REQUEST_TIMEOUT` / `S3_RETRIES` | (phase 6 (D-318, moved from phase 5), P4-S3) 5 s / 60 s (idle body 30 s) / 2 retries | — | 15 |
| `S3_ORPHAN_GRACE` / `S3_ORPHAN_SWEEP_MAX` | (phase 6 (D-318, moved from phase 5), P4-S3) 2 h minimum object age before the sweep may delete an unreferenced object / 50 000 keys per org per run | — | 10 |
| `S3_PROBE_INTERVAL` | (phase 6 (D-318, moved from phase 5), P4-S3) 30 s | — | 05, 15 |
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
| `PACKAGE_MAX` | 20 MiB archive, decompressed ≤ 4 × `PACKAGE_MAX`, `plugin.yaml` ≤ 256 KiB (`PACKAGE_MANIFEST_MAX`, FIX-107), icon PNG ≤ 256 KiB, setup guide ≤ 64 KiB | — | 09 |
| `AGENT_SNIPPET_MAX` | 2 000 chars per app | — | 09 |
| `AGENT_SUMMARY_MAX` | (phase 5, P5-GUIDE-1) 160 chars, one line (`agent.summary`) | — | 24 |
| `AGENT_GUIDE_FILES_MAX` / `AGENT_GUIDE_FILE_MAX` / `AGENT_GUIDE_APP_MAX` | (phase 5, P5-GUIDE-1) 24 guide files per app / 64 KiB per file / 256 KiB per app (UTF-8 bytes) | — | 24 |
| `AGENT_GUIDE_SYNC_MAX` | (phase 5, P5-GUIDE-2) 2 MiB of package reference files and recipes projected into one agent's home per sync; the generated `guide.md`, `tools.md` and `api.json` are never dropped | — | 24, 12 |
| `AGENT_OPERATIONS_MAX` / `AGENT_OPERATIONS_FILE_MAX` | (phase 5, P5-GUIDE-6, D-296) 2 000 operations / 512 KiB per operations file | — | 24 |
| `AGENT_API_JSON_MAX` | (phase 5, P5-GUIDE-6) 512 KiB per rendered `api.json` | — | 24 |
| `AGENT_CLI_PREVIEW_MAX` | (phase 5, P5-GUIDE-6/7) 2 000 chars printed by one `aurora-agent` call unless `--raw` | — | 24 |
| `AGENT_CLI_PAGES_DEFAULT` / `AGENT_CLI_PAGES_MAX` | (phase 5, P5-GUIDE-6) 20 / 200 pages for `aurora-agent call --all` | — | 24 |
| `AGENT_CLI_SAVE_MAX` | (phase 5, P5-GUIDE-6) 64 MiB written by one `aurora-agent call`, all pages together | — | 24 |
| `PLATFORM_TOOLS_CORE_MAX` | (phase 5, P5-GUIDE-7, D-297) 8 platform tools listed by the MCP bridge in `on_demand` mode | — | 24, 12 |
| `MODEL_ID_MAX` | (phase 5, P5-MODELS-1, D-303) 200 chars per model id (default, role, `model_info` key); characters `[A-Za-z0-9._:/@-]` (`08 §Model roles`) | — | 08 |
| `MODEL_INFO_MAX` | (phase 5, P5-MODELS-1) 64 `model_info` entries per LLM endpoint | — | 08, 15 |
| `USAGE_BY_MODEL_MAX` | (phase 5, P5-MODELS-2, D-304) 16 `by_model` entries per turn; entries beyond the limit are folded into the default model's entry and `unverified_models` is set | — | 10 |
| `USAGE_RANGE_MAX_DAYS` | (phase 5, P5-USAGE-1, C-1000) 366 days per `GET /v1/workspaces/{ws}/usage`; a longer range is 422 `usage_range_invalid` | — | 05 |
| `USAGE_TOP_MAX` | (phase 5, P5-USAGE-1, C-1003) 50 workspaces listed by `GET /v1/admin/usage`; the rest are summed in `other_workspaces` | — | 05 |
| `AVATAR_MAX` | 512 KiB (PNG/JPEG/WebP) | — | 10 |
| `SSE_KEEPALIVE` | 15 s | — | 05 |
| `API_RATE_*` | (phase 2, RFE-017) buckets of `05 §Conventions`: general 600/min (burst 120), write 120/min (30), message 30/min (10), upload 20/min (5), simulate 20/min (5), search 30/min (10), export 6/h (2), org 3 000/min (600); (phase 5, FIX-146, D-232) telemetry 60/min (20), the per-user limit of `POST /v1/telemetry/traces` | `ratelimit.api.*` | 05 |
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
| `PARK_FOLLOWUP_LINES_MAX` | (phase 5, P4-011B spec batch, D-264(e)) 10 approvals listed in one resume follow-up, then "…and N more" (C-824) | — | 07, 10 |
| `PUSH_RETRY_MAX` / `PUSH_DISABLE_AFTER_FAILURES` | (phase 5, P4-011B spec batch) 3 tries within 60 s for a 429/5xx answer / 5 consecutive failures disable a subscription (C-834) | — | 07, 16 |
| `PUSH_ENDPOINT_MAX` / `PUSH_VAPID_JWT_TTL` | (phase 5, P4-011B spec batch) 2 048-character subscription endpoint URL (422 `push_endpoint_not_allowed`) / 1 h VAPID JWT `exp`, reused per (key, audience) until 5 min before (C-831, C-833) | — | 05, 16 |
| `STORAGE_SNAPSHOT_MAX_AGE` | (FIX-126) 1 h: the Storage pages (`GET /admin/storage`, `GET /orgs/{org}/storage`) are served from a shared snapshot at most this old; a stale or missing one is recomputed once under a lock, the daily attachment sweep and the Refresh buttons recompute it too | — | 05, 10, 14 |
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
| `TERMINAL_WAKE_WINDOW` / `TERMINAL_WAKES_PER_AGENT_HOUR` | (phase 4, P4-TERM) the revocation bound (freeze/disable closes sessions) is the API's 10 s access re-check `ACCESS_RECHECK` in `terminal/service.rs`: sessions close within 10 s; there is no separate auth-cache constant (a 15 s draft value was never read and was removed, FIX-119) / 60 s wake demand window / 12 wakes per agent per hour | — | 11 |
| `SSH_KEYS_PER_USER` | (phase 5, P4-SSH-1, D-239) 10 registered SSH public keys per person (409 `ssh_key_limit`) | — | 04, 05 |
| `SSH_CERT_TTL` | (phase 5, P4-SSH-1) 900 s validity of a minted user certificate (`valid_after` is backdated 60 s for clock skew); 60 … 86 400 s | `ssh.cert_ttl_secs` | 04, 05 |
| `SSH_MINTS_PER_USER_AGENT_HOUR` | (phase 5, P4-SSH-1, D-239(c)) 30 certificate mints per person per agent per clock hour, counted exactly across API replicas in `ssh_mint_counters` (429 `ssh_mint_rate_limited` with `Retry-After` to the hour's end) | — | 05 |
| `SSH_SESSION_LEASE` / `SSH_HEARTBEAT` / `SSH_HEARTBEAT_FAILURES` | (phase 5, P4-SSH-2) 90 s session lease / 30 s terminator heartbeat (renews the lease, re-runs the access law) / 3 consecutive failed heartbeats make the terminator close every session it holds (fail closed) | — | 11, 16 |
| `SSH_SESSIONS_PER_USER` / `SSH_SESSIONS_PER_AGENT` | (phase 5, P4-SSH-2) 4 / 8 open SSH sessions, counted over open lease-current `ssh_sessions` rows (global, not per replica); over a cap the session open is refused and the client sees an ordinary authentication failure | `ssh.sessions_per_user`, `ssh.sessions_per_agent` (lower only) | 11 |
| `SSH_CHANNELS_PER_SESSION` / `SSH_CHANNELS_PER_SANDBOX` | (phase 5, P4-SSH-2) 4 concurrent session channels per SSH connection (terminator) / 8 concurrent SSH channels per sandbox (runner and supervisor; separate from `TERMINALS_PER_SANDBOX`) | — | 12, 16 |
| `SSH_GRANT_TTL` | (phase 5, P4-SSH-2) 30 s single-use `auz_` channel grant | — | 16 |
| `EXECS_PER_SANDBOX` | (phase 5, P5-FILES-1, D-316) 12 concurrent `exec.*` children per sandbox at the supervisor (`SSH_CHANNELS_PER_SANDBOX` + `FILE_SESSIONS_PER_SANDBOX`; the per-kind caps are the runner's) | — | 12 |
| `FILES_GRANT_TTL` / `FILE_SESSIONS_PER_SANDBOX` | (phase 5, P5-FILES-1) 30 s single-use `auf_` files grant / 4 live files legs per sandbox (runner, 4409) | — | 16 |
| `FILES_SESSION_IDLE` / `FILES_SESSION_LIFETIME` | (phase 5, P5-FILES-1) 60 s without a request / 1 h, then the API's pooled SFTP session for a (person, agent) closes; the next request opens another | — | 16 |
| `FILES_LIST_MAX` / `FILES_DELETE_MAX` | (phase 5, P5-FILES-1, D-316(e)) 5,000 entries per listing (`truncated` beyond) / 10,000 entries per recursive delete (422 `too_many_entries`, nothing deleted) | — | 05 |
| `FILES_UPLOAD_MAX` / `FILES_EDIT_MAX` | (phase 5, P5-FILES-1, D-316(e)) 512 MiB per uploaded file (413 `file_too_large`) / 1 MiB of UTF-8 text opened in the editor (422 `file_not_editable`) | — | 05 |
| `PENDING_ACCESS_TTL` / `PENDING_ACCESS_MAX` | (phase 5, P5-UX-5, D-325) 30 days before an unused pending workspace access row expires / 50 pending rows per workspace (422 `pending_access_limit`) | — | 04, 05 |
| `ACCESS_REQUEST_TTL` | (phase 5, P5-UX-17, D-348) 14 days before an agent's open request for access expires | — | 03, 14 |
| `ONBOARDING_NOTICES_MAX` | (phase 5, P5-IA-1, C-987) 50 one-time notice keys remembered per person | — | 03 |
| `FILES_PATH_MAX` | (phase 5, P5-FILES-1) 4,096 bytes per path, 255 per component (400 `invalid_path`) | — | 05 |
| `FILE_TRANSFERS_PER_USER` / `FILES_TRANSFER_IDLE` | (phase 5, P5-FILES-1) 4 concurrent content transfers per person per API replica (429 `too_many_transfers`) / 60 s without progress aborts a transfer | — | 05 |
| `FILES_AWAKE_GRACE` / `FILES_AWAKE_CAP` | (phase 5, P5-FILES-1) 120 s after the last Files request / 4 h per boot of Files-held wakefulness (keep-awake reason `files`) | — | 11 |
| `SSH_IDLE_TIMEOUT` / `SSH_MAX_LIFETIME` | (phase 5, P4-SSH-2) 600 s without channel bytes or client keepalives / 4 h per SSH connection | `ssh.idle_timeout_secs` (lower only) | 11 |
| `SSH_AWAKE_GRACE` / `SSH_AWAKE_CAP` | (phase 5, P4-SSH-2) 120 s after the last channel byte / 4 h per boot of SSH-held wakefulness | `ssh.awake_cap_secs` (≤ `BACKGROUND_CAP`) | 11 |
| `SSH_WAKE_WAIT` | (phase 5, P4-SSH-2) 90 s a channel waits for a waking sandbox (the terminator's long-polls of ≤ 45 s each), then the channel fails with a notice | `ssh.wake_wait_secs` | 11 |
| `SSH_CHANNEL_POLL_MAX` | (phase 5, P4-SSH-2) 45 s: the longest `wait_secs` of one `POST /v1/terminator/sessions/{id}/channels` long-poll for a waking sandbox | — | 16 |
| `SSH_PREAUTH_MAX` / `SSH_PREAUTH_PER_IP` | (phase 5, P4-SSH-2) 256 / 8 concurrent unauthenticated connections per terminator replica (globally / per client address); beyond them the socket is closed at once | `terminator.max_preauth`, `terminator.max_preauth_per_ip` (lower only) | 15 |
| `SSH_CONNECT_RATE` / `SSH_CONNECT_BURST` | (phase 5, P4-SSH-2) 30 new connections per minute per client address, burst 10, per terminator replica (`terminator.connect_rate_per_min` / `connect_burst` may raise them) | — | 15 |
| `SSH_AUTH_TIMEOUT` / `SSH_AUTH_ATTEMPTS` / `SSH_AUTH_FAILURE_DELAY` | (phase 5, P4-SSH-2) 20 s from accept to successful authentication / 6 credential attempts per connection (the `none` probe does not count) / 1 s before every authentication failure, whatever its cause (hint-free) | — | 15, 16 |
| `SSH_EXEC_COMMAND_MAX` / `SSH_ENV_VARS_MAX` | (phase 5, P4-SSH-2) 8 KiB `exec` command (no NUL) / 16 accepted `env` variables of ≤ 256 bytes each | — | 12, 16 |
| `SSH_API_RATE` | (phase 5, P4-SSH-2) 50 terminator-plane requests/s per terminator replica, burst 100 | — | 16 |
| `SSH_SESSION_RETENTION` | (phase 5, P4-SSH-2) 30 days for closed `ssh_sessions` rows | — | 03 |
| `EVAL_QUESTIONS_PER_AGENT` | (phase 5, P4-EVAL, D-293(c); Q-EVAL-3 accepted, D-300) 100 eval questions per agent (422 `eval_question_limit`) | — | 10, 05 |
| `EVAL_QUESTION_CHARS` / `EVAL_EXPECTED_TEXT_CHARS` / `EVAL_CRITERIA_CHARS` | (phase 5, P4-EVAL) 2 000 / 1 000 / 1 000 characters of a question / of an expected text answer / of judged criteria | — | 10, 03 |
| `EVAL_ANSWER_CHARS` | (phase 5, P4-EVAL) 4 000 characters of an answer kept on an eval result (sanitised; the judge reads its own cut, `EVAL_JUDGE_INPUT_MAX`) | — | 10, 03 |
| `EVAL_TURN_TIMEOUT` | (phase 5, P4-EVAL, D-290) 900 s per eval question; the turn is aborted and the question is `error` (`timeout`); never above `TURN_CEILING` | — | 10 |
| `EVAL_RUNS_ACTIVE_PER_AGENT` / `EVAL_RUNS_PER_AGENT_DAY` | (phase 5, P4-EVAL, D-290(d), D-293(c); Q-EVAL-3 accepted, D-300) 1 active run per agent (409 `eval_run_active`) / 20 run starts per agent per UTC day, manual and scheduled together (429 `eval_runs_limited`) | — | 10, 05 |
| `EVAL_SCHEDULE_MIN_INTERVAL` / `EVAL_SCHEDULES_PER_AGENT` | (phase 5, P4-EVAL, D-293(b); Q-EVAL-3 accepted, D-300) 1 h between eval schedule occurrences / 5 eval schedules per agent | — | 10, 05 |
| `EVAL_JUDGE_TIMEOUT` / `EVAL_JUDGE_MAX_TOKENS` / `EVAL_JUDGE_INPUT_MAX` / `EVAL_RATIONALE_CHARS` | (phase 5, P4-EVAL, D-292; Q-EVAL-3, Q-EVAL-4 accepted, D-300) 60 s per judge call / 512 output tokens / 8 KiB of answer text sent to the judge / 300 characters of judge rationale kept | — | 10, 08 |
| `EVAL_RUN_RETENTION` | (phase 5, P4-EVAL, D-294; Q-EVAL-6 accepted, D-300) 90 days after a run finishes, then the run, its results and its eval conversations are deleted | — | 10, 05 |
| `EVAL_REGRESSION_DROP` | (phase 5, P4-EVAL, D-293(d); Q-EVAL-7 accepted, D-300) 10 percentage points of pass rate below the previous finished run of the same question-set version before a notice shows | — | 10 |
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
| `ADDON_MANIFEST_MAX` / `ADDON_ADDONS_MAX` / `ADDON_VERSIONS_MAX` | (phase 5, P5-ASRC, D-250) 1 MiB per manifest / 100 addons per manifest / 50 versions per addon | — | 09, 22 |
| `ADDON_LEASE` / `ADDON_CHECK_CONCURRENCY` / `ADDON_FETCH_TIMEOUT` / `ADDON_FAILURE_BACKOFF` | (phase 5, P5-ASRC) 5 min lease per source / 2 checks per replica at once / 20 s per fetch / 1 h backoff after a failed check | `plugins.addon_sources.update_interval` (the schedule itself) | 05, 22 |
| `API_STREAMS_PER_PRINCIPAL` | (phase 2, RFE-017) 20 concurrent SSE connections + long-polls | — | 05 |
| `DELTA_COALESCE` | (phase 1, RFE-016) 100 ms or 2 000 chars per turn per delta | `events.delta_coalesce_ms` (100–500) | 05 |
| `LONG_POLL_MAX` | 30 s | — | 05 |
| `TELEMETRY_INGEST_BODY_MAX` / `TELEMETRY_INGEST_SPANS_MAX` | (phase 5, FIX-146, D-232, C-732 (6)) 256 KiB request body (413 above it) / 512 spans per `POST /v1/telemetry/traces` request | — | 05 |
| `TELEMETRY_SPAN_ATTRIBUTES_MAX` / `TELEMETRY_SPAN_EVENTS_MAX` | (phase 5, FIX-146, C-732 (6)) 32 attributes / 8 events per browser span | — | 05 |
| `TELEMETRY_STACKTRACE_MAX` | (phase 5, FIX-146, C-732 (6)) 4 KiB of `exception.stacktrace` per browser span | — | 05, 14 |
| `TELEMETRY_CLOCK_PAST` / `TELEMETRY_CLOCK_FUTURE` / `TELEMETRY_SPAN_DURATION_MAX` | (phase 5, FIX-146, C-732 (6)) browser span timestamps clamped to [now − 15 min, now + 1 min]; duration clamped to ≤ 15 min | — | 05 |
| `DB_SPAN_STATEMENT_MAX` | (phase 5, FIX-146, D-232(g)) 4 KiB of parameterized SQL in a database span's `db.query.text` | — | 15 |
| `SETTINGS_SECTION_BYTES_MAX` | (phase 5, SET-1, D-236) 128 KiB of stored JSON per instance-settings section (413 `settings_too_large`) | — | 03, 05 |
| `SETTINGS_IMPORT_BYTES_MAX` | (phase 5, SET-1, C-776) 512 KiB settings document per import request (413) | — | 05 |
| `SETTINGS_RELOAD_POLL` | (phase 5, SET-1, D-236(f)) 30 s: API replicas and gateways compare the section generations with their snapshot as a backstop to `aurora_settings` | — | 06, 15 |
| `SETTINGS_AUDIT_VALUE_MAX` | (phase 5, SET-1, C-777) 1 KiB per before/after value in an `instance_settings.*` audit row (longer values truncated with `…`; lists over it recorded as added/removed entries) | — | 04 |
| `SETTINGS_PREVIEW_ORGS_MAX` | (phase 5, SET-1, C-774) 200 affected organizations listed by a settings preview (`affected_orgs_total` counts the rest) | — | 05 |
| `SECRET_STORE_HEALTH_INTERVAL` | (phase 5, FIX-124, C-746) 30 s: a store's health and the value counts of Admin › Secret stores are probed at most once per interval, however many page loads | — | 05, 08 |
| `SECRET_STORE_PROBE_TIMEOUT` | (phase 5, FIX-124, C-746) 8 s per store probe (health, Test connection) | — | 05, 08 |
| `SECRET_MIGRATION_DISCARD_GRACE` | (phase 5, FIX-124, C-747) 2 s between a dashboard migration's last commit and the discard of the old values (gateway caches reload) | — | 05, 08 |
