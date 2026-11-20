# P5-REFINE-19: Expert speed: command palette, shortcuts, bulk actions, export and import, copy as curl, CLI snippets (owner, 2026-11-20)

## Goal

Aurora has no command palette (no `⌘K` handler anywhere in `web/src`), no bulk actions on People or
Accounts, an export only for administration settings (YAML/JSON/Import) and conversations, the CLI is
mentioned on the SSH page only, and API keys say nothing about what they can do. Meanwhile expert controls
leak into everyone's screens (cron, `{{secret:name}}`, `X-Signature`). Rec 14 adds speed for the people who
want it **without** adding anything for the people who don't.

The rule (D-406, brief 3): **everything here is behind "Show more controls"**, except the nouns in the
palette. Nothing in this brief changes what a person who never turns the switch on sees.

Decisions that bind this brief: **D-405**, **D-406** (altitudes, the per-person switch), **D-413** (the
export format is the format of the templates, which live in the web bundle).

## Linked requirements

- **The review:** <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt>. Workers cannot open artifacts; read the
  local copy `/home/alex/orca/workspaces/aurora/design-ux-review-5/` (`UXR5/` below).
  - `UXR5/layer-above.md` **rec 14** ("Expert speed that never leaks"), rec 1 (altitudes), rec 5 (the
    purpose templates, whose data format the export reuses), rec 12 (Automations). Anchor
    <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt#above>.
  - There are **no per-finding mockups** for this brief; rec 14 is text. The implementer follows the
    descriptions below and the existing kit (`components/ui/*`, the scope switcher's look for the palette).
    Reference screens (Before): `UXR5/screens/C112-ssh.png` (the only CLI snippet today),
    `UXR5/screens/B172-settings-apikeys.png` (API keys with no explanation),
    `UXR5/screens/C025-webhook-deliveries.png`, `UXR5/screens/B163-people.png` (workspace People), `UXR5/screens/A114-org-people.png`
    (organization People), `UXR5/screens/A136-org-apikeys.png` and `A172-account-apikeys.png` (API keys).
  - The mockups of the webhook "How to call it" panel are `UXR5/after/webhooks-1.png` / `-lt.png` and
    `webhooks-3.png` (brief 14); this brief adds **Copy as curl** to the cards that brief builds.
- **Decisions:** D-405, D-406, D-413, D-389 (the switcher's search indexes workspaces and organizations).
- **Contracts:** none new. Everything uses existing routes (list below). If a route is missing for bulk
  actions, the implementer sends the calls one by one with a progress line and reports the gap.
- **Security:** the palette and the shortcuts never bypass a check: each verb performs the same API call as
  its button and shows the same dialog (the confirm ladder of brief 2). Export never includes a secret.

## Wave and order

- Wave 3, **lane A**, after brief 15 (lane A: 1 → 11 → 15 → 19). Needs on master: brief 3 (`<Altitude>`,
  `useMoreControls`), brief 2 (confirm ladder, `SecretOnce`, `ResponsiveTable`), brief 12 (templates data
  shape, Settings pages), brief 14 (Automations list, webhook cards, "How to call it"), brief 16 (People,
  API keys explained), brief 15 (Computer pages).
- Disjoint files: owns `components/palette/**`, `components/shortcuts/**`, `lib/shortcuts.ts`,
  `lib/export/**`, and small additive edits in the lists named in Scope.

## Context

Paths under `web/src/` unless noted. Line numbers: master `c0fe7d10` (some files are rewritten by earlier
briefs; re-read them before editing).

- **Switcher and its search:** `components/nav/scope-switcher.tsx` :86 `ScopeSwitcher`, :133–140 recents,
  :176 `onKeyDown`, :368 the panel; indexes workspaces and organizations (D-389).
- **Existing key handling:** `components/agents/chat-tab.tsx` :199–208 `panelKey` (`/` and `⌘F` focus the
  search); `components/chat/composer.tsx` :328; `components/agents/status-rail/status-rail.tsx` :158
  (Escape); `components/agents/terminal-tab.tsx` :542, `files/viewer/file-viewer.tsx` :244,
  `ui/code-field.tsx` :59 (they capture keys: shortcuts must not fire inside them); `onboarding/tour.tsx`
  :166.
- **Approvals:** `components/chat/approval-card.tsx` (buttons "Allow once" :147, "Block" :150),
  `components/approvals/approvals-view.tsx`.
- **Lists for bulk:** People `components/org/members-view.tsx` and `components/settings/access-tab.tsx`;
  Accounts `components/accounts/accounts-list.tsx`; Automations (brief 14's list in
  `components/automations/`).
- **Export that exists:** `components/admin/settings/settings-index-view.tsx` :92 (`exportUrl("yaml")`),
  `import-dialog.tsx`; conversation export (JSON/zip) in `components/agents/conversation-actions.tsx`.
- **Agent data routes (client):** `lib/api/agents.ts` (agent, brief), `lib/api/skills.ts` (`useAgentSkills`
  :99, `useCreateSkill` :132, `usePutSkillFile` :171), `lib/api/memory.ts` (`useMemories` :27,
  `useCreateMemory` :60), `lib/api/schedules.ts` (`useSchedules` :21, `useCreateSchedule` :45),
  `lib/api/webhooks.ts`, `lib/api/channels.ts`.
- **Templates:** `web/src/lib/onboarding/templates.ts` (created by brief 12, D-413): the data shape of a
  purpose template. **Export writes that shape; Import reads it.** If the shape differs from the sketch in
  Scope 4, adapt this brief to the shape on master, not the other way round.
- **CLI today:** `components/agents/ssh-page.tsx` :87 (`aurora ssh <id>`), `components/agents/connect-tab.tsx`
  :130 (`aurora run`), `docs/14` §Computer (Connect, SSH).
- **API keys:** `components/settings/keys-tab.tsx`, `components/org/keys-view.tsx`,
  `components/account/my-keys-view.tsx`, `account/key-table.tsx`; OpenAPI at `GET /v1/openapi.json`.
- **Webhook cards and `{{secret:name}}`:** `components/webhooks/webhooks-tab.tsx` (rebuilt by brief 14).
- **Primitives:** `components/ui/dialog.tsx`, `menu.tsx`, `popover.tsx`, `secret-once.tsx`,
  `responsive-table.tsx`, `altitude.tsx`, `lib/api/use-more-controls.ts`.

## Scope

1. **Command palette (⌘/Ctrl K).**
   - Registry: `lib/palette-registry.ts` (nouns and verbs). Component `components/palette/command-palette.tsx`
     mounted once in `app-shell.tsx`.
   - Opens with `⌘K` (macOS) or `Ctrl K` elsewhere, and from a button in the rail's footer ("Search · ⌘K",
     the shortcut shown in muted text). Closes with Escape or a choice. It is a dialog (`ui/dialog.tsx`,
     top-anchored by brief 2) with a search field, a result list (`role="listbox"`, arrow keys, Enter),
     group headings, and a footer hint line. Opening focuses the field. Looks like the switcher's panel.
   - **Nouns (altitude Use, everyone):** workspaces and organizations (same index as the switcher,
     D-389), agents of the current workspace, the topics the viewer's `visibleTopics` lists (Overview,
     Agents, Approvals, …), the agent's pages the viewer may see, and the person's own account pages.
     Choosing one navigates.
   - **Verbs (altitude Expert; only when "Show more controls" is on, via `<Altitude id="palette.verbs">`
     logic):** "Give <Agent> an app", "New schedule for <Agent>", "New webhook for <Agent>", "New chat with
     <Agent>", "Pause <Agent>" / "Resume <Agent>" (opens the same dialog as the button), "Open Approvals",
     "Add people", "New agent", "Export <Agent>". A verb that needs a role the viewer lacks is not listed.
     A verb opens the same dialog/page the button opens; it never acts without that confirmation.
   - Matching: case-insensitive substring and word-start match, recent items first (reuse `readRecent`).
     No new dependency (no `cmdk`).
   - No result: "Nothing matches “<text>”." No error when the session is still loading: show a skeleton row.
   - The palette is not opened while focus is inside the terminal, an editor (`code-field`, the file
     viewer) or a native `<select>`: guard with the same "typing" test as `chat-tab.tsx` :201 plus
     `closest("[data-no-shortcuts]")`; add `data-no-shortcuts` to the terminal and editor containers.
     Exception: `⌘K`/`Ctrl K` itself works inside a plain text field (it is not a printable character), but
     not inside the terminal (it is a terminal key) or an editor.
