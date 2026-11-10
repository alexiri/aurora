# P4-PRV-2: Port previews — `aurora-relay` and byte streams

## Goal
Implement D-146/D-147 and SEC-81, SEC-82, SEC-85 (SPEC-P4c): the relay service that authenticates browsers on the preview domain and streams bytes to the sandbox port through the runner's stream listener, multiplexed over the supervisor channel with credit-based flow control.

## Spec (source of truth; read, don't re-decide)
- D-146, D-147; SEC-81, SEC-82, SEC-85 (+ residual) in `docs/17`.
- `docs/04` §Preview access (origin isolation, `__Host-aurora_preview` cookie, sealed 60 s single-use grant, revocation, host-only session cookie, Fetch Metadata and WebSocket `Origin`, the same-site tightening of the dashboard API while previews are on); `docs/16` W17 relay plane and runner-plane additions, W18 byte streams (frames on W6, 128 KiB credit windows, 64 streams per sandbox, allow-list at both ends); `docs/01` relay row + trust boundary 13; `docs/00` (`aul_`, `auv_`, `aux_`); `docs/03 §Planned changes` (`preview_sessions`, `preview_grant_uses`, `preview_stream_grants`, `relay_tokens`, `relay_replicas`); `docs/05` §Relay plane and preview authorization; `docs/11` runner stream listener :4108 (+ `runner.stream_tls`); `docs/12` Streams; `docs/14` Admin Overview Previews line; `docs/15` (config, wildcard edge rule, relay Helm, NetworkPolicy: runner 4108 from relay pods **only**, relay egress only API/runners/DNS, metrics, upgrade); `docs/02` limits.
- `docs/18` §Port previews (relay and stream tests).

## Scope
New `services/aurora-relay`; runner stream listener and port-stream API; supervisor stream frames + allow-list; API relay plane and grant/session routes; migration + repos; Helm (Deployment, Service :4112, wildcard Ingress/HTTPRoute only when enabled, NetworkPolicy, token Secret), `scripts/guards/edge-routes.sh` and the sandbox-network check for 4108/4112; image + CI publish list; kind e2e with a preview round trip (HTTP and WebSocket) using `kind.sh` env overrides. Contracts **C-550..C-559** only.

## Acceptance
Every relay/stream test in `18 §Port previews`; SEC-81, SEC-82, SEC-85 `proven`; relay/runner/supervisor/api tests, `scripts/e2e/kind.sh --fresh` with the preview round trip (own cluster via env overrides; ask the coordinator first), helm lint/template (previews on/off), guards, fmt, clippy `-D warnings`, OpenAPI drift test.

## Constraints
Depends on P4-PRV-1 merged. The dashboard session never reaches a preview origin; with previews off nothing new is deployed or reachable. Change files only with the Edit/Write tools. No `scripts/perf/*`.
