# REVIEW-5 wave 2 — DEP (deployment/release) and SPEC (contracts/inventory) — saved by coordinator

COORDINATOR CHECKS: RUN-5 REJECTED — a namespaced RoleBinding's ServiceAccount subject with no namespace defaults to the binding's namespace in the RBAC authorizer (and kind e2e passes); hygiene only. GWD-1 confirmed in code (HostPattern::matches checks port only when the pattern has one; absolute.rs sets Scheme::Http; no scheme check before injection). POL-1 confirmed (freeze UPDATE … WHERE frozen_at IS NULL).

## DEP-1 (medium) — runner/connector-host/relay ops listeners (health+metrics) unreachable from every source; no scrape path (no monitoring ingress peer, no ServiceMonitor)
- `deploy/helm/aurora/templates/workloads.yaml:52` (runner ports 4105, 4108; 4106 absent), `:58` (relay 4193 skipService), `:63` (connector-host 4192); `networkpolicy.yaml` ~170-190, ~237-243, ~283-290; docs/15:86, :89, :98, §Observability ~510. (FIX-074 F-5 touched relay 4193 only — check.)
## DEP-2 (medium) — readiness/liveness probes are tcpSocket for every workload; docs/15 "Kubernetes probes use plain /readyz" (`_helpers.tpl:153-157`; `docs/operator/outage-and-alerts.md:27-35`). Gateway /readyz is on 4103 (policy: api only) — kubelet reachability depends on CNI.
## DEP-3 (medium, PLAUSIBLE: depends on GitHub environment settings) — `release` environment (signing key) reachable from PR-edited workflow unless the environment restricts branches/reviewers (`.github/workflows/ci.yml:421-423`, `:447`, `:479-498`; `docs/ops/builtin-plugin-signing.md`)
## DEP-4 (medium) — docs/18:915 claims multi-arch images on main; CI builds amd64 only (`ci.yml:405-416`, `:461-473`, `:514-524`, `:538-590`)
## DEP-5 (medium, docs) — key-rotation step 2 cannot be run as written (curl to 4103/4104/4111 blocked by NetworkPolicy; distroless images have no curl) (`docs/operator/key-rotation.md:53-56`)
## DEP-6 (low) — `scripts/e2e/llm.sh:39`, `:96` reuses a stale base sandbox image
## DEP-7 (low) — sandbox-image actions/cache key omits `**/Cargo.toml`, `rust-toolchain*`, `.cargo/**`, `scripts/images/**` (`ci.yml:216`)
## DEP-8 (low) — base images / tool installs not pinned by digest (`images/services/Containerfile:21`, `:148`; `values.yaml` postgres:16; `images/services/web.Containerfile:3`, `:13`; sandbox npm tree unlocked, `pip3 install uv`)
## DEP-9 (low; = OPS-12) — hooks SA can get/create/patch every Secret (`hooks.yaml:33-35`, `:140`, `:225`)
## DEP-10 (low) — guards: `no-live-tokens.sh` scans only plugins/builtin; `no-dev-idp-in-release.sh` step 2 checks only services Containerfile; `dep-graph.sh` treats "did not match any packages" as OK
## DEP-11 (low, docs) — docs/18:918 claims a nightly performance run; nightly.yml has none
## DEP-12 (low; ~OPS-7) — secrets hook egress falls back to 0.0.0.0/0:443 when lookup is empty (`hooks.yaml:78-91`)
## DEP-13 (low, docs) — backups.md restore omits Secret token keys (runner/connector/relay tokens, POSTGRES_PASSWORD, DATABASE_URL — `hooks.yaml:163-206`) and has no Vault row (`docs/operator/backups.md:10-19`, `:83-101`)

## DEP not covered: helm cases; e2e script bodies; perf; ci rust job bodies; most operator pages; real-cluster behaviour.

---

## SPEC-1 (medium; = CHAN-2 wider) — documented daily retention pruning doesn't exist: channel_inbox/ops/cards, sessions, expired context credentials never deleted (`docs/05:363`; `docs/02:269`; `docs/13:270`; `limits.rs:190`; only `repo/channel_cards.rs:229-235` action_token_uses)
## SPEC-2 (medium) — `GET /v1/workspaces/{ws}/usage`, runtime daily rollup, token-usage aggregation documented as current, not built, no phase tag (`docs/05:516` + jobs table; `docs/10:765-767`; `runtime_daily` table never written — migration `20261105000009_runners_sandboxes.sql:79`); D-138 moved P4-LLM to phase 5; no FEATURES row
## SPEC-3 (medium) — Web Push specified as phase 4 in docs/05 §Web Push, docs/14:457, :918-921, docs/16, docs/17 SEC-74..76, but moved to phase 5 (D-138); FEATURES.md:147 says "spec not written yet"
## SPEC-4 (medium) — `GET /v1/schedules/{id}/runs` and `GET /v1/webhooks/{id}/deliveries` page by limit only (`next_cursor: None`) (`schedules/routes.rs:237-256`; `webhooks/routes.rs:242-261`; docs/05:111)
## SPEC-5 (medium, PLAUSIBLE) — attachment quota "dashboard notice to owners" at 80/100 % is an audit row + badge on the storage page only (`docs/10:456-459`; `attachments/sweep.rs:183-206`, `:29-76`; `web/src/components/org/storage-view.tsx:150-153`)
## SPEC-6 (low) — openapi declares 401 on 13 of 315 ops, 429 on 6 (`docs/API_CONTRACTS.md:34-36`)
## SPEC-7 (low) — 119 string properties document their enumeration in prose, not as OpenAPI `enum`
## SPEC-8 (low, docs) — route catalogue drift: docs/05:766,767,769 list DELETE on collections (code: DELETE …/{id}); placement/choices (C-528) missing from docs/05; `PUT /connector/files/{id}` without /v1 (API_CONTRACTS:685); 61 served routes outside openapi.json by design (`tests/it/foundation.rs:506`) and not all listed in docs/05
## SPEC-9 (low, unwired) — `WATCH_PATTERN_MAX` (limits.rs:153; validator uses literal 256 `aurora-registry/src/validate/mod.rs:785`), `CHANNEL_LEDGER_RETENTION`, `REQUEST_LOG_RETENTION` (limits.rs:43; job uses config key) have no reader

## SPEC not covered: handler-by-handler status trace for a 40-route sample (403 vs 404 on sensitive reads); most FEATURES rows of phases 0-1; D-001..D-118 in full; MASTER_SPEC relation; docs/03 vs 05 vs 14 for all tables.
