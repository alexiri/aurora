# P5-POLISH-2: Shared pieces: lock pill, settings rows, switches, Unavailable, empty states, status lines (owner, 2026-11-19)

## Goal

Build once the shared pieces that the page briefs (P5-POLISH-5..10) compose, and apply the ones that
live in shared components everywhere they appear. UX review 4 found the same problems on every level:

- amber "HELM" / "MANAGED BY HELM" / "SET BY YOUR ORGANIZATION" chips. These only say who owns a value,
  but amber is the colour for "needs you";
- settings as a 500 px column on a 1,440 px screen, with "More in … in the operator guide." under every
  field, and switches drawn as bare checkboxes;
- disabled forms shown under notices that say the feature cannot be turned on;
- empty states that are a small box beside "Select a … to edit it.";
- two or three tracked-out caps chips per card;
- dates with seconds in US format next to relative times.

## Linked requirements

- **Design reference (D-380).** Proposals **F1**, **F2**, **E6**, **E7** and **C6** of “Aurora UX
  fixes”, <https://claude.ai/artifact/WXbcpcrkbCLuxMSurpkuQW> (`#f1`, `#f2`, `#e6`, `#e7`, `#c6`).
  - You cannot open artifacts. Read `/home/alex/orca/workspaces/aurora/design-ux-fixes/README.txt`,
    then `after/f1.png`, `after/f2.png`, `after/e6.png`, `after/e7.png`, `after/c6.png` and their
    `before/` crops, and `after/src/<id>.html` for the exact tokens and spacing.
  - The review: <https://claude.ai/artifact/33oHaXQpCU3oSVRVmgtY5Z> (screens 020, 022, 028, 033, 106,
    211, 212, 221, 222, 240 under `review/img/`).
  - Yellow dashed labels are commentary. Never copy the mockup's HTML or class names.
- **D-384** (amber only where the viewer can act; lock pill; one chip per card, sentence case),
  **D-392** (settings rows, Unavailable), **D-383** (the health line's look; its data is P5-POLISH-3
  and its wording mapping P5-POLISH-6). `14 §Visual design` › "UX fixes".
- The redesign canvas's Read me › Patterns board (D-354; copy `/home/alex/orca/workspaces/aurora/design-canvas/`).

## Wave and order

- After **P5-POLISH-1** (layers and focus), in lane A: P5-POLISH-1 → **P5-POLISH-2** → P5-POLISH-6 → ….
- P5-POLISH-5, -6, -7, -9 and -10 use these pieces. They must not re-create them.

## Context

All paths are under `web/src/`.

- **Badge:** `components/ui/badge.tsx:22`, tones `neutral`, `accent`, `pending`, `danger`, tracked
  caps.
- **Helm chips are `Badge tone="pending"`** in:
  - `components/admin/settings/settings-field-controls.tsx:220`
  - `settings-section-form.tsx:737`
  - `llm-controls.tsx:244`
  - `sign-in-settings-view.tsx:80`
  - `operational-settings-views.tsx:461`

  "SET BY ADMINISTRATION" / "SET BY YOUR ORGANIZATION" come from the limits rows: grep
  `Set by` / `SET BY` in `components/internet/`, `components/governance/` and `components/agents/`.
  `LockIcon` exists in `components/ui/icons.tsx:44`.
- **Settings field:** `components/ui/setting-field.tsx`.
  - "More in … in the operator guide." at :118.
  - Source badge (Helm/Dashboard/Default) at :85; locked state at :54 and :94.
  - Raw checkboxes in `settings-section-form.tsx:607`, `governance/models-tab.tsx`,
    `governance/byo-panel.tsx` and `apps/org-apps-list.tsx`; `CheckboxField` in `components/ui/form.tsx:32`.
  - No generic Switch exists (only `ThemeSwitch` and `AnchorSwitch`).
- **Empty and who-can:** `components/states.tsx:80` (`EmptyState`, has an `ask` prop), `Loading` :40,
  `ErrorState` :52; `components/ui/who-can.tsx:17` (`WhoCan`, C-981).
