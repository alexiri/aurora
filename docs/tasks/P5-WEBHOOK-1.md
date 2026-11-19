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

## Completion report
**Commits** (branch `alexiri/p5-agent`): `6b0f2d14` (the crate, limits, workspace member) and the
registry/package/docs commit that carries this report.

**Crate `aurora-webhook`** (pure; deps `aurora-core`, `serde`, `serde_json`, `hmac`, `sha2`, `hex`;
dev `proptest`; no new external crate):
- `model`: `WebhookConfig`, `Slot`, `SlotType`, `HeaderTemplate`, `Rate`, `WebhookPreset`, `Method`,
  `ContentType` (`deny_unknown_fields`; `version` must be 1 at deserialization), `FieldError`,
  `SlotError`/`Rule`, `SecretValues` (Debug redacted), `Rendered`, `RenderError`.
- `validate(config, secret_value_names) -> Result<CompiledConfig, Vec<FieldError>>` with every C-909
  code: `template_json`, `template_syntax`, `placeholder_unknown`, `slot_unused`, `secret_unused`,
  `slot_name`, `slot_duplicate`, `slot_type_content_type`, `slot_limits`, `optional_slot_at_root`,
  `schema_unsupported`, `header_name_denied`, `header_placeholder`, `rate_above_ceiling` (static
  maxima 600/min and 100,000/day only; the live ceiling is the API's), `too_large`. The template is
  parsed once into an AST (a JSON-aware parser for `{{…}}` in value and string positions).
- `check_values(compiled, &Value) -> Result<CheckedValues, SlotError>`: rules `missing`, `unknown`,
  `type`, `max_length` (Unicode scalar values), `max_items`, `enum`, `schema` (own validator of the C-909
  subset, `schema.rs`). A `null` counts as left out.
- `render` / `render_masked`: one pass over the AST; JSON (`serde_json` for values, string-content
  escaping inside literals, members/elements of left-out optionals dropped), form (percent-encoding,
  `string_list` repeats the key, left-out optional pair dropped), text (UTF-8). `sign(secret, t, body)`.
- Constants `WEBHOOK_*` in `limits.rs`. `Cargo.lock` changes only by the new workspace member
  (`aurora-webhook`) and `aurora-registry`'s dependency on it; `Cargo.toml`: the member line only (the registry takes a `path` dependency).

**Choices where C-909 is silent** (all stricter, none loosens a rule; tell me if you want another):
1. `render` returns `Result<Rendered, RenderError>` (not a bare `Rendered`): `TooLarge` (body over
   `WEBHOOK_BODY_MAX`), `MissingSecret`, `HeaderValue` (a secret with CR/LF/control characters would
   enter a header). The caller maps them (413 `rendered_too_large`, 502, 502).
2. A `json`/`string_list` slot inside a JSON string literal, a `json` slot in a form value, a
   `string_list` anywhere in a text body or inside a larger form value → `slot_type_content_type`; a
   `string_list` may be a whole form value (repeats the key).
3. A placeholder in a JSON object key → `template_json`. A form pair without `=`, an empty pair, a
   placeholder in a key → `template_syntax`. Form constants are used as typed (not re-encoded).
4. Duplicate header names → `slot_duplicate`; more than 4 headers → `too_large`; a header value over 2 KiB
   → `too_large`; control characters in a header value → `template_syntax`.
5. `json` slot value `null` = left out (so a required one is `missing`); an `integer` must be a JSON integer
   (`3.0` is `type`).
6. `rate` is checked only against the static maxima; a zero is the API's to refuse.

**Registry**: `secret_link.mode` (`fetch` default, `send`; unknown value = parse error) and `presets: [file]`
(`SecretLinkMode`, `preset_file_ok`, `PRESETS_DIR`); a send-mode app declares only the URL credential
(`secret_link_mode`), no `expect_prefix` (`secret_link_send_expect_prefix`), exactly one `write` tool with
methods ⊆ {POST, PUT} on `*.link.aurora.internal` `/` (`secret_link_send_tool`); presets in `fetch` mode
→ `secret_link_presets_mode`; names/count/duplicates and the archive rules (listed file present, no
unlisted `presets/*.json`, JSON shape, unique id, name, ≤ 8 plain DNS `suggest_for_hosts`, and
`aurora_webhook::validate(config, secret_value_names)`) → `webhook_preset_invalid` (`webhook.rs`,
called from `validate::package`). `Package` accepts `presets/<file>.json`. `webhook::presets(manifest, pkg)`
loads the validated presets per send-mode app. The ICS package is unchanged and still valid (its
manifest and snapshot bytes do not change: `mode`/`presets` are skipped when absent).

**Package `plugins/builtin/webhook`**: app *Outgoing webhook* (category `communication`), one `fields` method
(`name` metadata, `url` secret), one tool `send` (`write`, `POST`, `*.link.aurora.internal` `/`), nine
presets (`slack`, `mattermost`, `google-chat`, `discord`, `teams-workflow`, `ntfy-json`,
`home-assistant`, `generic-json`, `ifttt`), setup guide (receiver URL locations with each receiver's
documentation link, "the URL is a password", revoking, signing with Python and Node snippets, no
retries and `X-Aurora-Delivery`), the agent snippet (1,017 characters), fixtures `tests/send.yaml`,
a neutral icon pair (rendered with `rsvg-convert`, recorded in `ICONS.md`). The Python and Node
snippets were run against the unit vector (`whsec_test`, `t=1700000000`, `{"text":"hello"}` →
`4de4482f…9e80`, also checked with `openssl dgst`): valid → true, altered body → false, stale `t` → false.

**Not done / for you**
- **Receiver formats were written from my knowledge of each receiver's documented payload; I did not
  re-fetch the live documentation pages** (no web access in this lane). The setup guide links each
  receiver's documentation; worth a human re-check before release (Teams Workflows' Adaptive Card shape and
  the IFTTT `…/json/with/key/…` URL are the two most likely to drift).
