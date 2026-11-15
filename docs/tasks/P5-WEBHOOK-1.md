# P5-WEBHOOK-1: Outgoing webhook — the `aurora-webhook` crate, the registry's send mode and the `webhook` package (owner, 2026-11-15, phase 5, D-320, C-909, C-912, SEC-127)

## Why
The owner wants agents to call webhooks whose secret URL the person stores, as with the ICS calendar
link (D-273). The agent fills in only the typed blanks ("slots") of a body template the person wrote.
The design is in `docs/tasks/P5-WEBHOOK.md`; the contracts win where they differ. This brief builds
the pure parts that the API (P5-WEBHOOK-2), the gateway (-2) and the `aurora-agent` renderer (-3) all
use, plus the package.

Read first: `P5-WEBHOOK.md` §2 and §5; `API_CONTRACTS` **C-909** (all of it), C-912's constants, and
C-843 (the secret-link lane it extends); `17` SEC-127 and SEC-107; `09 §3.4` as P5-ICS left it.

## Scope

### 1. New crate `crates/aurora-webhook` (pure: no IO, no async, no database)
This is the single implementation of C-909's model. The API and the gateway must not re-implement it.
- **Types:** `WebhookConfig`, `Slot`, `SlotType`, `HeaderTemplate` with `serde` and
  `deny_unknown_fields`, exactly as C-909 says.
- **`validate(config, secret_value_names) -> Result<CompiledConfig, Vec<FieldError>>`.** It checks
  every rule of C-909 and returns `{ path, code }` with C-909's codes:
  - names, duplicates, limits;
  - every slot and secret value is used;
  - JSON validity with placeholders;
  - `json` slots are refused for form and text;
  - optional slots at the root;
  - the schema subset;
  - header names and the denylist; a slot placeholder in a header is refused;
  - sizes.

  `CompiledConfig` holds the parsed template (an AST of constant parts and placeholders), so render
  never parses text again.
- **`check_values(compiled, values: &serde_json::Value) -> Result<CheckedValues, SlotError>`.** The
  input must be an object. `SlotError { slot, rule }` uses the rules `missing`, `unknown`, `type`,
  `max_length` (counted in Unicode scalar values), `max_items`, `enum`, `schema` (the C-909 subset only;
  write a small validator, no new heavy dependency without asking).
- **`render(compiled, checked, secrets: &SecretValues) -> Rendered { content_type, body: Vec<u8>, headers: Vec<(name, value)> }`**:
  - **one pass over the AST.** Inserted text is never scanned for placeholders again, so a value
    containing `{{secret:x}}` stays literal;
  - **JSON:** a value outside a string literal is serialized by `serde_json`. Inside a string, the
    scalar is escaped as string content. A left-out optional removes its member or element, or is empty
    inside a string;
  - **form:** `application/x-www-form-urlencoded` encoding of every inserted value. A `string_list`
    repeats the key, and a left-out optional pair is dropped;
  - **text:** values are inserted as UTF-8;
  - `render_masked(...)` is the same with secrets as `«secret:name»`, for previews and dry runs.
- **`sign(secret, t, body) -> String`**: the `X-Aurora-Signature` value `t=<t>,v1=<hex>` of C-910.
  Use the HMAC/SHA-256 crates already in the workspace.
- **Constants:** add C-912's `WEBHOOK_*` constants to `crates/aurora-core/src/limits.rs`, and use
  them from there.

### 2. Registry (`crates/aurora-registry`)
- `secret_link` gains `mode` (`fetch` default, `send`) and `presets: [file]`. The send-mode rules and
  the linter codes are those of C-909. The ICS package must stay valid and unchanged.
- **Presets:**
  - each preset is a JSON file in the archive,
    `{ id, name, suggest_for_hosts?, secret_value_names, config: WebhookConfig }` (C-909);
  - each is validated with `aurora_webhook::validate(config, secret_value_names)`, and a failure is
    `webhook_preset_invalid`;
  - presets are carried in the snapshot so the API can serve them (C-909 `GET …/webhook-presets`;
    the route itself is P5-WEBHOOK-2).
