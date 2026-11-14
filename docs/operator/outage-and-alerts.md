# Database outage behaviour and alert rules (phase 3, RFE-014)

What the gateway does while Postgres or its LISTEN connection is down, what you will see, and which alerts to
set up. This page restates, for operators, [docs/06 §Operation during a Postgres outage](../06-gateway.md#operation-during-a-postgres-outage-phase-3-rfe-014-d-61),
contracts C-380..C-384 in [`docs/API_CONTRACTS.md`](../API_CONTRACTS.md) and
[docs/15 §Observability](../15-deployment-operations.md#observability); where they disagree, those win.
Test evidence: SEC-54 (`docs/tests/security-coverage.md`).

**In one sentence:** an outage of Postgres degrades the gateway to what its caches already know, for at most
`gateway.resolve_cache_ttl_secs`, and refuses everything else (nothing is extended past its TTL and nothing is
refreshed; docs/06).

## The three states

The gateway is in exactly one state, shown by `/readyz`, by `/internal/v1/status` and by the
`aurora_gateway_state{state="ready"|"degraded"|"down"}` gauge (1 for the current state, 0 otherwise).

| State | When | `/readyz` | Behaviour |
|-------|------|-----------|-----------|
| `ready` | database reachable, LISTEN connection established, snapshot loaded | 200 `{"state":"ready"}` | normal |
| `degraded` | snapshot loaded, but the database is unreachable **or** the LISTEN connection is down (more than `GATEWAY_LISTEN_GRACE` = 30 s; a brief reconnect does not flap it) | **200** `{"state":"degraded","reason":"database_unavailable"\|"listener_down","since":…,"cache_entries":n,"cache_oldest_age_secs":n\|null}` | serves from cache within the TTL, refuses new state (below) |
| `down` | no snapshot loaded (for example started while the database was unreachable), or draining after SIGTERM | **503** `{"state":"down","reason":"snapshot_not_loaded"\|"draining"}` | refuses new `CONNECT`s |

- `degraded` answers **200** on purpose, so that orchestrators keep the gateway in rotation: pulling every gateway
  out because the database is down would turn "degraded" into "down" (docs/06).
- `GET /readyz?strict=true` answers **503 for `degraded` as well** (same body); any other value of `strict` is ignored.
  Use it **in monitoring only, never as a Kubernetes readiness probe** (docs/15). The gateway's `/readyz` is on the
  internal listener, port 4103, unauthenticated; it is not routed through the edge. Other services' `/readyz` are unchanged.
- *Unreachable* means a readiness probe (`SELECT 1`, every `GATEWAY_DB_PROBE_INTERVAL` = 5 s, timeout 2 s) failed
  twice in a row, **or** a hot-path query failed with a connection-class error, whichever comes first. A statement-level error
  (constraint, permission, query bug) is **not** an outage and does not change the state.
- Recovery needs one successful probe **and** an established LISTEN subscription.
- Every transition writes one structured log line `gateway.state_change` with `from`, `to`, `reason`.
- This chart's built-in Kubernetes probes for the gateway are TCP checks on its first port (`deploy/helm/aurora/templates/_helpers.tpl`),
  not `/readyz`; they do not see `degraded`. If you add an HTTP readiness probe of your own, use plain `/readyz`.

## What is served from cache, and for how long

The tolerance is **`gateway.resolve_cache_ttl_secs`** (default 60 s). Above 300 the gateway still starts but logs a warning
at start (C-384). While `degraded`, these are served **only while their cache entry is within its TTL**:

- the credential → identity entry and the resolved-host entry for `(credential, host)`: existing agents keep working for the rest
  of their TTL;
- the plaintext cache (60 s), the effective governance and posture, and the registry snapshot (kept in memory indefinitely);
- open tunnels, WebSockets and in-flight requests (they need no database), and in-memory rate limits.

Uncredentialed traffic whose identity and host entries are cached is judged by policy as usual, so **default-deny is never
weakened** by degradation. Existing approval holds keep waiting in memory until their deadline; a hold whose deadline passes is
answered `manual_approval_expired` and its `approvals` row is corrected on recovery. A failed fallback poll is never read as a
denial or an approval.

**Revocation lag.** NOTIFY cannot arrive while the database or the LISTEN connection is down, so a revocation, a freeze, a posture
change or a policy publish takes effect at the latest when the affected cache entries' TTL runs out. Lower
`gateway.resolve_cache_ttl_secs` for less exposure (shorter outage tolerance); raise it for a longer outage tolerance (longer
revocation lag).

**Request log.** Rows are buffered in memory up to `REQUEST_LOG_OUTAGE_BUFFER` (10 000 rows); the oldest are dropped beyond that
(`aurora_gateway_request_log_dropped_total`), and the buffer is flushed on recovery. `last_used_at` writes are skipped. Requests are
never blocked by the log.

## What is refused, and with which code

Refusals are fail-closed. On the proxy port they use the standard gateway error body (C-382):

| Situation | Response |
|-----------|----------|
| credential not in the identity cache | **503** `gateway_degraded`, `details.reason = "database_unavailable"`. **Never 407**: the gateway cannot tell an invalid token from an unreadable store, and a client must not conclude that its token is bad |
| host with no resolved-host entry; context credential minted by the API after the outage began | 503 `gateway_degraded`, `details.reason = "database_unavailable"` |
| plaintext not in the plaintext cache; an OAuth refresh or mint is needed (inline-store values and refresh need Postgres) | 503 `gateway_degraded`, `details.reason = "credential_store_unavailable"` |
| a request that needs a **new approval hold** (it cannot be persisted) | 503 `approval_store_unavailable` |
| new `CONNECT` while `down`, no snapshot | 503 `gateway_degraded`, `details.reason = "snapshot_not_loaded"` |
| new `CONNECT` while draining | 503 `gateway_restarting` |
| the **database lane** (SQL through the gateway, port 4102) when the credential store or the policy/approval store is unavailable | FATAL `57P03` (C-382; unchanged from C-220; docs/06 §Database lane) |
| a hold whose deadline passes during the outage | 403 `manual_approval_expired` |

`gateway_degraded` refusals carry `retry-after: 5` and `x-should-retry: true` (docs/06, C-382). The secret store has its own refusal: an unreachable Vault gives `503 secret_store_unavailable` (see [vault.md](vault.md)).

## Recovery

1. The database comes back: one successful probe **and** an established LISTEN subscription move `degraded` → `ready`.
2. The gateway re-subscribes **first** and flushes **after**: it keeps all caches while reconnecting (jittered backoff 0.5 s → 10 s),
   re-issues `LISTEN` on every channel, then flushes every cache except leaf certificates (invalidations sent during the gap
   were missed; the flush covers them), re-polls the active registry hash and returns to `ready`. If the reconnect keeps failing the state
   stays `degraded` and entries age out at their TTL: at no time is anything served beyond TTL.
3. Cache misses after the flush are database lookups, coalesced per key, so expect a short burst of load on the pool.
4. A LISTEN connection that stops delivering without closing is detected by a heartbeat every `GATEWAY_DB_PROBE_INTERVAL`; two missed in a row
   make the listener down (`degraded` at once) and trigger the same reconnect-then-flush.
5. The request-log buffer is flushed, and approval rows of holds that expired meanwhile are corrected (docs/06).

A gateway that **started** while the database was down has no snapshot and stays `down` until one loads (docs/06).

## Alert rules

From docs/15 §Observability. Adjust the thresholds to your service levels; these are the ones the design names.

| Alert | Expression | Severity and meaning |
|-------|------------|----------------------|
| Gateway degraded | `aurora_gateway_state{state="degraded"} == 1` for 1 min | warning: the database or the LISTEN connection is down; the gateway is serving from cache |
| Gateway degraded past its cache tolerance | the same expression for `gateway.resolve_cache_ttl_secs` | critical: the cache is running out and new requests are being refused |
| Gateway down | `aurora_gateway_state{state="down"} == 1` | page |
| Orphaned bots | `aurora_channel_deprovision_failed_total` increases, and audit rows `presence.deprovision_failed` | a bot account was not removed on the chat platform (connector down for more than 24 h or credentials revoked) and must be removed by hand (Mattermost: System Console → Integrations → Bot Accounts, disable or delete the account named by `platform_user_ref`). An org's shared bot is never deprovisioned by Aurora |
| NOTIFY queue | `aurora_pg_notify_queue_usage` ≥ 0.5 | warning; at 1.0 NOTIFY fails. Related: watch `aurora_delta_publish_seconds` p99 and `aurora_delta_dropped_total` (docs/15 §Streaming fan-out ceiling; [limits-and-approvals.md](limits-and-approvals.md)) |
| Counter fallback | `aurora_counters_state{state="fallback"} == 1` for 5 min | warning: rate limits are per replica again (phase 4). `rate(aurora_counters_fallback_total{policy="closed"}) > 0` is critical for `closed` classes: requests are being refused. `readyz` is not affected |
| Blob store down | `aurora_blob_store_up == 0` for 2 min | attachment uploads and downloads fail with 503; the rest of the API keeps working and `readyz` does not change (phase 5, D-138, S3 backend; the metric does not exist yet) |
| Parked approvals | a growing `aurora_approvals_parked_pending` | nobody is deciding (phase 5, D-138; the metric does not exist yet) |
| Push | sustained `result=forbidden`, or `result=rejected_host`, on `aurora_push_sent_total` | VAPID mismatch or a push service outside `push.allowed_endpoint_hosts` (phase 5, D-138; the metric does not exist yet) |

Other gateway series for dashboards (C-383): `aurora_gateway_db_unreachable_seconds`,
`aurora_gateway_degraded_requests_total{outcome="served_from_cache"|"refused", reason}`, `aurora_gateway_listener_reconnects_total`,
`aurora_gateway_cache_flush_total{reason="listener_reconnect"|"invalidate_all"}`, `aurora_gateway_request_log_buffered`,
`aurora_gateway_request_log_dropped_total`. `GET /internal/v1/status` (port 4103, internal-secret auth) also reports `state`,
`db_unreachable_since`, `listener_connected`, `cache` sizes and `request_log_buffered` (C-381).

## Recovery check

Run these after Postgres is back (or after a planned database maintenance). The gateway's `/readyz` is on port 4103, not behind the
edge, so reach it from inside the cluster or through a port-forward:

```bash
# any gateway pod (the chart labels them app.kubernetes.io/component=gateway)
GW=$(kubectl -n aurora get pods -l app.kubernetes.io/component=gateway -o name | head -n1)
kubectl -n aurora port-forward "$GW" 4103:4103 &
curl -fsS 'http://127.0.0.1:4103/readyz?strict=true'     # expect 200 {"state":"ready"}; 503 means degraded or down
curl -fsS http://127.0.0.1:4103/metrics | grep -E '^aurora_gateway_(state|request_log_(buffered|dropped_total))'
```

Expected: `?strict=true` answers 200 `{"state":"ready"}` on every gateway replica;
`aurora_gateway_state{state="ready"} 1` (and `degraded`, `down` at 0); `aurora_gateway_request_log_buffered` back to 0 and
`aurora_gateway_request_log_dropped_total` not increasing; the `degraded` alert has cleared. Then confirm through the proxy that a
previously refused request now works (an agent request that returned 503 `gateway_degraded` succeeds on retry), and that
`GET /v1/instance` through the edge answers 200.

Not covered here (docs/06 and docs/15 are the reference): the other services' behaviour during a database outage, and the Vault
outage behaviour, which is in [vault.md](vault.md).
