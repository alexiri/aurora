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

## Completion report (lane p5-platform, 2026-11-18)

Web only. Commits `P5-WEBHOOK-4:` part 1 (connect steps), part 2 (card, Edit, Send test), part 3 (org switch, admin
section), then Playwright and `docs/14 §12`.
- **Types** are in one module, `web/src/lib/api/webhook.ts`, written from C-909/C-912/C-913 (P5-WEBHOOK-2 is not merged).
  When -2 regenerates `schema.d.ts`, replace its types by the generated ones and its `call()` by `api.GET/PUT/POST`.
  **Assumption to check against -2:** the org settings keys are `outgoing_webhooks_enabled`,
  `outgoing_webhooks_per_minute_max` and `outgoing_webhooks_per_day_max` on `PATCH /v1/orgs/{org}` (by analogy with
  `calendar_invitations_enabled`); and the `webhook_config_invalid` paths are JSON pointers (`/slots/2/name`; other
  spellings are normalised).
- **Shared:** new `ui/code-field.tsx` (the Files viewer's CodeMirror editor as a labelled control, Esc then Tab leaves it).
  `SignInStep` dispatches to the webhook steps for `app_id === "webhook"`; `AccountRow` shows the card line.
  Org switch: Apps tab of the organization. Admin: the `outgoing_webhooks` section under Apps & accounts › Advanced
  through SET-5's generic form (the route table already mapped it). Fix-167's files were not touched.
- **Not done / open:** "Read only" is not repeated on the card (the row already shows each agent's level). The ceiling is
  checked in the browser only when it is what applies (the view's `effective_rate.source` is not `connection`); the
  server's `rate_above_ceiling` covers the rest. The dev-stack specs are written and skipped (`webhook-stack.spec.ts`,
  `AURORA_E2E_WEBHOOK_STACK=1`): **not run — needs P5-WEBHOOK-2**. `e2e/support/mock-api.ts` got one additive mock
  (`/v1/admin/settings/outgoing_webhooks`).
- **Gate results:** `pnpm typecheck` ok; `pnpm lint` 0 errors (6 old warnings); vitest 2403/2404 in the full run — the one
  failure (`connect-journey` grant_failed, an 11 s timeout under load ~45) passes alone (22/22); the three webhook
  vitest files 63/63; `pnpm build` ok; guards all passed; Playwright mocked full run (1,302 tests, 3 workers): the
  webhook spec 16/16 in both themes at 1280 and 390 px with axe; first full run had 12 failures — 4 `app-setup` and the
  org webhook specs were my button/notice text ("Save changes", "Settings saved.") colliding with another spec's
  substring locators, fixed by renaming to "Update outgoing webhooks" / "Outgoing webhook settings updated." and rerun
  green; `ia-navigation` ×2 and `web-push` passed on rerun; `app-setup:78` and `welcome-summary:88` fail at random
  variants with 5 s timeouts under load (a different variant each run; not touched by this lane) and pass in other
  runs. Production project 11/11.
- **Canary:** vitest and Playwright check that after Save the address, the secret values and the signing secret are
  not in the DOM, and that a preview request carries secret names only.
- **Gated for today's server (2026-11-18, before P5-WEBHOOK-2 merges):** `OutgoingWebhooksOrgSettings` renders only when
  the organization's settings carry `outgoing_webhooks_enabled` (the server returns it, with the limits, once -2 lands);
  the admin `outgoing_webhooks` section is rendered by `OutgoingWebhooksAdminSection`
  (`components/webhook/admin-webhook-section.tsx`) only when `GET /v1/admin/settings` lists that section (an unknown
  slug would give `SettingsSectionView` a not-found error state, and while the index loads or fails nothing shows).
  The connect steps and the card line only appear for app `webhook`; no package of that id exists in the repository
  (P5-WEBHOOK-1's), so on today's server they never render. New `tests/webhook-gating.test.tsx` (5 tests: absent on
  today's shapes, present on the -2 shapes, the section is never requested while unlisted); the Playwright helper
  answers the -2 shapes. Re-run: typecheck ok; lint 0 errors; vitest webhook files 32/32; `pnpm build` ok; Playwright
  `webhook.spec` 16/16 (both themes, 1280 and 390 px); `stage4-axe` and `ia-navigation` passed in the 358-test run (354
  passed, the 4 failures were the webhook org/admin test before I fixed my mock; rerun green).

## Follow-up after P5-WEBHOOK-2 and P5-TOOLASK merged (lane p5-apps, 2026-11-19)
- `web/src/lib/api/webhook.ts` now takes its types from the generated `schema.d.ts` (`WebhookConfig`, `WebhookView`,
  `WebhookPreset`, `PreviewResponse`, `TestResponse`, `ConnectResponse`, `PutWebhookRequest`, `OrgSettingsPatch`). The wire
  leaves `slots`, `headers`, `description`, `signing`, `rate`, `preset` optional, so the module exports the editor's
  `WebhookConfig` with those defaults filled (`normalizeConfig`, applied to the view, presets and the Put cache) and splits
  the test answer into a sent result and a preview; the calls stay on `tracedFetch`. Components were untouched by the swap.
- Confirmed against -2: org keys `outgoing_webhooks_enabled` / `_per_minute_max` / `_per_day_max` on `PATCH /v1/orgs/{org}`
  (`OrgSettings` returns `outgoing_webhooks_enabled` always, so the org switch now renders; the `as never` patch cast is
  gone) and the admin section slug `outgoing_webhooks` (`aurora-config` settings registry), so both gates switch on.
- `webhook-stack.spec.ts` stays skipped (all four cases need the real stack and receiver; local dev-stack e2e is not run);
  `webhook.spec.ts` is the mocked coverage.
- P5-TOOLASK: the card line says an agent's send waits for approval unless allowed in the access settings; Send test's
  confirmation says a test goes out now while an agent's send waits.
- Gates: web typecheck ok; eslint on touched files 0; vitest webhook-logic/dialogs/gating 60/60; Playwright `webhook.spec`
  16/16 + 4 skipped (port 4181); guards ok.

## Suggested agent/model and reason
`nextjs-developer` (Sonnet). An interactive editor with a tree picker, live preview and careful secret
handling across several dialog steps.