2. **Keyboard shortcuts (Expert, behind the switch).**
   - `lib/shortcuts.ts`: one registry and one `useShortcuts()` hook with a single `keydown` listener on
     `document`, chords with a 1 s window (`g` then `a`).
   - Shortcuts (listed in the sheet):
     - `g a` → agents of the current workspace; `g p` → Approvals; `g o` → Overview; `g s` → Settings.
     - `n` → New chat with the current agent (on an agent page; uses brief 7/11's "New chat").
     - `a` / `b` on a **focused** approval card → **Allow once** / **Block** (the card must have focus or
       be the selected one in the list; same confirmation behaviour as the buttons; for an approval whose
       buttons are not "Allow once"/"Block" the keys do nothing).
     - `/` focuses the conversation search (already exists; listed), `⌘K`/`Ctrl K` the palette (always on,
       it is a noun feature).
     - `?` opens the **shortcut sheet** (a dialog listing every shortcut by group; also reachable from the
       palette verb "Keyboard shortcuts" and the account menu entry **Keyboard shortcuts**, shown only with
       the switch on).
   - **Never** fire when: focus is in an `input`, `textarea`, `select`, a `contenteditable`, the terminal,
     the file editor or any `[data-no-shortcuts]`; when a modifier other than the chord's is down; when a
     dialog is open (except the sheet and the palette themselves); on a touch-only device (no `hover:
     hover` media match, to avoid on-screen keyboards).
   - With the switch **off**, only `⌘K`/`Ctrl K` (nouns) and the existing `/` work.
3. **Bulk actions (Expert).**
   - **People** (organization and workspace): row checkboxes (a select-all in the header; `ResponsiveTable`
     handles the stacked layout), a bar "<n> selected" with **Change role…** and **Remove…**. Both open the
     brief 2 confirm dialog with the consequence sentence and the count ("Remove 3 people from Acme?
     They lose access now."; promoting to Owner in bulk says what Owners can do). Calls are the existing
     per-person routes, sent sequentially with a progress line ("2 of 3"); failures are listed by name at
     the end with a Retry for the failed ones; successes are not rolled back. A person cannot select
     themselves for Remove, or the last owner (the server also refuses).
   - **Accounts:** **Give to <agent>…** (a picker) and **Turn off**, same pattern.
   - **Automations:** **Pause** / **Resume** (undoable rung: toast with Undo), **Delete…** (permanent rung).
   - The checkboxes and the bar exist only with the switch on (`<Altitude id="bulk">`); with it off the
     lists look as before.
4. **Export and import an agent (Expert).**
   - Menu entries on the agent's Settings page (brief 11/12): **Export <Agent>…** and **Import into
     <Agent>…** (also palette verbs).
   - **Export** builds, in the browser, a `.zip` named `<handle>-<date>.zip` containing a folder
     `<handle>/` with:
     - `agent.yaml`: `{ format: 1, name, purpose?, model?: "default" | <model name>, may_message_first }`
       (the agent's own settings that a template carries; **no ids, no secrets, no account links**);
     - `instructions.md`: the instructions text;
     - `memory/<key>.md`: one file per memory, front-matter `key:` and `updated:` then the text;
     - `skills/<skill-name>/SKILL.md` (+ the skill's other files, as stored) with front-matter `name:` and
       `use-it-when:`;
     - `automations/<name>.yaml`: schedules `{ kind: schedule, name, prompt, cadence: { preset | cron },
       time_zone, enabled }`, webhooks `{ kind: webhook, name, verification: signed | password | none,
       enabled }` **without URL or secret**, channels are **not** exported;
     - `README.md`: one paragraph "Exported from Aurora on <date>. Secrets, accounts and conversations are
       not included."
   - The file shape equals the purpose templates' data (`templates.ts`): a template **is** an export with
     `purpose` set. A test builds an export from a template and asserts it equals the template data.
   - A small store-only zip writer in `lib/export/zip.ts` and a restricted YAML writer/reader in
     `lib/export/yaml.ts` (strings, numbers, booleans, null, arrays, maps; quoting and escapes tested;
     reading rejects anchors, tags and anything else). **No new dependency** unless the implementer shows
     that a maintained one is already in `pnpm-lock.yaml` (then use it and say so).
   - **Import** opens a dialog: pick a `.zip` (or drop it), the dialog parses it and shows a checklist
     "Instructions · 3 memories · 2 skills · 1 schedule · 1 webhook" with a checkbox per group and a
     **Replace instructions** vs **Keep mine** choice for the instructions (default Keep). **Import**
     creates through the existing routes (memories, skills, schedules, webhooks); items whose name already
     exists are skipped and listed ("Skipped: skill “release-notes” already exists"). Imported webhooks are
     created **paused** with a fresh signing secret shown once with `SecretOnce`; imported schedules are
     created **paused** with a note "Check the schedule, then turn it on". Import never changes access,
     accounts or the AI key. The zip is read in the browser; nothing is uploaded as a file.
   - Limits: 5 MB per zip, 200 files, names validated (no `..`, no absolute paths, no hidden files);
     anything else is refused with one sentence naming the problem.
5. **Copy as curl (Expert).**
   - **Webhook cards** (brief 14): **Copy as curl** next to "How to call it", producing the same command the
     panel shows for the webhook's verification (signed: with the `X-Signature: sha256=<hex>` recipe and a
     placeholder `$SECRET`; password: the header; none: plain), with the real URL and placeholders for
     secrets (never the secret value).
   - **API keys** (`keys-tab.tsx`, `keys-view.tsx`, `my-keys-view.tsx`): each key row's menu gets **Copy as
     curl** (`curl -H "Authorization: Bearer $AURORA_API_KEY" <base>/v1/me`, the base URL from the page's
     origin; never the key). The page says, under its heading, **"Acts as you in this workspace through the
     Aurora API."** (organization keys: "Acts for this organization …", with the scope the key has) and
     links **OpenAPI description** to `/v1/openapi.json`.
6. **CLI snippets in a "From your terminal" fold (Expert).**
   - A reusable `components/ui/from-your-terminal.tsx` (a `<Altitude id="fold.cli" level="expert">`
     `<details>`) showing `CopyBlock`s with one-line explanations. Add it to: **Computer** (`aurora ssh
     <handle>`, `aurora run`), **Automations** (the webhook curl and, when the CLI supports it, an
     `aurora` command to trigger or list runs), **API keys** (`aurora login`, `curl` call). Use only
     commands that exist in the CLI today; read `docs/14` §Computer and the CLI's `--help` in the repo
     (`crates/aurora-cli`) and list in the completion report each command and where it was confirmed.
     No command is invented.
7. **Docs:** `docs/14` §Expert speed (palette, shortcut sheet and the "never fire" rule, bulk, export format
   with a tree and an example `agent.yaml`, copy as curl, CLI fold), and the shortcut list.

## Out of scope

- Any API change; server-side export; export of conversations/secrets/accounts/channels.
- A new template editor, organization-editable templates (D-413).
- Palette over message content (the conversation search stays where it is).
- Shortcuts inside the terminal and editors.

## Acceptance criteria

1. With the switch **off**: `⌘K`/`Ctrl K` opens the palette with nouns only (no verbs); no checkboxes in
   People/Accounts/Automations; no `Export`/`Import` entries; no "Copy as curl"; no "From your terminal"
   fold; `g a` does nothing; the account menu has no "Keyboard shortcuts".
2. With the switch **on**: the palette lists verbs the viewer's role allows; choosing "Pause Daneel" opens the
   Pause dialog (it does not pause by itself); `g a`, `g p`, `n`, `a`, `b` and `?` work as listed.
3. Typing `a` in the composer, a text field, the terminal or the file editor never triggers a shortcut;
   `⌘K` does not open the palette inside the terminal.
4. Bulk: changing 3 people's role asks once, sends 3 calls, shows "3 of 3", lists a forced failure by name
   with Retry; cannot select yourself for Remove.
5. Export then Import round trip: export an agent with instructions, 2 memories, 1 skill, 1 schedule, 1
   webhook; import into another agent; the memories and skill exist, the schedule and webhook exist and are
   **paused**, the webhook shows a new secret once; no URL, secret, id or account appears in the zip (test
   greps the zip text).
6. A template and an export of the same content are equal (test).
7. "Copy as curl" copies a command with placeholders, never a secret or a key value (test).
8. Every CLI command in a "From your terminal" fold appears in the CLI help (list in the report).
9. Palette and sheet pass axe, are keyboard-complete, and fit at 390 px (the palette is full width, top
   anchored); both themes; no horizontal scroll; no console errors.
10. No new runtime dependency in `web/package.json` (or the report names it and why).

## Tests

Targeted only; heavy commands through `/home/alex/orca/workspaces/aurora/heavy.sh <lane> <command>`; GitHub
CI is the full gate.

- Vitest: `shortcuts` (chords, guards for typing/terminal/dialog/touch), `palette` (nouns/verbs per altitude
  and role, matching, navigation), `zip` and `yaml` (round trip, malicious names/YAML refused), `agent-export`
  (tree, no secrets, equals template), `agent-import` (skips, paused, secret once), `bulk` (progress,
  failures, self-protect), `copy-as-curl`.
- Playwright (mocked): palette open/search/choose; shortcut sheet; export download then import; bulk role
  change; at 1256 and 390 px, both themes, axe.

## Tests move with the UI (owner, 2026-11-20 — binding)

The owner: "Make sure the briefs not only change the website, but also the tests that test it! I don't want a ton of
broken tests because a button was renamed and the tests look for the old label." (P5-REFINE-1 merged with old labels
in tests, and master's CI broke until the CI Watcher fixed it in `ee7bfa0d`.) So, in **every commit** of this brief:

1. **Find every test that sees what you change.** Before committing a change to visible text, a label, an
   `aria-label`, a role, a heading, a route or `?tab=` id, a page's structure, an API field, an error code or message,
   search all test trees for the **old** strings, selectors and paths, and for the components you touched:
   - `web/tests/**` (vitest + Testing Library);
   - `web/e2e/**` (Playwright: the mocked default project **and** the `production` project that `scripts/e2e/*.sh`
     run against a real stack — `first-run.sh`, `kind.sh`, `files.sh`, `ssh.sh`, `channels.sh`, `approvals-park.sh`…);
   - `scripts/e2e/*.sh` themselves (they grep pages and call routes);
   - Rust tests that assert a message, an error code or a response shape (`services/*/tests/**`, `crates/*/src/**`
     `#[cfg(test)]`, `services/aurora-api/tests/it/**`);
   - fixtures and mocks (`web/e2e/fixtures/**`, `web/tests/**/mocks*`, MSW handlers, `web/openapi.json` snapshots).
   A plain `rg -n "<old text>|<old selector>" web/tests web/e2e scripts/e2e services crates` per change is the minimum.
2. **Update them in the same commit as the change.** New expectations use the new words exactly as the mockup shows
   them. Prefer `getByRole(…, { name })` or a `data-testid` over raw text where the text is not the point of the test;
   where the words are the point (they are the fix), assert the new words. Never loosen an assertion so that both the
   old and the new label pass, never `.skip`/`test.fixme`/delete a test to get green; a test is removed only when the
   feature it covers is removed, and the completion report names it and why.
3. **Run what you touched, plus everything that references it.** Through `heavy.sh`: every vitest file you changed
   **and** every vitest file that renders a component you changed; every Playwright spec (mocked project) that visits a
   page you changed. **Exception to the targeted-tests rule for this set (owner, 2026-11-20):** a commit that renames
   or restructures shared UI (a primitive, the app shell, the rail, the agent page, a label used on several pages) runs
   the **whole** web vitest suite once (`cd web && /home/alex/orca/workspaces/aurora/heavy.sh <lane> pnpm exec vitest run` (not `pnpm --dir web …`, which fails here)) before handing over. Production
   specs (`--project production`) can't run in a lane without a real stack: update them by reading, and list them in
   the completion report so the nightly / P5-REFINE-20 runs them.
4. **New behaviour gets new tests** (the brief's Tests section); old tests are **kept and updated**, not replaced.
5. **The completion report lists** every test file changed (and why), every test removed (and the removed feature),
   the commands run with their results, and the production specs that only the nightly will run.
6. A merge that turns master's CI red because a test still expects old UI is a send-back to this lane, not the CI
   Watcher's job.

## Docs to update

- `docs/14-web-dashboard.md` §Expert speed; `docs/FEATURES.md`, `docs/PROGRESS.md` rows.

## Suggested agent/model and reason

Sonnet (`nextjs-developer`). Several independent mid-size features; the key guards and import safety need
care. Split into two runs if useful: palette + shortcuts; bulk + export/import + snippets.

## Completion report

- **Items done** (Scope 1–7, each with the commit):
- **Deviations and why** (including the template shape actually used):
- **CLI commands used in folds** (command → where confirmed):
- **Screenshots compared / recorded** (palette, shortcut sheet, bulk bar, import dialog; dark/light, 1256/390):
- **Dependencies added** (none expected):
- **Tests run** (command and counts):
- **Open questions:**
