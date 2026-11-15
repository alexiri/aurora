# REVIEW-5 wave 2 — GWD (gateway decision pipeline) and GWT (gateway transport) — saved by coordinator

## GWD-1 (high, CONFIRMED) — credentials injected over plain HTTP and on any port of a claimed host
- `crates/aurora-policy/src/host.rs` (`HostPattern::matches`: port checked only when the pattern has one); `services/aurora-gateway/src/proxy/absolute.rs:14-45` (absolute-form `http://` → Scheme::Http, port 80); `decide.rs` (no scheme check in the injection path; Scheme used only in `finalize_request` 1923-1935); `upstream.rs:240-245` (plain TCP when !https); `apps.rs:231-237`; `resolve.rs:662-735`. LLM: `crates/aurora-llm/src/governance/effective.rs:289` (`endpoint_for_host` ignores scheme).
- Scenario: agent sends `GET http://api.github.com/user` via the proxy → token in cleartext to port 80; or `https://host:8443/`.
- Spec gap: SEC-04; docs/06 §Plain HTTP "same pipeline without TLS termination"; docs/09 §7 claim = (host pattern, path prefix).
- Fix: inject only when scheme = https and port = 443 unless the claim names a port; document in docs/06, docs/09 §7.

## GWD-4 (high; code confirmed, impact depends on key privilege) — LiteLLM / configured LLM endpoint: sanctioned key injected on every path (SEC-65 not implemented but litellm endpoints are live since D-138)
- `crates/aurora-llm/src/model.rs:4-11` (MODEL_PATHS six routes; else `ModelRef::NoModel`); `litellm.rs`; `services/aurora-gateway/src/governance.rs:181-290` (`check_6a` never checks the path); no `llm_path_not_permitted`/`allowed_paths` in code.
- Scenario: `POST https://litellm.corp/key/generate` with a master key → agent mints keys; `/v1/images/generations` bypasses `allowed_models`.
- docs/17 SEC-65 (pending, phase 5 P4-LLM); docs/08 §Providers 441-451.
- Fix: per-provider inference-path sets + 403 `llm_path_not_permitted` before the credential is opened, at least for litellm/openai_compatible; or document "never a master key" + defer.

## GWD-2 (medium) — wildcard host claims not bounded to a registrable domain (PSL check only for zones)
- `crates/aurora-registry/src/validate/mod.rs:417-440` (`host_rule`); `validate/package_rules.rs:86-92` (`zone_too_broad`), `:113-122`, `:165-200`. SEC-17 names zones only (spec gap). e.g. `*.herokuapp.com`.

## GWD-3 (medium, PLAUSIBLE) — upstream responses forwarded without checking for injected credential values (path/query-injected secrets echoed in 404 bodies / Location) — SEC-01
- `decide.rs:157-246` (`on_response`); `upstream.rs:298`.

## GWD-5 (low) — policy block responses disclose rule names + policy URL (org slug) to the agent (`decide.rs:416-455`, ~470)
## GWD-6 (low, PLAUSIBLE) — plaintext cache ignores Credential/Agent/User invalidations (60 s) (`plaintext.rs:144-151`); reqwest fallback `unwrap_or_default()` drops timeout + redirect policy (`refresh.rs:71-77`)

## GWD not covered: decide.rs 760-1180 (hold loop site, stale retry, tunnel accounting); mechanisms/** (finalize SigV4, template); summary fallback; ratelimit/resolver invalidation; repo gateway/request_log tenant scoping; guidance texts.

---

## GWT-1 (medium) — own-listener refusal doesn't cover the gateway's routable address when bound to the wildcard
- `services/aurora-gateway/src/guard.rs:134-140` (`is_own`); `main.rs:311-317`; `crates/aurora-config/src/raw.rs:112`; `values.yaml:142,147`. docs/06 §Destination guard step 2. Needs `allowed_private` covering the pod network.

## GWT-2 (medium, CONFIRMED) — unbounded per-tunnel tasks/sockets: no timeout on the inner TLS handshake, no header/idle timeout on the inner HTTP connection (no `.timer()` on the tunnel builder), no per-agent/global cap
- `proxy/connect.rs:116-126`, `:165-166`; `proxy/listener.rs:~45-58`, `:145`. hyper-1.8 `server/conn/http1.rs:472`.

## GWT-3 (medium) — open-lane tunnels never reach the idle limit (`connect.rs:85` `bridge(…, None, …)`; `:70` dial without timeout); docs/06 §Open-lane "end at the idle limit"

## GWT-4 (medium; spec decision) — database lane default `sslmode=require` doesn't verify the server; stored DB password sent to an unverified peer
- `db/pg/server.rs:189-197`, `:363`. docs/06 §Database lane Login; docs/operator/gateway-posture.md:134; SEC-60.

## GWT-6 (medium, PLAUSIBLE) — with `gateway.egress_proxy`, names are never judged locally (`upstream.rs:54-62`; `guard.rs:186-191` `check_name` judges IP literals only; decimal IP spellings pass as names)
## GWT-5 (low) — database login relays the upstream server's error text / internal detail to the agent (`proxy/database.rs:283-296`; `db/pg/wire.rs:283-291`)
## GWT-7 (low, PLAUSIBLE) — NAT64 `64:ff9b::/96`, 6to4 `2002::/16`, IPv4-compatible `::/96`, `fec0::/10`, 198.18/15, 192.0.0/24 not judged (`guard.rs:42-47`, `:83-102`) — same class as CRED-2 (api `connect/env.rs`)

## GWT not covered: hygiene/plaintext/health/status/coalesce; byo.rs admission rules; proxy_protocol v2; pg wire framing edge cases; sqlclass rule actions.
