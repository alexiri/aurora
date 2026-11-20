# P5-REFINE-2: Shared primitives and patterns: save, secrets, confirm ladder, dates, tables, colour, anchored dialogs, 404 in the shell, labels (owner, 2026-11-20)

## Goal

UX review 5 found the same problem class on every area: each page invents its own way to save, show a
secret, confirm a danger, print a date, fit a table on a phone. This brief builds **one shared primitive
or pattern for each** and **adopts it on every screen that exists today**, so the area briefs (11–18)
only use them.

What is wrong now:

- **Five save patterns** (priority 2): a dark floating bar (Instructions), a white "1 change not applied
  yet" bar (Internet access, `ApplyBar`), instant saves with no word (avatar, access level, roles), a
  "Saved." line at the **top** of long Administration pages, a green "Settings saved." beside the button.
  On most Administration pages a successful and a failed save look the same.
- **One-time secrets** (theme): API keys, webhook URL/secret, signing secret and connector tokens each
  have their own dialog. Copy gives no "Copied" in some, the close X throws the secret away without a
  warning, the secret wraps mid-token.
- **Destructive and privileged actions** (theme): typed "delete", a typed slug nobody has seen, an
  inline red "Confirm delete", a plain dialog, nothing at all for promote-to-Owner or rotate.
- **Five date formats** (theme): "11/19/2026, 11:55:34 PM", "19 Nov 2026", "Nov 27", "23:50", "Just now"
  that never ticks, US `mm/dd/yyyy` date inputs.
- **Tables that don't fit** (theme, org-13, computer-5): nine columns on Activity, an inner horizontal
  scrollbar hiding the actions on phones.
- **Contrast and colour meaning** (theme, admin-11): dark logos on dark tiles, faint row actions, a green
  "Critical", amber for failures and red for information.
- **Pages and dialogs jump** (theme, setup-7, workspace-12): dialogs re-centre when a field appears,
  error banners push forms down, "Loading…" with no skeleton.
- **Context gets lost** (theme, org-3): an unknown organization renders a fake shell named "new"; account
  pages drop the workspace ("Choose a workspace"); the agent's SSH page sends you to Account with no way
  back.
- **Machinery in sentences** (priority 4, rec 13): no single place maps an enum to words, and nothing
  stops a new snake_case or UUID from reaching the screen.

Decisions that bind this brief: **D-405**, **D-407** (Pause/Resume words from brief 1), existing **D-384**
(amber only when the viewer can act), D-338.

## Linked requirements

- **The review:** <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt>. Local copy for workers (they cannot
  open artifacts): `/home/alex/orca/workspaces/aurora/design-ux-review-5/` (`UXR5/` below).
  - `UXR5/priorities-themes.md`: priority 2 (save), 3 (results), 4 (language), the ten themes. Anchors
    `#top`, `#themes`.
  - `UXR5/layer-above.md`: rec 13 (copy system with teeth), rec 15 (the ladder). Anchor
    <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt#above>.
- **Findings with a mockup** (anchor `…#f-<key>`; After `UXR5/after/<key>.png` and `UXR5/after/<key>-lt.png`;
  markup `UXR5/after/src/<key>.html`; Before `UXR5/before/<key>-b1.png`; screens `UXR5/screens/<id>.png`):
  - **admin-2** (save bar, in view): Before `-b1`, `-b2`; screens `A032-agents-defaults-invalid`,
    `A033-agents-defaults-saved`, `A068-signin-invalid`, `A069-signin-saved`.
  - **behaviour-6** (one save pattern; "Saved" toast; avatar and judge the same): Before `-b1`, `-b2`;
    screens `B054-instructions-saved`, `B056-avatar-upload`, `B085-judge-saved`.
  - **workspace-7** (save part only: "Saved" on Settings › General; the switcher showing the new name is
    brief 16): Before `-b1`, `-b2`; screens `B169-settings-general`, `B171-settings-saved`.
  - **webhooks-5** (the pattern: consequence dialogs for Rotate URL, Delete, new secret; the webhook
    screens themselves are brief 14): Before `-b1`, `-b2`; screens `C028-webhook-secret-rotated`,
    `C030-webhook-rotate-url`, `C032-webhook-delete-confirm`.
  - **workspace-8** (Delete workspace asks for the visible name): Before `-b1`; screen `B179-danger-zone`.
  - **setup-7** (add-key dialog keeps a fixed top; the display-name options are brief 18): Before `-b1`,
    `-b2`; screens `A014-add-key-dialog`, `A016-after-add-key`.
  - **workspace-12** (skeleton rows; the slow `/apps` load is brief 16/13): Before `-b1`; screens
    `B144-apps-accounts`, `B194-phone-accounts`, `B205-accounts-LIGHT`.
  - **org-3** (404 inside the shell): Before `-b1`; screens `A152-o-new`, `A142-org-app-page`.
  - **admin-11** (light plate behind logos; the alphabetical order and search are brief 18): Before `-b1`;
    screen `A040-admin-apps-expanded`.
  - **org-13** (stacked rows on a phone): Before `-b1`; screen `A203m-phone-org-people`.
  - **computer-5** (stacked file rows on a phone): Before `-b1`; screen `C165-m-files`.
