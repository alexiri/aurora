# P5-WEBHOOK-3: Outgoing webhook — every webhook is an `aurora-agent` operation (owner, 2026-11-15, phase 5, D-320(g), C-911, SEC-127)

## Why
Owner, 2026-11-15: "everything should be exposed through `aurora-agent`". An agent discovers and calls
the person's webhooks with the one in-sandbox command (P5-GUIDE-6):
- `aurora-agent list webhook`;
- `aurora-agent describe webhook send_ops_alerts`;
- `aurora-agent call webhook send_ops_alerts text="Deploy 42 failed"`.

The slots are the operation's typed body fields. There is no separate `webhooks.json`.

Read first: `API_CONTRACTS` **C-911**, and C-651..C-653 (operations file, `api.json`, `aurora-agent`);
`24 §5.2`, `§5.4`, `§10a`; `P5-WEBHOOK.md` §3; SEC-127 and SEC-118..SEC-120.

## Scope

### 1. Render inputs and `api.json` (`crates/aurora-registry/src/agent_guide.rs`, the home sync)
- **Render inputs:** the `accounts[]` of a send-mode app gain
  `webhook: { name, description, method, slots }`, built by the API from `app_connections.config`.
  They never include the template, the headers, the URL or the secrets. A test asserts that the
  type has no such field.
- **`api.json`** for that app, per C-911:
  - one operation per account, with the id slug and suffix rule;
  - the `host`, the dry-run header param and the body fields from the slots (`json` type new);
  - `access` for its own letter only;
  - the new keys `account` and `notes`, and `source: "connection"`.

  Every other operation gets `account: null, notes: null`. The output stays deterministic, and the
  `AGENT_API_JSON_MAX` drop order of `24 §5.4` applies.
- **`tools.md`:** a webhook section per account (name, notes, slots table, a `curl` example to its
  `base_url`).
- **Re-render on change:** editing a webhook (`PUT …/webhook`), connect, reconnect, delete and grant
  changes must re-render the agent's `apps` part. Find what triggers the home sync for connection
  changes today and make sure a config-only change triggers it too. Test it.
- The home sync's `links` (C-843) keeps only fetch-mode connections. Assert that a send-mode connection
  never appears in `links.json`.

### 2. `aurora-agent` (`services/aurora-supervisor/src/agent_cli/`)
- **An operation with `account`:**
  - it uses that account;
  - `--account` naming another is refused with exit 2;
  - several accounts on the app are not an error.
- **The `json` field type:** accepted only as `name:=<json>`. `name=value` for a `json` field is exit 2
  with a hint.
- **`describe`** prints the notes, the slot table and two example lines: send, and the dry run
  `X-Aurora-Dry-Run=1`.
- **`search`** indexes webhook names, notes and slot names.
- **Exit codes:** a gateway refusal recognised by `X-Aurora-Error` or the `06 §Errors` body exits 3,
  and the receiver's 4xx/5xx exits 1. Print `may_have_been_delivered` when present.
- The output and preview discipline is unchanged.

### 3. Docs
- `24 §5.2`, `§5.4` (the new keys and type), `§10a` (account-bound operations).
- The `webhook` package's agent snippet (P5-WEBHOOK-1): check that it matches the real commands.

## Acceptance criteria
- [ ] **Renderer snapshots:**
  - two webhooks with the same name → `send_x`, `send_x_2`;
  - slot type mapping;
  - `notes` clamped;
  - deterministic output;
  - **canaries** in the URL, the secret values, the header values and the fixed template parts appear
    in neither `api.json` nor `tools.md`.
- [ ] **API IT:** a `PUT …/webhook` changing a slot re-renders `api.json` in the next home sync. A
  send-mode connection is absent from `links`.
- [ ] **`aurora-agent` unit tests:**
  - an account-bound call needs no `--account`, and another account is refused;
  - a `json` field with `:=` is accepted and with `=` refused;
  - dry run;
  - exit 3 on a gateway refusal and 1 on a receiver 400.
- [ ] **End-to-end on the dev stack:** an agent runs `aurora-agent search alert`, finds the webhook,
  `describe`s it and `call`s it. The recorded receiver gets the rendered body. A dry run sends nothing.
- [ ] `cargo fmt --check`, clippy, `cargo test -p aurora-registry -p aurora-supervisor` and the
  targeted `aurora-api` home-sync tests.

## Ownership and sequencing
- **Owns:**
  - `crates/aurora-registry/src/agent_guide.rs` and its snapshots;
  - `services/aurora-supervisor/src/agent_cli/**`;
  - the home-sync render-input code in `services/aurora-api/src/domains/hosted/home.rs`;
  - `24`.
- **After P5-GUIDE-6** (which creates `api.json` and `aurora-agent`) **and P5-WEBHOOK-2**. Same lane
  as both (`p5-apps`).

## Numbers
D-320, C-911, SEC-127. No migration.

## Suggested agent/model and reason
`rust-developer` (Sonnet). A renderer extension and CLI behaviour, in code the same lane will have
just written for P5-GUIDE-6.