- **Card:** `components/ui/card.tsx:5`.
- **Dates:** `fmtDate` in `components/account/key-table.tsx:14` (`toLocaleString`), `relativeTime` in
  `lib/api/files.ts:124`, and about 32 direct `toLocale*` calls.
- **Guards:** `web/tests/design-guards.test.ts`.

## Scope

1. **`LockPill`** (`components/ui/lock-pill.tsx`, D-384).
   - Neutral pill: `--subtle` background, `--ink-2` text, lock icon, sentence case. Examples: "Set by
     Users", "Set in Helm", "Set by Administration".
   - Props: `owner` (the label), `whoCan?: WhoCan` (a tooltip and an accessible description listing who
     can change it), `href?` (opens where it is changed, when the viewer may go there).
   - Replace every Helm `Badge tone="pending"` listed above, and every "SET BY …" chip, with it. Remove
     the "Ask alice (organization owners and admins)." lines that sit under limit rows: the pill's
     `whoCan` carries that information. Keep `WhoCan` lines where a page's main action is missing
     (C-961).
2. **`AttentionChip`** and the Badge rule (D-384, F2).
   - `Badge` renders in sentence case (no `uppercase`, no tracking) for every tone.
   - The `pending` tone moves to a new `AttentionChip`, which requires an `action` (a button or link
     rendered next to it, or an `actionId` that names one). `Badge` no longer accepts `tone="pending"`.
   - Convert every existing `pending` badge:
     - those with an action become `AttentionChip`;
     - those without one become neutral `Badge` or `LockPill`, or are removed when they say nothing
       new (for example "DEFAULT" next to a label).
   - **Design guard:** `design-guards.test.ts` fails on `tone="pending"` outside `AttentionChip`, and
     on `uppercase` in `badge.tsx`.
3. **`Switch`** (`components/ui/switch.tsx`): a labelled switch built on a native checkbox
   (`role="switch"`), 40×24, tokens only, keyboard and axe clean, with a `SwitchField` (label, help) for
   forms. Every boolean in settings uses it (see 4); raw checkboxes in the files listed above become
   `Switch`, except true multi-select lists, which stay checkboxes.
4. **`SettingRow` and settings cards (E7, D-392).**
   - Rebuild `setting-field.tsx` as a two-column row:
     - left, the label and its help;
     - right, the control and, when the value is managed, a `LockPill` ("Set in Helm") with the control
       read-only.
     - Rows stack below 640 px.
   - **Settings cards:** `SettingsCard` with a title, an optional one-line sentence and **one guide
     link** in its header (`Guide`), replacing the "More in … in the operator guide." line under every
     field.
   - Booleans use `SwitchField`.
   - **Full width:** settings pages use the page container (`--container-page`, 1080 px), not a
     500 px column. Fix the containers in the settings section form (`settings-section-form.tsx`), so
     every page that renders settings through it widens: admin, organization and workspace.
   - Tables inside settings cards (the runners table) span the card. No inner horizontal scrollbar at
     1280 px.
   - Keep each field's guide anchor: the card's link goes to the section, and a field-level anchor stays
     reachable through the help text's link where one existed.
5. **`Unavailable` card** (`components/ui/unavailable.tsx`, E6, D-392).
   - Neutral card: lock icon, a title ("The web terminal isn't available on this Aurora"), one sentence
     on what is missing, who can fix it (`WhoCan` or "An operator installs it"), and a guide link.
   - Replace the "can't be turned on here" notice plus disabled form pattern wherever it appears:
     - Agents › Computers (`operational-settings-views.tsx:349` `terminalNotice`, :203);
     - People › Sign-in › Directory sync (`sign-in-settings-view.tsx`, `section("scim")`);
     - any other place found by grepping for the notice components followed by disabled forms.
   - The settings render only when they can take effect.