- **Findings text:** `UXR5/findings.md` (search the key). The designer's note on workspace-12 is binding:
  a skeleton hides the wait but does not remove it.
- **Decisions:** D-405, D-407, D-384. **Spec:** `14` §Visual design (the "UX fixes" rules), §Settings in
  words.

## Wave and order

- Wave 1, **lane B**, starts immediately. Brief 1 (strings) merges first where both touch a component;
  rebase on it.
- Briefs 11–18 use these primitives. Brief 13 (lane B) follows this one.
- Disjointness: this brief owns `components/ui/**`, `lib/labels.ts`, `lib/format-date.ts`, the new guard
  tests, and the **adoption edits** listed in Scope. It does not redesign any page.

## Context

Paths under `web/src/`. Line numbers: master `c0fe7d10`.

- **Save today:** `components/ui/apply-bar.tsx` (C-989, "n changes not applied yet", Discard / Apply
  changes, sticky bottom, primary background); `components/agents/instructions-tab.tsx` :284–303 (own
  fixed bar "Unsaved changes", `saved` line); `components/ui/notice.tsx` :27 `SavedNotice`; the
  `SavedNotice` users: `account/profile-view.tsx` :91, `admin/settings/import-dialog.tsx` :103,
  `agents/workspace-byo.tsx` :102, `admin/settings/settings-section-form.tsx` :295,
  `apps/availability-view.tsx` :109, `internet/internet-level.tsx` :401, `org/directory-view.tsx` :107,
  `org/retention-form.tsx` :124, `org/settings-view.tsx` :132, `settings/general-tab.tsx` :67,
  `webhook/org-webhook-settings.tsx` :91; the judge form `agents/evals/judge-section.tsx`; the avatar
  upload in `instructions-tab.tsx`; instant selects such as `accounts/access-control.tsx`.
- **Secrets today:** `components/account/one-time-key-dialog.tsx` (Copy, "Copied", "I have saved it", the
  value in `<code class="break-all">`), used by `agents/reveal.tsx` :18, `connectors/connectors-admin-view.tsx`
  :84, `connectors/host-tokens.tsx` :125, `admin/runners-view.tsx` :189–204, `org/keys-view.tsx` :53,
  `settings/keys-tab.tsx` :55, `org/directory-view.tsx` :362; `webhook/signing-secret-once.tsx` (C-909,
  C-913); the webhook URL after create/rotate (`webhooks/webhooks-tab.tsx`); `ui/copy-value.tsx`,
  `lib/use-copy.ts`.
- **Confirms today:** `ui/confirm-dialog.tsx` (`ConfirmDialog`, `FormDialog`), `ui/confirm-button.tsx`
  (inline; used by `channels/channels-view.tsx` :181, `connectors/connector-card.tsx` :68,
  `skills/skills-view.tsx` :247, `memory/memory-tab.tsx` :196), `ui/type-to-confirm-delete.tsx` (slug;
  used by `org/settings-view.tsx` :221 and `settings/danger-tab.tsx` :27), `admin/admin-delete-dialog.tsx`
  :74 (`typed !== slug`), promotion by select in `org/members-view.tsx`, rotate/delete in
  `webhooks/webhooks-tab.tsx`, `schedules/schedules-tab.tsx`, `agents/reveal.tsx`.
