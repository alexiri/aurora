# P5-WEBHOOK-2: Outgoing webhook — storage, API routes, settings and limits, the gateway send path, the `slots` approval decoder (owner, 2026-11-15, phase 5, D-320, C-909, C-910, C-912, SEC-127)

## Why
P5-WEBHOOK-1 built the pure model (`crates/aurora-webhook`), the registry's send mode and the
`webhook` package. This brief makes it work end to end:
- the person connects and edits a webhook;
- an agent's request is admitted, approved, rate-limited, rendered and sent by the gateway to the
  stored URL;
- nothing secret leaks.

Read first: `P5-WEBHOOK.md`; `API_CONTRACTS` **C-909** (Storage, Connect, Edit, Preview, Presets,
Send test), **C-910** (all of it) and **C-912**; C-843 and the gateway `link` module P5-ICS wrote; `17`
SEC-127 and SEC-107; `15 §Instance settings`; D-119 (the shared counters).

## Scope

### 1. Storage
- **Migration `20261201000243`:** `app_connections.config jsonb null` with the CHECK of C-909, and a
  gateway-role `SELECT` grant on the column if the role's grants are column-level.
- Update `03 §app_connections`.

### 2. API (`services/aurora-api`)
- **Connect** for a send-mode app (C-909):
  - URL validation as C-843;
  - a DNS resolve plus the destination guard and the org egress policy, **with no request sent**;
  - `aurora_webhook::validate`;
  - seal `url`, `secret_values` and `signing_secret` in the credential JSON (`credential_keys` lists
    the names), and store `config`;
  - `metadata.host`, as ICS does;
  - return a generated signing secret once.

  It refuses with C-909's codes, and with 403 `outgoing_webhooks_disabled` when the org switch is off.
  A reconnect keeps `config` unless one is given.
- **`GET`/`PUT /v1/connections/{id}/webhook`**, with `effective_rate` (C-912). This needs the existing
  right to manage the connection.
  - A secret change bumps `credentials_version`.
  - Every change fires the connection-change signal the gateway already uses (P5-ICS names it).
- **`POST /v1/apps/webhook/webhook-preview`** (no network) and **`GET /v1/apps/{app}/webhook-presets`**
  (from the snapshot P5-WEBHOOK-1 fills).
- **`POST /v1/connections/{id}/webhook/test`:**
  - it uses the same upstream code as the gateway (§4): the same crate, guard, egress, redirect rule and
    rate counter;
  - put that client in one shared place both services can use, the way ICS's connect fetch reuses
    the guard, so it is not written twice;
  - the person is the actor;
  - best-effort audit `webhook.test_sent`.
- **No read path returns the URL, a secret value or the signing secret** (connection reads, exports,
  audit metadata, errors).
- **Settings (C-912):**
  - the instance section `outgoing_webhooks` as movable keys, in the SET-1 framework (registry entry,
    validation, Helm lock, `aurora-api settings show`);
  - the org settings `outgoing_webhooks.enabled`, `.per_minute_max` and `.per_day_max`, stored and
    authorized exactly like `calendar_invitations.enabled` (P5-CALINV). **If P5-CALINV is not merged
    when you start, stop and report** rather than inventing a second org-settings store;
  - the effective-limit function lives in one place and is used by the API and the gateway.
- **OpenAPI and `web/src/lib/api/schema.d.ts`** regenerated, and the permission matrix entries for the
  new routes.
- Update `05` with the routes.

### 3. Approval decoder `slots` (`07`, `09 §Decoders`)
It produces the C-910 summary from the checked values. It shows no fixed template part, header or
secret, and clamps each value to 500 characters with the full length shown. The `send` tool's default
rule is **Ask**. If P5-WEBHOOK-1 reported that the manifest has no per-tool default, add the smallest
mechanism that gives a package tool a default `ask` and record it in `09`.

### 4. Gateway (`services/aurora-gateway`, the `link` module)
Implement the send mode exactly as C-910 says.
- **Order:**
  1. admission (grant, policy, Read only, org switch);
  2. method;
  3. content type and size;
  4. `check_values` — before any approval is created;
  5. the dry-run branch;
  6. approval;
  7. the rate take;
  8. render;
  9. upstream.
- **Upstream headers:** only those C-910 lists. The agent's headers never go upstream.
- **Redirects:** `307`/`308` resend the method and body with a fresh signature; `301`/`302`/`303`
  continue as a body-less `GET`; ≤ 3 hops; `https`; guard and egress per hop.