6. **`EmptyState`** (C6).
   - Update `components/states.tsx`: icon tile, one title sentence, one sentence on what will appear,
     and **one** action or a who-can line with **Ask** (when the viewer cannot act). Dashed border card,
     as in `after/c6.png`.
   - Add a `MasterDetail` helper (or a rule in the existing list/detail components) that hides the
     detail pane, and its "Select a … to edit it." text, while the list is empty.
   - The agent pages adopt both in P5-POLISH-6. Here, apply them to the shared list/detail used by
     Memory and Skills only if that is a shared component; otherwise leave adoption to P5-POLISH-6.
7. **`StatusLine`** (`components/ui/status-line.tsx`, F2).
   - Dot plus a sentence-case line, with tones `ok`, `fail`, `idle` and `attention`, and an optional
     trailing action button ("Reconnect", "Sign in again").
   - Purely visual. P5-POLISH-6 maps account health to it.
8. **`LinkCard`** (`components/ui/link-card.tsx`): a card that is a link, with a title, one line, a
   trailing arrow, and hover and focus states. Used for Welcome's "Next" cards (P5-POLISH-7) and the
   setup checklist (P5-POLISH-10).
9. **Dates.**
   - One helper `formatWhen(date, { now? })` in `lib/format-date.ts`:
     - "Just now", "5 min ago", "2 h ago", "Yesterday", or "18 Nov" (the year when it differs), from
       the browser's locale;
     - plus `formatExact(date)` for a `title` tooltip (locale date and time, minutes, no seconds).
   - Replace `fmtDate` and `relativeTime` with it.
   - List the other direct `toLocale*` calls in the completion report. The page briefs replace them on
     the pages they rebuild.
10. **Patterns page.** If the app has a dev-only primitives or patterns page, add the new pieces there;
    otherwise add a Vitest render test per piece (both themes).

## Out of scope

- Page layouts and page content: P5-POLISH-6, -7, -9 and -10 use these pieces.
- Wording changes beyond the pieces' own fixed words. The wording pass is separate.
- The health data and its wording mapping (P5-POLISH-3, P5-POLISH-6).

## Constraints / file ownership

- **Owns:** `components/ui/{lock-pill,switch,unavailable,status-line,link-card}.tsx` (new),
  `components/ui/badge.tsx`, `components/ui/setting-field.tsx`, `components/states.tsx`,
  `components/ui/form.tsx` (`SwitchField`), `lib/format-date.ts` (new), the settings section form and
  controls under `components/admin/settings/` (layout and chips only), `tests/design-guards.test.ts`
  (new rules; P5-POLISH-1 added its own first).
- The call-site replacements listed in Scope 1, 2, 3, 5 and 9 are part of this brief. Page structure is
  not.
- Tokens only. No new colours: the lock pill uses `--subtle` and `--ink-2`, and attention uses the
  existing pending tokens.

## Acceptance criteria

1. No amber chip remains for ownership or Helm management. Every amber element has an action next to
   it, and the design guard enforces it.
2. Admin, organization and workspace settings pages use the full container: label left, control right,
   switches for booleans, one guide link per card, "Set in Helm" as a neutral lock pill with a
   read-only control. The runners table shows all columns at 1280 px without an inner scrollbar. Match
   `after/e7.png`.
3. Agents › Computers and Directory sync show one Unavailable card and no disabled form when the
   deployment lacks the feature (`after/e6.png`).
4. `EmptyState`, `StatusLine`, `LinkCard`, `Switch` and `LockPill` have Vitest render and axe tests in
   both themes.
5. Dates in the replaced helpers read "5 min ago" with the exact time in a tooltip. No seconds.
6. Web gates: typecheck, lint, vitest, build, Playwright mocked (axe and narrow-route, both themes,
   390 px). Report the counts.

## Suggested agent/model and reason

Sonnet (`nextjs-developer`). Several primitives plus a sweep through shared settings components; the
call sites are many, but each change is mechanical once the primitive exists.
