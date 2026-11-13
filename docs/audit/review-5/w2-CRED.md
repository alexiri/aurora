# REVIEW-5 wave 2 — CRED (secrets, credentials, connect) — saved by coordinator

## CRED-1 (medium, CONFIRMED, concurrency) — OAuth client PATCH is a lost update (public fields and secret) and orphans sealed secrets
- `services/aurora-api/src/domains/oauth_clients/service.rs:225-262`; `crates/aurora-db/src/repo/oauth_clients.rs:94-113` (UPDATE … COALESCE, no CAS on prior `secret_ref`); `oauth_clients/routes.rs:205-212`. Convention: CAS on old ref (`secrets_migrate.rs`, connect `write()`, `reconnect_cas`), docs/08 discard-after-commit.
- Fix: `AND secret_ref = $old_ref` + version check → 409 `conflict`, discard new ref; or SELECT … FOR UPDATE.

## CRED-2 (medium, PLAUSIBLE: needs DNS64/NAT64 network) — probe/instance SSRF guard admits NAT64 (`64:ff9b::/96`, `64:ff9b:1::/48`) and 6to4 (`2002::/16`) prefixes embedding private IPv4; misses 240.0.0.0/4, 198.18.0.0/15, 192.0.0.0/24
- `services/aurora-api/src/domains/connect/env.rs:24-47` (`is_public`), `Guarded::resolve` (~55), `Env::prepare` (~225). docs/09 instance hosts; docs/08 SEC-17, I-20.

## CRED-3 (low, docs) — `docs/operator/key-rotation.md:115` "all six purposes"; `secrets_migrate.rs` SOURCES has eight
## CRED-4 (low) — connect-flow randomness falls back to time-derived UUID if the OS RNG fails (`connect/flow.rs:24-31`; PKCE verifier `service.rs:901-905`) → return an error
## CRED-5 (low, PLAUSIBLE) — popup `postMessage` target origin empty when `external_url` has no origin (`connect/routes.rs` callback `origin_of(...).unwrap_or_default()`; `page.rs connected()`) → silent drop

## Sound (summary): AES-256-GCM aws-lc-rs, AAD binds purpose+org+owner row; Secret<T> redaction; constant-time verify; rekey CAS; Vault client (TLS, no redirects, token never logged, fail closed); OAuth state/cookie sealed + nonce match + user match; redirect URI server-configured; PKCE S256; probe https-only, no redirects, resolver drops non-public at connect time, 10 s, 1 MiB; app instance registration rules + advisory lock; client delete guarded.

## Not covered (first pass)
- connections service/routes authz; secrets routes authz matrix; secrets/gov.rs; LLM key verification (aurora-llm verify_key_origin, litellm team key); apps/availability; signing-key rotation, ed25519.rs; Vault tests; connect/hooks.rs; repo SQL + migrations.

# Second pass (CRED-6..13)
## CRED-6 (medium, CONFIRMED) — LLM key stored `verified` when the endpoint is unreachable, if the endpoint has a static `models` list
- `crates/aurora-llm/src/litellm.rs:77-89` (`list_models` falls back to `ep.models` on any non-InvalidKey error); `services/aurora-api/src/domains/secrets/llm.rs:48-70` (`check_key`), dead arm `:56-62`. docs/08 §LLM keys; docs/05 422 `llm_endpoint_unreachable`; C-502. Fix: `check_key` uses the strict fetch; fallback stays for the picker. Check Anthropic/OpenAI providers for the same pattern.
## CRED-7 (medium; Vault backend only) — deleting an org/workspace removes secrets/app_connections/app_oauth_clients rows without discarding their values in Vault
- `crates/aurora-db/src/repo/orgs.rs:109-133`; `tenancy/service.rs:327-350`; `tenancy/routes.rs:553-563`; contrast `secrets/service.rs delete`, `connections/service.rs delete`.
## CRED-8 (medium, PLAUSIBLE) — org deletion sends no gateway invalidation (`repo/orgs.rs:109-133`; `plaintext.rs:15`, `:146` Scope::Org exists)
## CRED-9 (low) — agent deletion leaves `derived_rules` of source `grant` behind (`agents/service.rs:482-505`)
## CRED-10 (low, latent) — `source_connection_id` never written; `with_children` one level (`repo/connections.rs:123-130`)
## CRED-11 (low, PLAUSIBLE) — hook-based connects never record requested scopes → `missing_scopes` always empty (`connect/service.rs:579-590`, ~:713)
## CRED-12 (low, PLAUSIBLE) — any workspace owner sees provider account ids (`origin`) and creator names of org secrets (`secrets/service.rs:93-103`; `dto.rs`)
## CRED-13 (low, PLAUSIBLE) — `GET /v1/llm/providers` returns every endpoint's `base_url` to any authenticated principal (`secrets/routes.rs:327-395`)
## Also: `llm_http` client follows redirects (default policy, `app.rs:132-135`) — same-host only keeps Authorization.