- **Time-out** `WEBHOOK_TIMEOUT`. There is no retry and no cache.
- **The response:** status mapping, the header allowlist, the body cap with `X-Aurora-Truncated`, the
  echo check with SEC-127's needles (URL, path-with-query raw and percent-encoded, every secret value).
- **Refusals** use the `06 §Errors` body plus `X-Aurora-Error`, including `may_have_been_delivered`.
- **Request log and spans:** C-910's fields only.
- **Rate:** the effective limit (C-912), both windows taken all-or-nothing on the shared counters.
- Update `06` with the send mode of the secret-link section.

### 5. Docs
`03`, `05`, `06`, `07`, `15` (the section and the org settings), `18` (the tests below). SEC-127 is
already in `17`; fill in its "Proven by" with the test names.

## Acceptance criteria
- [ ] **Migration** up/down on a copy of the dev database; `upgrade.sh` green.
- [ ] **API ITs:**
  - connect success, and each refusal code (including a private address refused without
    `allowed_private`, the org switch off, and a bad config with field paths);
  - no request reaches a mock receiver at connect;
  - a generated signing secret is returned once and never again;
  - GET/PUT edit (a secret set, deleted and kept; signing keep, generate, provided, off);
  - reconnect keeps the config;
  - preview and presets;
  - send test (real and dry run, rate-counted, audited);
  - the settings section (lock, validation) and org settings (above the ceiling refused);
  - `effective_rate` source for each combination;
  - **canaries** in the URL, the secret values and the signing secret appear in no response, log line,
    audit row or export.
- [ ] **Gateway ITs** (a loopback mock receiver):
  - an ungranted agent and a Read-only connection are refused;
  - a wrong method → 405;
  - a non-JSON body → 415, oversized → 413;
  - `invalid_slot` arrives before any approval exists;
  - dry run: the masked body, no upstream call, not rate-counted, and an `ask` rule does not hold it;
  - an approval is created under Ask, and the request is sent after approve and refused after deny;
  - the receiver gets exactly the rendered body and the allowed headers (the agent's `Authorization`,
    `Cookie` and custom headers are absent), and the signature verifies;
  - `X-Aurora-Delivery` is the same in the response and the request log;
  - `307` keeps the body, `303` becomes `GET`, a fourth hop is refused, and a redirect to `http` or
    a private address is refused;
  - 404 → `link_not_found`;
  - a time-out → 504 with `may_have_been_delivered: true`, and the mock saw exactly one request;
  - a connect failure → 502 `link_unreachable` with `false`;
  - an echo of a secret value → 502;
  - a response over the cap is truncated with the header;
  - the rate limit per minute and per day, lowest of instance, org and connection, and a lowered org
    ceiling applies to an existing connection at once;
  - org switch off → 403;
  - the request log row and spans hold the slot names and not their values (canary).
- [ ] **Approval:** the decoder's summary shows the slot values and no canary from the template or
  secrets.
- [ ] **End-to-end on the dev stack:** an agent posts with `curl` to the link host and a recorded
  receiver gets the body (the `aurora-agent` path is P5-WEBHOOK-3).
- [ ] **Live check:** one real incoming webhook the owner provides (a test Mattermost or Slack channel),
  at `~/.config/aurora-dev/p5-webhook-test-url` (`0600`, outside the repo). Never copy it into the repo,
  fixtures, logs or a report. If the file is absent, report it and skip.
- [ ] `cargo fmt --check`, clippy, the targeted `aurora-api`/`aurora-gateway`/`aurora-db` suites, the
  OpenAPI drift guard, the permission matrix; the full suites at the lane's batch gate.

## Ownership and sequencing
- **Owns:**
  - the migration;
  - `services/aurora-api` (connect path for send mode, the webhook routes, settings section, org
    settings);
  - `services/aurora-gateway` (the `link` module's send path and the decoder registration);
  - the decoder's crate (where `json`/`form` decoders live);
  - the shared webhook upstream client;
  - the docs above.
- **After P5-WEBHOOK-1, in the same lane.** The Admin rendering of the section needs SET-5's generic
  form (dashboard, P5-WEBHOOK-4).

## Numbers
D-320, C-909, C-910, C-912, SEC-127, migration `20261201000243`.

## Suggested agent/model and reason
`rust-developer` (Sonnet). It spans API, gateway, database and policy like P5-ICS. The security tests
are most of the work.