- **Dates today:** `lib/format-date.ts` (`formatWhen`: "Just now / n min ago / n h ago / Yesterday / short
  date" and `formatExact`); `toLocale*` calls in `account/send-as-me.tsx`, `accounts/connection-details.tsx`,
  `admin/attention-section.tsx`, `agents/files-tab.tsx`, `admin/secret-stores-view.tsx`, `admin/ssh-line.tsx`,
  `admin/settings/*`, `chat/approval-card.tsx`, `skills/library.tsx`, `usage/usage-view.tsx`, `lib/api/ssh.ts`,
  `lib/api/agent-tokens.ts`, `admin/previews-line.tsx`, `agents/reveal.tsx`, `auth/cli-view.tsx`,
  `freeze/freeze-panel.tsx`, `memory/size-meter.tsx`, `internet/internet-level.tsx`, `memory/memory-tab.tsx`,
  `skills/skills-view.tsx`, `approvals/parked.tsx`, `webhook/receiver-step.tsx`, `lib/usage-format.ts`
  (30 hits; `grep -rn toLocale`). Date inputs: `admin/audit-view.tsx` :59 :69, `settings/keys-tab.tsx` :69,
  `org/keys-view.tsx` :67, `usage/usage-view.tsx` :359 :362 (`type="date"`). A ticking clock exists:
  `lib/api/approvals.ts` :254 `useNow(intervalMs)` (used by `overview-view.tsx` :95 at 30 s).
- **Tables today:** `components/ui/data-table.tsx` (`DataTable`, `Th`, `Td`: scrolls inside itself, 40
  users). To adopt `ResponsiveTable` first: Activity `approvals/activity-view.tsx`, Audit
  `audit/audit-panel.tsx` (and `admin/audit-view.tsx`, `org/audit-view.tsx`), webhook Deliveries
  `webhooks/webhooks-tab.tsx`, People `org/members-view.tsx` and `settings/access-tab.tsx`, invitations
  (in `members-view.tsx`), Users `account/admin-users-view.tsx`, Files `agents/files/file-list.tsx`.
- **Colour:** `components/ui/app-icon.tsx` (dark variant by `hasDarkIcon`), `ui/notice.tsx` (tones),
  `audit/audit-panel.tsx` :138 ("Critical" badge), `ui/menu.tsx` (row action trigger), `ui/badge.tsx`,
  `ui/status-line.tsx`.
- **Dialog:** `components/ui/dialog.tsx` :87–91 (`fixed left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2`,
  centred), `fixedHeight`.
- **404 and context:** no `app/not-found.tsx`; layouts `app/o/[org]/layout.tsx`, `app/account/layout.tsx`
  (`AppShell` wrap), `app/error.tsx`; `components/states.tsx` (`ErrorState`, `Loading`); org resolution
  in `components/app-shell.tsx` (`useResolve`) and `components/nav/scope-switcher.tsx` :86, :325
  ("Choose a workspace"); `components/agents/ssh-page.tsx` :88–100 (link to `accountRoutes.ssh`).
- **Labels:** `lib/settings-words.ts` (D-341 settings words), `lib/who-can.ts` (`ROLE_PHRASE`,
  `ACTION_PHRASE`), `lib/api/errors.ts`; `web/tests/ui-text.test.ts` (C-974 test, reads strings).

## Scope

1. **One save pattern (priority 2; admin-2, behaviour-6, workspace-7 save part).**
   - Artifacts `#f-admin-2`, `#f-behaviour-6`, `#f-workspace-7`; After `UXR5/after/admin-2.png`,
     `UXR5/after/behaviour-6.png`, `UXR5/after/workspace-7.png` (and `-lt.png`); markup
     `UXR5/after/src/<key>.html`; Before as listed.
   - New primitive `components/ui/save-bar.tsx`: `SaveBar({ dirty, errorCount, saving, onSave, onDiscard,
     savedMessage? })` plus a `useSaveBar()` state helper. States:
     - **Hidden** until the first change.
     - **Unsaved:** a bar fixed at the bottom of the viewport (inset 16 px, max width 36 rem, centred,
       `shadow-pop`): "Unsaved changes", **Discard** (outline), **Save**. The bar is the one that
       `instructions-tab.tsx` has today (this is the Behaviour pattern; it is the standard).
     - **Saving:** Save shows "Saving…" and is disabled.
     - **Saved:** the bar turns into a check and "Saved" (or `savedMessage`, e.g. "Saved. New agents use
       these defaults.") for **2 seconds**, then disappears. Admin-2's mockup also shows **Undo** next to
       the message on Administration defaults: offer `onUndo` as an optional prop, and use it there
       (it restores the values before the save and saves them again).
     - **Error:** the bar stays and reads "2 fields need fixing" (count of fields with an error); the
       page scrolls to the first bad field and focuses it. A failure that is not a field error shows the
       error sentence in the bar with the Save button still enabled.
     - `role="region"`, `aria-label="Unsaved changes"`; the "Saved" text is an `aria-live="polite"`
       status. No page-top "Saved." or green banner remains.
   - **Adopt** on every settings form that exists: `admin/settings/settings-section-form.tsx` (:295, long
     pages; this is admin-2), `account/profile-view.tsx`, `agents/workspace-byo.tsx`, `apps/availability-view.tsx`,
     `internet/internet-level.tsx` (its `ApplyBar` becomes `SaveBar`; "n changes not applied yet" becomes
     "n unsaved changes"; Apply changes becomes Save; `ApplyBar` and its tests are removed),
     `org/directory-view.tsx`, `org/retention-form.tsx`, `org/settings-view.tsx` (A135's good pattern is
     promoted, so replace the green "Settings saved."), `settings/general-tab.tsx` (workspace-7: Saved
     appears), `webhook/org-webhook-settings.tsx`, `agents/instructions-tab.tsx` (replace its own bar),
     `agents/evals/judge-section.tsx`. A120 (Internet access) and A135 (organization settings) are the
     existing good examples; keep their behaviour.
   - **Instant controls** (a switch, a select in a table row, the avatar, an access level, a model role)
     get a small inline **"Saved"** (check + word, 2 s, `aria-live`) next to the control: new primitive
     `InlineSaved` in the same file, driven by the mutation's success. Apply to `accounts/access-control.tsx`,
     the avatar in `instructions-tab.tsx`, `agents/model-roles-fields.tsx` selects, switches in
     `apps/availability-view.tsx` and People role selects.
   - Remove the "Saved."/`SavedNotice` lines these forms had. Keep `SavedNotice` only for the Import
     dialog result (it is not a settings save) or delete it if nothing is left.
2. **`SecretOnce` (theme "One-time secrets").**
   - Artifact `#themes`; no per-key mockup. The webhook-specific screens are brief 14's; the behaviour
     is defined here.
   - New `components/ui/secret-once.tsx` replacing `account/one-time-key-dialog.tsx` (keep the old file
     as a re-export for one release only if a caller outside this brief uses it; otherwise delete it).
     Props: `secret: string | null`, `title`, `description` (what it is for), `children` (extra notes),
     `onClose`, optional `copyLabel`.
   - The value sits in a **scrollable mono box that never wraps** (`overflow-x-auto whitespace-nowrap`,
     `select-all`), with **Copy** on its right. Copy turns into **Copied** (check, `aria-live`) for 2 s
     and stays "Copied" for the rest of the dialog once used.
   - Footer: **I have saved it** (primary). **Closing before copying** (the X, Escape, the overlay, or
     the footer button when Copy was never used) shows an inline confirm in the dialog: "You haven't
     copied it. Close anyway?" with **Copy** and **Close anyway**. After it was copied, closing is
     immediate.
   - The secret lives only in the props/state of the opener and is cleared in `onClose`; it is never
     written to storage, logs or the URL. Keep `data-testid="secret-key"` on the box.
   - **Adopt on every one-time secret:** `agents/reveal.tsx` (agent token), `connectors/connectors-admin-view.tsx`,
     `connectors/host-tokens.tsx`, `admin/runners-view.tsx` (new token), `org/keys-view.tsx` and
     `settings/keys-tab.tsx` (API keys), `org/directory-view.tsx` (`aud_` token),
     `webhook/signing-secret-once.tsx`, and the webhook URL shown after create/rotate in
     `webhooks/webhooks-tab.tsx` (the URL is a one-time secret too). Brief 14 re-uses it for its new
     dialogs.
3. **The confirm ladder (theme "Destructive and privileged"; rec 15; webhooks-5 and workspace-8 patterns).**
   - Artifacts `#f-webhooks-5`, `#f-workspace-8`, `#above` rec 15; After `UXR5/after/webhooks-5.png`,
     `UXR5/after/workspace-8.png` (and `-lt.png`); markup `UXR5/after/src/webhooks-5.html`,
     `workspace-8.html`; Before `UXR5/before/webhooks-5-b1.png`, `-b2.png`, `workspace-8-b1.png`.
   - Three rungs, each one primitive:
     - **Undoable** (pause, remove from a list, turn off): no confirm; a toast with **Undo**. New
       `useUndoToast()` helper in `components/ui/` (a 6 s toast region at the bottom, one at a time;
       reuse the existing toast host if one exists, else add a small one).
     - **Reversible but disruptive** (rotate, promote to Owner/Admin, pause an agent/workspace/
       organization, a role change that widens): `ConfirmDialog` with a **consequence sentence**
       (required `description`, enforced by a prop type and a test): e.g. "The old URL stops working now.
       Update every sender that uses it. The new URL is shown once." Button is the verb ("Rotate URL",
       "Make new secret", "Promote to Owner") in the danger tone when something stops working, primary
       otherwise.
     - **Permanent** (delete): `TypeToConfirm` replaces `TypeToConfirmDelete` and the slug field: a
       dialog with a red consequence line ("This deletes the workspace, its agents, files and
       conversations. It can't be undone."), then "Type the workspace name to confirm: **Bob's Lab**" with
       the **visible** name (never a slug), and a danger button "Delete workspace" disabled until it
       matches (case-sensitive, trimmed).
   - **Adopt:**
     - `settings/danger-tab.tsx` and `org/settings-view.tsx` (workspace and organization delete: visible
       name; workspace-8), `admin/admin-delete-dialog.tsx` (visible name);
     - retire `ui/confirm-button.tsx`: `channels/channels-view.tsx` :181, `connectors/connector-card.tsx`
       :68, `skills/skills-view.tsx` :247, `memory/memory-tab.tsx` :196 move to the right rung (delete of
       a skill or memory is permanent: dialog naming the item; remove a connector card: reversible
       dialog) and the file is deleted;
     - promotion to Owner/Admin in `org/members-view.tsx` (and the workspace People role select): a
       dialog "Make Carol an owner? Owners can change settings, add and remove people and delete the
       organization." instead of an instant select change;
     - rotate/delete in `webhooks/webhooks-tab.tsx`, `schedules/schedules-tab.tsx` (their dialogs' texts
       are brief 14's; here they only switch from `ConfirmButton` to the primitives, with the mockup
       sentences for webhooks as in webhooks-5: "Rotate the URL?", "Delete <name>?", "Make a new
       secret?"), and `agents/reveal.tsx` ("Regenerate token": "the old token stops working now").
   - Item names in a delete dialog are always the **visible name**, never an id or slug.
4. **`formatWhen` everywhere and ticking (theme "Five date formats").**
   - Artifact `#themes`. Change `lib/format-date.ts`: under a minute "Just now"; under an hour "n min
     ago"; under a day "n h ago"; **otherwise "19 Nov, 23:50"** (day, short month, time in the person's
     locale and time zone; with the year when not this year: "19 Nov 2025, 23:50"); "Yesterday" is
     dropped in favour of the dated form. `formatExact` (the `title` tooltip) keeps the locale date and
     time to the minute.
   - New `components/ui/when.tsx`: `<When value={iso} />` renders `<time dateTime title={formatExact}>`
     and re-renders **every 30 s** through one shared ticker (one `setInterval` for the whole page, not
     one per row; paused when the tab is hidden).
   - **Replace** every raw `toLocale*String` for a time shown to people (the 30 hits above) by `<When>`
     or `formatWhen`/`formatExact`. Durations ("awake 3 h") stay in `lib/usage-format.ts`.
   - **Date inputs** use the person's locale: replace `type="date"` by `components/ui/date-field.tsx`
     (a text input that shows the locale format via `Intl.DateTimeFormat`, with a native picker button;
     the value sent stays ISO `YYYY-MM-DD`). Adopt on the four screens above. The person's time zone is
     `GET /v1/me` `timezone` (C-1054) when set, else the browser's.
5. **`ResponsiveTable` (theme "Tables that don't fit"; org-13, computer-5).**
   - Artifacts `#f-org-13`, `#f-computer-5`; After `UXR5/after/org-13.png`, `UXR5/after/computer-5.png`
     (and `-lt.png`); markup `UXR5/after/src/org-13.html`, `computer-5.html`; Before
     `UXR5/before/org-13-b1.png`, `UXR5/before/computer-5-b1.png`; screens `A203m-phone-org-people`,
     `C165-m-files`.
   - New `components/ui/responsive-table.tsx`: `ResponsiveTable({ caption, columns, rows, rowKey,
     title, meta, actions, stackBelow = 900 })`. At or above 900 px it renders today's `DataTable`
     (desktop keeps its columns; drop the lowest-value columns into the row's expandable details when
     there are more than six). **Below 900 px** it renders **stacked rows**: one card per row with a
     title line (the row's `title` cell), a meta line (the other cells as "label value", wrapped), the
     actions at the **right edge** of the title line (a `…` menu or the row's buttons), nothing hidden
     behind an inner scrollbar. Use a CSS breakpoint (container query or `min-w` classes), not a JS width
     listener, so SSR matches.
   - The org-13 mockup shows cards: name + e-mail, role chip and status chip, "Joined 2 weeks ago", `…`
     menu; invitations as "dana@example.test / Member · expires in 6 days / Copy link, Cancel". The
     computer-5 mockup: file name, size and date on stacked lines, menu button at the right edge.
   - **Adopt** (desktop unchanged, stacked under 900 px): Activity, both Audit tables, webhook
     Deliveries, People, Pending invitations, Users, Files list. Other `DataTable` users may stay.
6. **Contrast and colour meaning (theme; admin-11 plate).**
   - Artifact `#f-admin-11`; After `UXR5/after/admin-11.png` / `-lt.png`; markup
     `UXR5/after/src/admin-11.html`; Before `UXR5/before/admin-11-b1.png`; screen
     `A040-admin-apps-expanded`.
   - `AppIcon`: in the dark theme, an icon without a dark variant (`hasDarkIcon` false) sits on a **light
     plate** (rounded square, `bg-white` at 92 %, 2 px padding). Icons that have a dark variant keep
     using it. The plate is for logos only, not for monogram tiles. The Administration apps list uses
     it (alphabetical order and search are brief 18).
   - Row action buttons (`…` menu triggers, ghost icon buttons in tables) use the regular icon colour
     (`text-ink-2`), not `text-muted`; hover `text-ink`. One change in `ui/menu.tsx`/`ui/button.tsx`
     variants.
   - **Critical** in the audit (`audit/audit-panel.tsx` :138) is a **danger-tone** badge, never green.
   - Rule (written in `14` §Visual design and enforced by a test of the `Notice`/`Badge`/`StatusLine`
     tone mapping): **red = failure, amber = something the viewer can do, green = good news only,
     neutral = information**. Review the amber notices that are information or failures on pages that
     exist today (the terminal-failure banner is red; "private destinations" information is neutral) and
     change their tone.
7. **Dialogs anchored, errors reserved, skeletons (theme "Pages and dialogs jump"; setup-7, workspace-12).**
   - Artifacts `#f-setup-7`, `#f-workspace-12`; After `UXR5/after/setup-7.png`, `workspace-12.png`
     (and `-lt.png`); markup `UXR5/after/src/setup-7.html`, `workspace-12.html`; Before
     `UXR5/before/setup-7-b1.png`, `-b2.png`, `workspace-12-b1.png`; screens `A014-add-key-dialog`,
     `A016-after-add-key`, `B144-apps-accounts`.
   - `ui/dialog.tsx`: content is **top-anchored at a fixed offset** (`top: 12vh` on ≥ 640 px, `top: 1rem`
     below; no `-translate-y-1/2`), so a field that appears does not move the title. `max-h` keeps the
     bottom 1rem free. Dialogs of `size="sheet"` on a phone keep today's full-screen behaviour.
   - **Reserved error space:** `Field` and `FormDialog` keep a one-line `min-h` slot for the error under
     a field and under the form actions so an error does not push the form down. A page-level
     `ErrorNotice` inside a dialog renders in that slot.
   - **Skeleton rows:** new `components/ui/skeleton.tsx` (`SkeletonRows({ rows = 4 })`, pulse
     animation disabled under `prefers-reduced-motion`). Replace the bare "Loading…" in the Apps &
     accounts lists (`accounts/accounts-list.tsx`, `apps/apps-tab.tsx`, `apps/org-apps-list.tsx`), and
     in `states.tsx` `Loading` (keep a `variant="rows|inline"`). The slow `/apps` request itself is not
     fixed here; note in the completion report what the network panel shows (time to first byte) so
     briefs 13/16 can act (designer's note on workspace-12).
8. **Context kept; 404 inside the shell (org-3, theme "Context gets lost").**
   - Artifact `#f-org-3`; After `UXR5/after/org-3.png` / `-lt.png`; markup `UXR5/after/src/org-3.html`;
     Before `UXR5/before/org-3-b1.png`; screens `A152-o-new`, `A142-org-app-page`.
   - New `components/not-found-state.tsx`: a dashed card with a search icon, a title, one sentence and two
     buttons. For an unknown organization: title "We couldn't find that organization"; text "There is no
     organization called “new”, or you don't have access to it. Check the address, or pick one of
     yours."; buttons **Go to <the viewer's last or only organization>** (primary) and **Create an
     organization** (outline; shown when sign-up allows, `GET /v1/me` capability as the switcher uses).
   - `/o/<unknown>` (and unknown workspace slugs) render this **inside the app shell** with the viewer's
     real organization in the rail and switcher; the shell does **not** take the unknown slug as the
     current organization (no fake "new" shell, no red "could not find" box).
   - A global `app/not-found.tsx` renders the same state inside `AppShell` for any unknown path: "That
     page doesn't exist" / "Check the address, or go back to where you were." / **Go home**. A refused
     page (403 without a `who_can`) uses the same shell and the sentence-first rule.
   - **Account keeps the last scope:** `scope-switcher.tsx` :325 shows the last visited workspace
     (the switcher already records the recent workspaces per person: `writeRecent`/`readRecent` in
     `scope-switcher.tsx` :133–140; take the newest entry that still exists in `GET /v1/me` memberships;
     no new storage key) instead of "Choose a workspace" on `/account/*`; the rail on account
     pages stays the account's own.
   - **Back to the agent:** the Terminal/SSH "Add your key" link (`ssh-page.tsx` :88–100) goes to
     `accountRoutes.ssh?from=<agent page path>`; the account SSH page shows a **← Back to <Agent name>**
     link when `from` is a same-origin path starting with `/o/` (validate: starts with `/`, not `//`,
     no scheme). Other "go and return" links (Account › keys from an agent) use the same `from`.
9. **`lib/labels.ts` and the guard (priority 4, rec 13).**
   - Artifact `#top` priority 4, `#above` rec 13; no mockup.
   - New `lib/labels.ts`: **one table per enum the API returns** that the UI shows, each
     `Record<Key, { label: string; sentence?: (ctx) => string }>`: gateway decisions (`default_denied`,
     `blocked_by_policy`, …), request outcomes, sandbox phases (Off, Starting, On, Stopping, Failed;
     the raw phase stays for Expert, brief 15), approval states, account health states, schedule and
     webhook outcomes (words from brief 1's table), audit actions (`api_key.revoke` → "Revoked an API
     key"), verification kinds, roles, harness display names (`claude_code` → "Claude Code"; `fake` is
     hidden outside dev builds). Start from the strings already in `lib/api/errors.ts`,
     `lib/settings-words.ts` and the components that map enums inline (`grep -rn "Record<.*string>"`);
     those move here and are imported.
   - `label(table, key)` returns the label; an unknown key renders **"?"** in development and **fails the
     unit test** that walks every enum value of `lib/api/schema.d.ts` it covers (list the covered enums
     in the test; a new enum value in the schema fails it until labelled). Never derive a label from a
     key (C-974 already forbids `humanizeKey`).
   - **Guard test** `web/tests/copy-guard.test.ts` (extends the C-974 approach in
     `ui-text.test.ts`): parse the strings of `components/` and `app/` and fail on a string that
     (a) matches `[a-z]+_[a-z]+` (snake_case), (b) matches a UUID, or (c) is a dotted key
     (`governance.egress.baseline_allowlist`, `api_key.revoke`) **unless** it is inside `<code>`,
     inside a component named `TechnicalDetails`, a `data-testid`, a route, an import or a code
     identifier position (the existing test already skips these). Allow-list file-by-file exceptions in
     a record that can only shrink (as `VOCABULARY_EXEMPT`); the list is the work queue for later
     briefs and must be **empty at the end of P5-REFINE-20**.
   - Adopt `label()` in the places priority 4 names: `default_denied`/`blocked_by_policy` in Activity and
     the request log, audit actions, `scn-pong (litellm)` model ids (show the display name), "Hooks ·
     connect.exchange", "Disabled (user)" (brief 1), "UNTRUSTED EXTERNAL DATA" (brief 11 does the chat).
     Wide adoption belongs to the area briefs; here adopt where the table is built and where a leak is
     named above.
10. **`TechnicalDetails` primitive (rec 13; members never see it, brief 3).**
    - New `components/ui/technical-details.tsx`: a collapsed `<details>` titled "Technical details",
      mono body, `admin` prop. With `admin`, it renders only for instance admins (`GET /v1/me`).
      Without it, it is for Set up altitude and above (the altitude check is brief 3's `Altitude`;
      until it merges, render for everyone and keep the TODO that brief 3 removes).
    - Replace the existing folds: `accounts/account-row.tsx` :283–284, `accounts/actions-table.tsx`
      :255, `apps/app-setup/sections.tsx` :320, `agents/computer-tab.tsx` :324, and the hand-written
      `<details>` for technical text in `admin/runners-view.tsx` :322, `org-apps-list.tsx` :116.

## Out of scope

- Redesigning any page; rewriting texts beyond what a primitive needs (brief 1 does words, area briefs do
  their screens).
- The altitude switch and hiding Technical details from members (brief 3).
- The webhook How-to-call panel, schedule presets, the Settings button, the timeline (briefs 11–14).
- Server changes. The `from` parameter and `lastScope` are web-only.

## Acceptance criteria

1. **Save:** on each adopted form the bar appears on the first change, Save shows "Saving…", success
   shows "Saved" for 2 s (and Undo on Administration defaults), failure shows "n fields need fixing" and
   scrolls to the first bad field; no page-top "Saved." remains (`rg -n "SavedNotice" web/src` is empty or
   only the Import dialog). Instant controls show an inline "Saved". Matches `UXR5/after/admin-2.png`,
   `behaviour-6.png`, `workspace-7.png` (dark) and `-lt.png` (light) at 1256 and 390 px.
2. **Secrets:** every one-time secret opens `SecretOnce`; Copy becomes Copied; closing before copying asks
   "You haven't copied it. Close anyway?"; a 120-character token does not wrap and scrolls inside its box
   at 390 px.
3. **Ladder:** `ConfirmButton` and `TypeToConfirmDelete` no longer exist; promote-to-Owner, rotate and
   delete each open a dialog with a consequence sentence; workspace delete asks for the **visible
   name**; no dialog asks for a slug. Matches `UXR5/after/webhooks-5.png` and `workspace-8.png` (both
   themes, both widths) for the dialog shape and words.
4. **Dates:** `rg -n "toLocale(Date|Time)?String" web/src/components` finds none outside
   `lib/format-date.ts`; a "Just now" label changes to "1 min ago" within 30 s without interaction (fake
   timers); date inputs show the browser locale's format.
5. **Tables:** at 390 px Activity, Audit, Deliveries, People, Pending invitations, Users and Files show
   stacked rows with their actions visible and no inner horizontal scroll; at 1256 px they look as today.
   Matches `UXR5/after/org-13.png` and `computer-5.png` (both themes).
6. **Colour:** Sentry, GitHub and Notion logos are legible on dark tiles in Administration › Apps (matches
   `UXR5/after/admin-11.png`/`-lt.png`); "Critical" is red; no amber notice without an action on the pages
   touched.
7. **Anchor:** opening the add-key dialog and revealing the key field does not move the dialog title
   (Playwright: title `boundingBox().y` equal before and after); matches `UXR5/after/setup-7.png`. Skeleton
   rows show while Apps & accounts loads (`workspace-12.png`).
8. **404:** `/o/new` shows "We couldn't find that organization" inside the shell with the real
   organization, no fake "new" shell; `/nope` shows the global not-found inside the shell. Matches
   `UXR5/after/org-3.png`/`-lt.png`. The account page's switcher shows the last workspace; the SSH link
   returns with "Back to <Agent>" and rejects `from=//evil.test`.
9. **Labels:** `copy-guard.test.ts` and the enum-coverage test pass; adding a made-up enum value to a
   covered type makes the coverage test fail (show once).
10. Axe passes and there is no horizontal scroll at 390 px on every touched page, in both themes.

## Tests

Targeted only; heavy commands through `/home/alex/orca/workspaces/aurora/heavy.sh <lane> <command>`;
GitHub CI is the full gate.

- Vitest, new: `save-bar`, `secret-once`, `confirm-ladder` (consequence required; visible name),
  `format-date` (all branches; ticker), `responsive-table`, `date-field`, `app-icon` (plate), `dialog`
  (anchor class), `not-found-state`, `scope-switcher` (last scope), `ssh-page` (`from` validation),
  `labels` (enum coverage), `copy-guard`. Update the tests of every adopted component.
- Playwright (mocked): one spec per theme-row at 1256 and 390 px in both themes with axe and no console
  errors: a save flow (Administration defaults), the secret dialog, a delete dialog, `/o/new`, a stacked
  table.

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
   the **whole** web vitest suite once (`pnpm --dir web vitest run`, through `heavy.sh`) before handing over. Production
   specs (`--project production`) can't run in a lane without a real stack: update them by reading, and list them in
   the completion report so the nightly / P5-REFINE-20 runs them.
4. **New behaviour gets new tests** (the brief's Tests section); old tests are **kept and updated**, not replaced.
5. **The completion report lists** every test file changed (and why), every test removed (and the removed feature),
   the commands run with their results, and the production specs that only the nightly will run.
6. A merge that turns master's CI red because a test still expects old UI is a send-back to this lane, not the CI
   Watcher's job.

## Docs to update

- `docs/14-web-dashboard.md` §Visual design: "Save", "One-time secrets", "Confirm ladder", "Dates",
  "Tables on small screens", "Colour meaning", "Dialogs and loading", "Not found and context", "Copy
  system" (labels, guard, Technical details). Amend §Settings in words for `labels.ts`.
- `docs/API_CONTRACTS.md` C-989 note: `ApplyBar` retired for `SaveBar` (web).
- `docs/PROGRESS.md` and `docs/FEATURES.md`.

## Suggested agent/model and reason

Sonnet (`nextjs-developer`), possibly two runs (primitives first, adoption second). Wide, mechanical
adoption with several subtle behaviours (focus, timers, SSR).

## Completion report

- **Items done** (Scope 1–10, each with the commit):
- **Adoption table** (primitive → every file now using it; anything left on the old pattern and why):
- **Deviations from the brief and why:**
- **Screenshots compared** (path, dark/light, 1256/390 px, verdict) for admin-2, behaviour-6, workspace-7,
  webhooks-5, workspace-8, setup-7, workspace-12, org-3, admin-11, org-13, computer-5:
- **`/apps` load time observed** (for workspace-12):
- **`copy-guard` exemption list at the end** (file → count):
- **Tests run** (command and counts):
- **Open questions:**