- **Default Ask**: the manifest has no per-tool default mechanism (P5-TOOLASK, D-397, adds it, queued
  after P5-WEBHOOK-2). The package has an interim `approvals.send` with only a title (the check requires a
  template and a fixture asserting its title); P5-WEBHOOK-2's `slots` decoder replaces the body.
- **Presets in the snapshot**: not added. Putting them in the snapshot would change `compile_with_guides`'
  signature and its API/CLI callers (P5-WEBHOOK-2 territory); instead `aurora_registry::webhook::presets`
  reads them from the stored archive, which is how the API can serve `GET …/webhook-presets` "from the
  installed package version". Say if you want them in the snapshot proper.
- **`aurora plugin test` "recorded upstream receives the rendered body"**: the fixture runner is the
  gateway's `check-snapshot`. I made its secret-link case send-mode aware (POST/PUT served, other methods
  `method_not_allowed`, tool identified by the request's method, the approval summary template applied to
  the body: `check_snapshot.rs`), so the fixtures classify. Rendering a body and answering it from a
  recorded upstream needs the gateway send path and the connection's stored config (P5-WEBHOOK-2), so it is
  not here.
- Files outside "Owns" I had to touch, each minimal: `services/aurora-cli/src/plugin/pack.rs` (pack
  `presets/*.json`; without it the package cannot be built), `services/aurora-gateway/src/check_snapshot.rs`
  (above), `docs/02-engineering-conventions.md` (the limits row the `limits-readers` guard requires),
  `scripts/guards/limits-gaps.allow` (three `unread` entries until P5-WEBHOOK-2 reads them),
  `plugins/builtin/README.md`/`ICONS.md`. `docs/09 §3.4` documents the send mode. A "built-in catalogue
  list" does not exist as a file I could find (no list names `ics-calendar` outside its package, docs and
  tests), so there was nothing to add.
- P5-TOOLASK's registry part and this commit both touch `crates/aurora-registry/src/manifest/model.rs`
  (`SecretLink` only here); P5-POLISH-3 serialization is yours to arrange.

**Gates** (each through `heavy.sh`; the first two runs predate the two-slot change):
- `cargo test -p aurora-webhook` (proptest 300 cases each, once at `PROPTEST_CASES=3000`): 33 passed.
- `cargo nextest run -p aurora-webhook -p aurora-registry`: 199 passed (webhook 33, registry unit + `it` incl.
  15 new in `webhook`/`secret_link`); `-p aurora-webhook -p aurora-registry -p aurora-cli`: 244 passed;
  `-p aurora-gateway --lib`: 88 passed. **Not run:** the gateway's and core's `tests/it` suites and
  `aurora-api` (a `check_snapshot` link-case edit and a constants edit; no DB was used in this lane).
- `SKIP_CARGO=1 bash scripts/plugins/check-builtin.sh webhook ics-calendar`: `ok webhook`, `ok ics-calendar`
  (after `cargo build -p aurora-cli -p aurora-gateway`).
- `cargo clippy -p aurora-webhook -p aurora-registry -p aurora-core -p aurora-cli -p aurora-gateway
  --all-targets --all-features -- -D warnings`: clean. Not the whole workspace.
- `cargo fmt --all --check` clean; `bash scripts/guards/run-all.sh` exit 0 (it first failed on two things I
  fixed: an integration name in a linter message, and the limits rows); `python3 scripts/extract-schema.py
  --check` up to date.
- `cargo deny check`: **advisories FAILED, bans/licenses/sources ok**: `wasmtime 50.0.1` (RUSTSEC / GHSA-wvgq-g5hf-6gm6,
  fix ≥ 50.0.2), reached through `aurora-hookhost` → `aurora-cli`. It is not from this change (my `Cargo.lock` diff
  adds only `aurora-webhook`); it is the CI Watcher's `cargo update -p wasmtime`.
- The tests that prove the behaviour are new tests (no earlier code path); the injection tests, the
  per-code failing configs and the vectors assert specific outputs, so each fails on a wrong implementation.
