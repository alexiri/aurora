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

## Completion report
Done (2026-11-19, lane `p5-apps`; commits `6d6c90be` part 1 and the follow-up).
- **Migration (deviation from "No migration").** The home-bump trigger on `app_connections` fired
  on label/status/app changes only, so a config-only `PUT …/webhook` did not re-render. Migration
  `20261201000256_connections_home_bump_config.sql` adds `config` to its condition (function only,
  no table change: docs/03 untouched; `extract-schema --check` clean). Connect, delete (status) and
  grant changes already bump through the existing triggers; a reconnect that changes no config
  renders the same files.
- **home.rs / db.** `HomeAppConn` gains `config`; `webhook_input` builds `GuideAccount.webhook`
  (name = the connection label, note, method, slots) for send-mode apps only; `links` keeps
  fetch-mode connections only.
- **Package snippet.** `plugins/builtin/webhook/plugin.yaml`: the dry run is `X-Aurora-Dry-Run=1` to
  `aurora-agent call` (the header form is for `curl`), and a `json` field is `field:=<json>`.
- **Tests.** `a_webhook_is_an_agent_operation_that_follows_its_edits` (API IT): a granted agent's
  `api.json`/`tools.md` have the operation and slots, no URL/secret/header/template text, `links`
  empty, a slot-only PUT moves the home generation and the next render shows the new slot, revoking
  the grant removes it. Seen failing without the migration ("a config-only edit moves the home
  generation"). Registry snapshots, `aurora-agent` ITs: see part 1.
- **Gates run.** fmt, clippy (registry, supervisor, api, db, cli), guards, `extract-schema --check`,
  `aurora-registry` full, supervisor `agent_cli`, api `outgoing_webhooks`/`link_connect`/
  `home_agent_guides`. `the_send_test_posts_the_rendered_body_…` (P5-WEBHOOK-2's) failed in 2 of 4
  runs (the `webhook.test_sent` audit row was not yet there) and passed in the others: a timing
  flake in that test, not touched here. Not run: the dev-stack end-to-end (CI/Orchestrator).