- `aurora plugin test`: fixtures may send `POST https://<uuid>.link.aurora.internal/` with a JSON
  values body, answered from a recorded upstream that receives the rendered body.

### 3. Package `plugins/builtin/webhook/`
- **App:** **Outgoing webhook**, the closest existing category, a neutral icon.
- **Connect method:** one `fields` method (`name`, `url`), as C-909 says.
- **Tool:** one tool `send`, `access: write`, `POST` on `*.link.aurora.internal` `/`, with the
  package's default rule **Ask** (D-320(d); use the manifest's existing per-tool default mechanism;
  if none exists, report it, and P5-WEBHOOK-2 adds it with the decoder).
- **The nine presets of C-909.** Check each receiver's current documented payload while writing it;
  note the source URL in a comment field of the preset file if the format allows, else in `setup.md`.
- **`docs/setup.md`:**
  - where each receiver shows its incoming-webhook URL;
  - "the URL is a password";
  - revoking is deleting or regenerating at the receiver, then reconnecting;
  - signing: a receiver-side verification snippet (Python and Node), checked against the unit
    vectors;
  - why there are no automatic retries, and what `X-Aurora-Delivery` is for.
- **The agent snippet** (≤ 2,000 chars, the package's guide text):
  - discover with `aurora-agent list webhook` / `describe`;
  - follow each webhook's notes;
  - use the dry run when unsure;
  - after `webhook_timeout`, do not resend without asking, because it may have been delivered;
  - a gateway refusal (`X-Aurora-Error`) is not the receiver's answer.

### 4. Docs
- `09 §3.4`: the send mode, presets and linter codes.
- The built-in catalogue list.

## Not in this brief
API routes, storage, the gateway, approvals, settings (P5-WEBHOOK-2); `api.json` and `aurora-agent`
(-3); the dashboard (-4).

## Acceptance criteria
- [ ] **Crate unit tests:**
  - every C-909 validation code has at least one failing config;
  - each slot type is accepted and refused per rule;
  - **injection:** values containing `{{secret:a}}`, `{{x}}`, `"`, `\`, `}`, `]`, `&`, `=`, `%`, CR/LF,
    NUL and non-BMP characters are inserted literally and correctly escaped for JSON (in and out of
    string literals), form and text, and the output parses back to the same values;
  - optional slots left out in each position;
  - a `json` slot with and without a schema;
  - the masked render;
  - signature vectors (fixed secret, `t` and body → the expected hex), which `setup.md`'s snippets
    reproduce.
- [ ] **A property test** (proptest is already used? check; else a seeded loop): random strings in every
  slot of a JSON template always render to valid JSON, with the value round-tripping.
- [ ] **Registry:** the package passes the linter; each C-909 linter code refused by a fixture
  manifest; the ICS package is unchanged and still valid; a bad preset → `webhook_preset_invalid`;
  `aurora plugin test` fixture: a `POST` of values → the recorded upstream receives the rendered body.
- [ ] `cargo fmt --check`, clippy (workspace lints), and `cargo test -p aurora-webhook -p aurora-registry`.

## Ownership and sequencing
- **Owns:**
  - `crates/aurora-webhook/**` (new; add it to the workspace `Cargo.toml` members, which is the only
    edit to that file);
  - `crates/aurora-registry` (secret-link model, linter, plugin test);
  - `crates/aurora-core/src/limits.rs` (the `WEBHOOK_*` constants only);
  - `plugins/builtin/webhook/**`;
  - `09 §3.4`.
- **Runs after P5-ICS is merged** (it extends `secret_link`), in the same lane (`p5-apps`).
- The lockfile changes only by the new workspace member.

## Numbers
D-320, C-909, C-912 (constants), SEC-127. No migration in this brief.

## Suggested agent/model and reason
`rust-developer` (Sonnet). A self-contained parser, renderer and validator with heavy unit tests, plus
a registry extension modelled on P5-ICS's.
