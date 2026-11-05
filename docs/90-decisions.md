# 90 — Decision Log

Every deliberate departure from OneCLI (`../onecli-spec/docs`) and every
refinement of the proposals (`../onecli-spec/proposals`). New decisions are
appended; superseded ones are marked, never deleted.

| Id | Decision | Instead of | Rationale |
|----|----------|------------|-----------|
| D-01 | All services, CLI, supervisor, connectors and plugin SDK in **Rust**; dashboard in Next.js. | TS control plane/runner/supervisor/adapter + Rust gateway | One language lets crates be shared (policy, protocol, crypto), removing cross-runtime parity suites. User requirement. |
| D-02 | **One policy implementation** (`aurora-policy`) linked by API and gateway. | Proposal 01's API→gateway dry-run endpoint | With one language, linking the crate is simpler and faster than a network dry-run; the gateway still offers `check-snapshot` for plugin tests. Same goal (no second matcher). |
| D-03 | **Postgres `LISTEN/NOTIFY`** is the event bus; claims with `SKIP LOCKED` are the queue. | In-process emitter (API limited to one replica) | Horizontal API scaling without adding Redis/a broker. Notifications are hints; state is re-read. |
| D-04 | **Approvals stored in Postgres**; gateway waits on NOTIFY with polling fallback. | Memory or Redis approval store | Multi-replica gateways without Redis; holds are a slow path so DB writes are acceptable; the request hot path still never touches the DB. |
| D-05 | Rate-limit counters stay **in memory per gateway replica** in phase 0 (documented N× approximation). | Redis counters | Keeps the hot path DB-free with no new dependency; a `CounterStore` seam is reserved. |
| D-06 | Gateway **control API on a separate internal port** (4103), unreachable from sandboxes; no browser → gateway path. | Same port as the proxy, exposed as `/gw/*` | Reduces attack surface. With approvals in Postgres and no vault pairing, browsers never need the gateway. |
| D-07 | Gateway cache invalidation via NOTIFY. | API → gateway HTTP call | Reaches every replica; no API→gateway coupling. |
| D-08 | **Credential selection before the policy decision** (ids only, nothing decrypted). | Decision, then selection, then re-evaluation for account-specific rules | Rules can name connections directly; one evaluation; "policy before decryption" still holds. |
| D-09 | Hand-authored rules (published) are evaluated **before** grant-derived rules within a scope; derived rules are always live (not drafted). | Interleaved derived rules re-materialized into drafts | Admin overrides beat per-agent defaults; grant changes take effect immediately without a publish step. |
| D-10 | Rate-limited is **stricter than needs-approval** when combining scopes. | OneCLI order block < approval < rate-limited < allow | An exhausted quota is a refusal now; asking a human to approve a request that will be rate-limited anyway is pointless. |
| D-11 | Gateway refuses **ambiguous paths** (dot segments, encoded slashes). | Not specified | Prevents matcher/upstream disagreement (path confusion). |
| D-12 | **Pluggable secret stores** with `SecretRef` columns everywhere; the DB row is the compare-and-set authority; stores only append versions. Phase 0: `inline`; phase 3: HashiCorp Vault. | Inline ciphertext / KMS envelope + Bitwarden/1Password vault fallbacks | User requirement (Vault). Version-append stores need no CAS of their own; one refresh discipline for all stores. Bitwarden/1Password pairing flows dropped (could return as stores). |
| D-13 | Inline envelope with **key ids and AAD binding** (purpose, org, owner). | 3-part `iv:tag:ciphertext` without AAD | Key rotation without downtime; ciphertext cannot be swapped between rows. |
| D-14 | **All tokens hashed at rest**; hosted agents have no long-lived token: **boot tokens** per sandbox boot. | Stored agent tokens reused across boots | A leaked sandbox token dies with its boot; nothing re-showable is stored. |
| D-15 | Per-turn **context credentials** (Proposal 02 §10) adopted in phase 0, delivered via per-session **loopback proxies** in the supervisor. | — | Exact approval attribution; proxy env carries no credentials. |
| D-16 | **LLM provider registry** with operator-configured endpoints; phase 0 providers **LiteLLM** (first) and **Anthropic**; API dialects decide harness compatibility. | `anthropic`/`openai` hard-wired into secret types | User requirement; LiteLLM is self-hosted with a configurable host, so endpoints are configuration and LLM hosts are reserved dynamically. |
| D-17 | Phase 0 real harness: **Claude Code** (via LiteLLM's Anthropic-compatible endpoint or Anthropic directly), plus `fake`. | jcode | Mature headless agent runtime that speaks the Anthropic Messages dialect served by both phase 0 providers. Additional adapters later. |
| D-18 | Default hosted agent **`Daneel`** in every new workspace; shared bots default to the same name. | No default agent | User requirement; gives a working first-run path. |
| D-19 | **No editions.** Roles, OIDC SSO and app availability are plain features; SCIM, groups, budgets, billing, desktop takeover deferred or dropped. | Cloud/Community/Enterprise split | Single self-hosted product. |
| D-20 | **Instance admin** flag for deployment-level operations (plugins, keys, connectors, runners, host claims, instance OAuth clients). | Operator via env/installer only | Proposals require operator approvals in the dashboard; org roles must not grant deployment powers. |
| D-21 | Sign-up default **`invite_only`**, with `open` and `sso_jit` modes. | Always-open sign-up creating fenced orgs | Self-hosted team deployments usually restrict registration; JIT serves SSO. |
| D-22 | **Instance-level OAuth clients** stored in the database (in addition to org/workspace BYOC). | Env-var default clients | Both API and gateway need client credentials for refresh; DB is the single source. |
| D-23 | Hook host **pulls** snapshots and modules from the API's internal port; it has no DB credentials. | (Proposal 01 unspecified) | Strengthens the second isolation layer. |
| D-24 | Hook wall time 15 s for connect/mint hooks, 1 s for summarize. | "a few seconds" | Network hooks need realistic budgets; summarize must not delay holds. |
| D-25 | Connector **integration-level ownership** (one replica serves all sockets of an integration). | Presence-level ownership leases | Simpler, matches how platform connections are grouped; balancing by claim count. |
| D-26 | Connector manifests are **signed**; the core verifies the manifest signature; image signatures are left to deployment tooling. | "Connector images are signed" | The core cannot observe the image; it can verify what it receives. |
| D-27 | **Mirror rule**: answers go back through the door the question came in; doorless turns go to the conversation's most recent thread; web-originated turns are not mirrored. | Adapter mirror pass (unspecified for web turns) | Avoids noisy duplication between dashboard and chat while keeping "one conversation, several doors". |
| D-28 | Kubernetes sandboxes are **bare Pods** (`restartPolicy: Never`) + PVC + per-boot Secret. | Job + PVC | The platform, not the kubelet, decides restarts; bootstrap tokens are single use. |
| D-29 | Sandboxes have **no DNS** and a **read-only root filesystem**; boot material arrives as a secret env/Secret consumed by a non-dumpable supervisor. | Internal network with DNS; files mounted | Closes DNS exfiltration; keeps tokens out of the agent-visible environment. |
| D-30 | **podman** backend (Docker-compatible API) for development and single host. | Docker | User requirement; rootless-friendly. |
| D-31 | Single external origin with an edge proxy in **every** deployment (Caddy in compose). | Ports mode vs proxy mode | One routing model, first-party cookies, no CORS. |
| D-32 | Dashboard is a **pure API client** (generated OpenAPI client). | Server actions calling services directly | Removes the "inject providers in every entry point" bug class. |
| D-33 | Platform tools come from an API **tool registry** whose catalogue is synced to the sandbox. | Central switch + hard-coded supervisor list | Adding a tool is an API-only change. |
| D-34 | Webhook verification secrets stored via SecretRef; webhook payloads labelled untrusted in turn text. | Not specified | Consistency with secret storage and prompt-injection hygiene. |
| D-35 | One error envelope `{error:{code,message,details}}` everywhere, including gateway guidance. | Two API shapes + flat gateway shape | Simpler clients; agents read either equally well. |
| D-36 | Approval timeout **300 s** everywhere. | 180 s (gateway) vs 300 s (SDK/docs) | Removes OneCLI's inconsistency. |
| D-37 | Schedule tools renamed `schedule_*`; peer-task tools `*_peer_task`. | `schedule_task`/`list_tasks` overlapping with peer tasks | Avoids ambiguous tool names. |
| D-38 | Messaging `send_as_me`/read-as-user dropped from phase 0. | Slack-read [EE] | Requires `user_tokens`; no phase 0 connector provides it. |
| D-39 | **LLM governance by key origin**: endpoints carry key-origin rules (operator endpoint, LiteLLM team, provider account); keys are verified at save, before first use of each value, and against every response. Users may hold their own keys if they belong to the approved account. | Restricting who may store keys | The requirement is contractual (corporate accounts carry the privacy terms), not about who holds the key. Anthropic reports the key's organization and workspace in response headers, so verification needs no admin credential and sends no prompt. |
| D-40 | **Layered governance that only tightens**: instance ceiling (config) → org governance (live document) → workspace policy → grants; LLM governance is a dedicated gateway step, not rules. | Compiling governance into derived rules | Rules can be reordered and loosened by authors; governance must not be. A dedicated step also gives precise error codes. |
| D-41 | **Optional uncredentialed default-deny** (`credentialed_only` egress posture) with org and workspace allow-lists, applying to the open lane too. | OneCLI: default-deny only for credentialed traffic; open lane never default-denies | The only complete control against exfiltration and against models reached through unrecognized hosts; optional because it requires curating allow-lists. |
| D-42 | Governed LLM hosts (endpoint hosts + known LLM hosts) can never be targeted by generic secrets or app connections. | Plugins barred only from endpoint hosts | Closes the "personal key as a custom secret" bypass. |

## Implementation decisions (orchestrator, 2026-11-05)

Decisions made while implementing the spec. They do not change product
behaviour; they record environment and tooling choices.

| Id | Decision | Rationale |
|----|----------|-----------|
| I-01 | Requirement traceability ids are `M<n>/<doc>§<section>` (milestone + owning section, e.g. `M0/03§1`) plus the `SEC-nn` invariants of `17`. | The spec has no per-requirement ids; milestones in `19` define acceptance. |
| I-02 | Local toolchain is the system Rust (Arch, 1.99 stable, no rustup). `rust-toolchain.toml` pins `stable` + `wasm32-wasip2` for CI/rustup users, but the **wasm32-wasip2 target is not installed locally**: `aurora-hook-wit`, `aurora-plugin-sdk` and wasm fixture hooks join the workspace at M3 and need the target (rustup or the distro wasm target package). | Environment fact; blocks only M3 hook builds. |
| I-03 | `profile.dev` uses `debug = "line-tables-only"` and no debuginfo for dependencies; one shared `target/`. | Dev host has ~39 GB free disk; wasmtime/aws-lc-rs/kube debug builds are large. |
| I-04 | Version set pinned to the known-compatible line: axum 0.8, utoipa 5 + utoipa-axum 0.2, sqlx 0.8 (+ sqlx-cli 0.8.6), hyper 1, rustls 0.23 (aws-lc-rs), schemars 1. | Mutually compatible; newer majors (sqlx 0.9, utoipa 6) can be adopted later in one task. |
| I-05 | sqlx compile-time checked macros run **online** against the dev database (`DATABASE_URL` in the gitignored `.env`, dev Postgres container `aurora-dev-pg` on `127.0.0.1:55432`); `cargo sqlx prepare --workspace` keeps `.sqlx/` committed so CI builds offline (`SQLX_OFFLINE=true`). | Spec pins compile-time checked queries; online locally, offline in CI. |
| I-06 | Database tests use `#[sqlx::test(migrator = "aurora_db::MIGRATOR")]` (fresh database per test on the server named by `DATABASE_URL`); `aurora-testkit` wraps it and can start a podman Postgres when no server is configured. | Meets "one database per test" without bespoke harness code. |
| I-07 | HTTP error envelope rendering for axum lives behind the `axum` cargo feature of `aurora-core` (the type itself is dependency-light). | Keeps `aurora-core` usable from non-HTTP crates without adding a crate outside the `02` layout. |
| I-08 | `docs/API_CONTRACTS.md` indexes `05` (owning document for routes) and records the response/request **schemas** that `05` leaves implicit, defined per milestone before parallel backend/frontend work. The generated OpenAPI document (`/v1/openapi.json`, checked in as `web/openapi.json`) is the machine contract the dashboard client is generated from. | One source per fact; frontend never hand-writes API types. |
