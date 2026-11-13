# API rate limits, approval timeouts, e-mail and streaming fan-out

(Phase 2, RFE-017 and RFE-011a; streaming fan-out is phase 1, RFE-016.) The normative text is [docs/05 §Conventions](../05-control-plane-api.md#conventions) (rate limiting) and
[docs/07 §Approval deadlines](../07-policy-approvals.md) (approvals). In Helm, put the keys under `config.ratelimit`, `config.approvals` and `config.email`.

## API rate limits

Every authenticated `/v1/**` request passes the API limiter, after the caller is identified and before authorization. The caller is an API key, a signed-in user (all of a user's sessions share one budget) or a BYO agent token.
Webhook replay: a signed webhook delivery without a delivery id is deduplicated for 60 seconds only, so replay protection of older captured requests belongs to the sender (include a delivery id or a timestamp the receiver of your own agent can check). Signed-timestamp checking is a later item.

Not limited here: `/healthz`, `/readyz`, `/metrics`, the runner, connector and `/internal` planes, and webhook deliveries (which have their own limit). Unauthenticated routes have separate per-IP limits.

A request must get a token from **every** bucket that applies: the caller's `general` bucket, the caller's class bucket and the organization's bucket. Buckets are kept in memory **per API replica**, so with N replicas the effective limit can be up to N times higher.

| Class | Applies to | Default refill | Default burst |
|-------|-----------|----------------|---------------|
| `general` | every authenticated request | 600 per minute | 120 |
| `write` | any POST, PUT, PATCH or DELETE not in a stricter class | 120 per minute | 30 |
| `message` | posting a message, send-now | 30 per minute | 10 |
| `upload` | attachment, avatar, skill-file and plugin uploads | 20 per minute | 5 |
| `simulate` | policy simulation, an agent's effective view | 20 per minute | 5 |
| `search` | memory search | 30 per minute | 10 |
| `export` | conversation export, the two storage routes | 6 per hour | 2 |
| organization bucket | all callers touching one organization | `org_per_minute` (3000 per minute) | one fifth of it (600) |

| Key | Default | Meaning |
|-----|---------|---------|
| `ratelimit.api.enabled` | `true` | Turn the limiter off. |
| `ratelimit.api.multiplier` | `1.0` | Scales every bucket. 0.1 to 100. Raise it when service keys legitimately poll often. |
| `ratelimit.api.org_per_minute` | `3000` | Organization bucket refill per minute, at least 1. |
| `ratelimit.api.classes.<name>` | the table above | Override one class with `{ per_minute \| per_hour, burst }`. `<name>` is one of the seven class names above (an unknown name fails startup). Set `per_minute` **or** `per_hour`, not both; every number is at least 1; omitted fields keep the default. |

```toml
[ratelimit.api]
multiplier = 2.0
org_per_minute = 6000

[ratelimit.api.classes.message]
per_minute = 60
burst = 20

[ratelimit.api.classes.export]
per_hour = 12
```

Refusals are HTTP 429 with code `rate_limited`, the header `retry-after` and `details` naming the scope (`general`, `class` or `org`), the class and the limit. Streams are capped separately at 20 concurrent SSE connections and long-polls per caller (429 `too_many_streams`).
Verify with the metric `aurora_api_rate_limited_total{class,scope}`, and by sending a burst of requests to one class and watching for the 429.

## Approval timeouts

A request held for approval is denied when its deadline passes. The deadline comes from the rule that required the approval, within instance limits.

| Key | Default | Meaning |
|-----|---------|---------|
| `approvals.timeout_secs` | `300` | Deadline of a hold whose rules set no timeout. Must not exceed `approvals.max_timeout_secs`. |
| `approvals.max_timeout_secs` | `3600` | The longest timeout a rule or grant may ask for. 300 to 14400, and it must be **below `hosted.turn_ceiling_secs` minus 900** (`hosted.turn_ceiling_secs` defaults to 21600), or startup fails naming the key. |

Rule authors set `approval_timeout_secs` on a rule, or `ask_timeout_secs` on a connection grant, from 60 seconds up to the maximum (otherwise 422 `approval_timeout_out_of_range`). When several rules apply the **shortest** timeout set wins.
While an approval is pending, the turn is exempt from stall detection.

```toml
[approvals]
timeout_secs = 600
max_timeout_secs = 7200
```

Verify: the approval card and the transcript event `approval.pending` show `expires_at`; `POST /v1/workspaces/{ws}/policy/simulate` shows the timeout a request would get.
Both services that read these keys (API and gateway) must have the same values.

## Approval e-mail

When an approval has no live place to be decided (a schedule or webhook turn with no chat presence, and nobody with the approvals page open), the API e-mails the people who can decide it, 20 seconds after the approval was created.
The e-mail carries the agent and workspace names, the approval title, the rule name, the expiry and a link to the approvals page. It never contains an approve or deny link, because deciding needs a signed-in session.
Approvals of one batch produce one e-mail, and each person gets at most 20 per hour (the rest arrive as a digest). A person can turn it off for themselves (`PATCH /v1/me { approval_email: false }`).

| Key | Default | Meaning |
|-----|---------|---------|
| `approvals.email_notifications` | `true` | `false` disables approval e-mail for the whole instance. |
| `email.smtp_url` | unset | `smtp://` or `smtps://` URL, which may carry the password. Unset means **log-only**: mail is written to the API log, nothing is sent, and the instance overview warns "approval e-mails disabled". |
| `email.from` | unset | Sender address. **Required** when `email.smtp_url` is set (startup fails otherwise). |

The same mailer sends invitations. Recipients are the people with an explicit `owner` or `approver` workspace binding who can see the conversation; if the workspace has no explicit binding, the organization owners and admins.

```toml
[email]
smtp_url = "smtps://aurora:<password>@smtp.example.org:465"
from = "aurora@example.org"
```

Verify: with `smtp_url` unset, trigger an approval from a webhook or schedule and look for the e-mail in the API log. With it set, check the mailbox of a workspace owner after about 20 seconds.
Details: [docs/07 §E-mail when no surface is live](../07-policy-approvals.md).

## Streaming fan-out (events)

(Phase 1, RFE-016.) Streaming text and activity lines travel over a `DeltaBus`. Scope: API only; in Helm, `config.events`.

| Key | Default | Meaning |
|-----|---------|---------|
| `events.delta_bus` | `notify` | Which `DeltaBus` implementation carries deltas. `notify` (Postgres `NOTIFY`) is the only one; any other value fails startup naming `events.delta_bus`. |
| `events.delta_coalesce_ms` | `100` | Producer-side coalescing window of `text.delta` / `thinking.delta`: a delta is sent once this many milliseconds passed since the last one for the same message, or when 2 000 characters are buffered (that threshold is fixed). 100 to 500, otherwise startup fails naming the key. Raise it to cut the notification rate (at the cost of a choppier stream) when approaching the ceiling. |

```toml
[events]
delta_bus = "notify"
delta_coalesce_ms = 250
```

Verify: open a conversation stream while an agent writes a long answer; deltas arrive about every `delta_coalesce_ms`. To re-measure the `notify` ceiling on your Postgres, run `bash scripts/perf/delta_bus.sh` from a source checkout (release build; the role in `DATABASE_URL` needs `CREATEDB`; it works in a throwaway database it drops again). It prints PASS or WARN against the planning numbers in [docs/15 §Streaming fan-out ceiling](../15-deployment-operations.md), and never fails on a slow machine.

## Shared rate-limit counters

(Phase 4, P4-CNT, D-119, SEC-62.) Normative text: [docs/15 §Shared rate-limit counters](../15-deployment-operations.md). By default every limit above is counted **per replica** (N replicas can allow up to N times the limit) and nothing needs configuring. To make replicas enforce one limit, pick a shared backend. In Helm use `counters.*`; in `aurora.toml` the keys live under `[ratelimit.counters]`.

| Key | Default | Meaning |
|-----|---------|---------|
| `backend` | `memory` | `memory` (per replica, no infrastructure), `postgres` (the two `UNLOGGED` tables, a small dedicated pool, no new infrastructure) or `redis` (your own Redis, TLS, no Cluster). Any other value fails startup. |
| `api`, `gateway` | `shared` | Share the API limiter and/or the gateway rule quotas. `memory` keeps that limiter per replica. |
| `on_unavailable.security` / `.management` / `.gateway` | `local` | What happens when the backend cannot be reached: `local` counts per replica until it recovers (today's strength), `closed` answers 503 `counter_unavailable` with `retry-after: 5`. **There is no `open`**: it, or any other value, fails startup. `security` = sign-in start/callback, rejected-bearer and unauthenticated per-IP limits; `management` = the authenticated buckets and webhooks; `gateway` = rule quotas. |
| `timeout_ms` | `50` | Per store call, 10–2000. Three consecutive failures open a breaker for 5 s. |
| `postgres.pool_size` | `4` | Dedicated pool, 1–32, separate from `database.max_connections`. |
| `redis.address` | — | `rediss://host[:port]`, no user, password, path or query. `redis://` only for loopback or with `allow_plaintext = true` (startup warning). |
| `redis.username_env`, `redis.password_env` | unset | **Names** of environment variables (Helm: `counters.redis.existingSecret` with keys `REDIS_USERNAME`, `REDIS_PASSWORD`). Never a value, never in the URL. |
| `redis.ca_file`, `redis.db`, `redis.key_prefix` | system roots, `0`, `aurora:rl:` | CA bundle, database index, key prefix. |

```toml
[ratelimit.counters]
backend = "postgres"
[ratelimit.counters.on_unavailable]
security = "closed"
```

- **Redis**: run it with `maxmemory-policy noeviction` or `volatile-ttl` (an LRU policy evicts live counters and resets limits). Persistence may be off. The ACL needs `EVALSHA`, `SCRIPT LOAD`, `GET`, `PING` on the prefix. With Helm set `counters.redis.networkPolicy.egressTo` so api and gateway may reach it.
- **Not shared** (stay per replica): the stream cap (20 per caller), CLI device-code polling and `cli_start`/`cli_approve`, BYO admission.
- **Keys** hold ids and keyed hashes only; e-mail addresses and IPs are hashed with a key derived from the signing key (rotating it restarts the affected windows once).
- **Switching** `backend` is a rolling restart; usage resets once. A mixed fleet is safe (each replica counts what it sees).
- **Watch**: `aurora_counters_state{state="fallback"} == 1` for 5 min means limits are per replica again (warning); a rate above 0 of `aurora_counters_fallback_total{policy="closed"}` means requests are being refused (critical). `GET /v1/admin/overview` shows `counters: { backend, state }`. `readyz` is not affected.
