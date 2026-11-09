# API rate limits, approval timeouts and e-mail

(Phase 2, RFE-017 and RFE-011a.) The normative text is [docs/05 §Conventions](../05-control-plane-api.md#conventions) (rate limiting) and
[docs/07 §Approval deadlines](../07-policy-approvals.md) (approvals). In Helm, put the keys under `config.ratelimit`, `config.approvals` and `config.email`.

## API rate limits

Every authenticated `/v1/**` request passes the API limiter, after the caller is identified and before authorization. The caller is an API key, a signed-in user (all of a user's sessions share one budget) or a BYO agent token.
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
