# P5-WEBHOOK-4: Outgoing webhook — dashboard: connect steps, presets, example picker, editor, preview, Send test, settings (owner, 2026-11-15, phase 5, D-320(f), C-909, C-912, C-913)

## Why
"When configuring the addon, the user will probably need to specify the remote URL to hit and the shape
of the contents, so agents know what's expected on the other end" (owner, 2026-11-15). The owner wants
both the presets and the example picker in the first version (D-320(f)), so that a non-developer can
describe a receiver's payload in a few clicks.

Read first: `API_CONTRACTS` **C-913**; C-909 (the `WebhookConfig` model, the routes, the
`webhook_config_invalid.fields[].path` codes) and C-912 (`effective_rate`); `P5-WEBHOOK.md` §1, §2 and
§7; `14 §Visual design` and the connect dialog's current code (FIX-149, FIX-153 and the `fields` form).

## Scope
All files are under `web/`. The API is P5-WEBHOOK-2's: build against the regenerated
`schema.d.ts`, and mock the routes in unit tests until -2 is merged.

### 1. Connect dialog for app `webhook` (C-913), three steps
1. **Name and URL.** The URL is a secret field, never echoed back.
2. **What the receiver expects:**
   - **Preset picker:** from `GET /v1/apps/webhook/webhook-presets`. It pre-selects a preset whose
     `suggest_for_hosts` contains the URL's host, as a suggestion only. Choosing one fills the
     template, the slots, the content type and the required secret value names.
   - **Start from an example:**
     - paste JSON and show it as a tree;
     - click a scalar, a string array or an object to make it a slot, with the name taken from the key
       (made valid and unique), the type inferred, and editable description, required and limits;
     - click again to make it fixed;
     - it produces the template and slots, JSON only.
   - **Raw editor:** the template (shared code editor) and a slot table (add, remove, reorder, type,
     limits, enum values, a schema for `json`), for form, text, or anything the picker cannot express.
   - **Preview:** `POST …/webhook-preview` with sample values (each slot's `example`, editable). It
     shows the rendered body with `«secret:name»` masks, or maps
     `webhook_config_invalid.fields[].path` and `invalid_slot` onto the right control.
3. **Extras** (a folded section):
   - the description for the agent (≤ 1,000, with a counter);
   - secret values (≤ 4; name + password-style input);
   - headers (≤ 4; a value may use `{{secret:name}}`);
   - signing: off, generate or paste. When generated, the secret is shown **once** after connect, with
     copy and a "this will not be shown again" notice;
   - method;
   - limits per minute and per day, with the effective ceiling and its source shown, and validation
     against it.

### 2. Connection card and Edit
- **The card:** name, host, Read only, the effective limit and its source, **Edit** and **Send test**.
  It never shows the URL.
- **Edit:** the same steps 2–3 with `GET`/`PUT …/webhook`.
  - Secret values show only their names, with replace and delete.
  - Signing has keep, regenerate (shown once), paste and off.
  - Changing the URL is the existing Reconnect.
- **Send test:** a values form generated from the slots, with Dry run and Send.
  - It shows the status, the body (≤ 4 KiB, as text, never as HTML), the delivery id and
    `truncated`.
  - Send asks for confirmation ("This really calls the webhook").

### 3. Settings
- **Admin › Settings:** the `outgoing_webhooks` section through SET-5's generic section form (Helm-locked
  keys read-only, as for other sections). If SET-5 is not merged, report it and render nothing
  bespoke.
- **Org settings:** the switch `outgoing_webhooks.enabled` and the two limits, where the org settings
  page shows `calendar_invitations.enabled`.

### 4. Visual identity
- Shared primitives and page patterns only (`14 §Visual design`): tokens, both themes, no horizontal
  scroll at 390 px. The tree picker and slot table must work by touch at 390 px.
- Reuse the shared JSON/code editor. If there is none, add one to the shared set instead of a one-off.
- Keyboard accessible: the tree nodes are focusable and toggle with Enter or Space.
- Update `14` with the dialog, the editor and the card.

## Acceptance criteria
- [ ] **Unit tests (vitest):**
  - example picker: a Slack-style example → the template with `{{text}}` and a `string` slot; nested
    objects; arrays; key names made valid and unique; un-making a slot restores the constant;
  - preset suggestion by host;
  - error-path mapping for each C-909 code;
  - the signing secret is shown once and gone after closing;
  - the URL and secret values never appear in the DOM after save (canary);
  - Send test confirmation.
- [ ] **Playwright** against the dev stack (needs P5-WEBHOOK-2):
  - connect with a preset;
  - connect from an example;
  - edit a slot;
  - preview;
  - dry run and a real Send test to the recorded receiver;
  - the effective limit shown;
  - both themes at 390 px with no horizontal scroll;
  - axe has no serious violations.
- [ ] `pnpm -C web lint`, `typecheck`, `test`, `build`.

## Ownership and sequencing
- **Owns:** `web/**` files for the connect dialog's webhook steps, the connection card's webhook
  actions, the editor components and their tests, any new shared primitive, and `14`.
- It may start once this spec is merged (contracts fixed). Playwright needs P5-WEBHOOK-2 merged. Any
  free lane with web capacity: it shares no files with -1..-3.

## Numbers
D-320, C-913 (with C-909, C-912). No migration.

## Suggested agent/model and reason
`nextjs-developer` (Sonnet). An interactive editor with a tree picker, live preview and careful secret
handling across several dialog steps.
