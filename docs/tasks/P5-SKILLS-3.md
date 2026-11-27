# P5-SKILLS-3: One list of everything an agent can do, a side panel that reads first, read-only skills that say so, and an editor with files folded (owner, 2026-11-27)

## Goal

Today an agent's Skills page shows the same skills twice under two spellings with a precedence essay between them
(S-1), every description reads wrong ("Use it when Draft a short email…", S-2), opening a skill puts a form in the
middle of a 2,700 px page and an inherited one at the very bottom (S-3), a workspace skill is labelled "On for the
organization's agents" (S-4, a bug), organization and library skills look editable (S-5; a library skill even has a
Save button), an inherited skill cannot be turned off for one agent (S-6), files hide behind an always-open upload form
and cannot be read (S-7), and nothing says whether a skill is used or lets you try it (S-12). Implement the review's
**skill-1, skill-2, skill-3 and skill-4**, and "Also" 3 (one set of words):

- **one list** with search, chips, a source chip and a **For <Agent>** switch per row (inherited skills included);
- a **right-hand panel** that shows the skill **rendered**, its files as a small tree you can open, its usage, and
  **Edit**, **Try it in chat**, "…";
- **read-only** skills with no fields at all, a lock line and the per-agent switch; library skills with their source,
  pin and update state;
- an **editor** in the panel: Name (folder shown small), "When should <Agent> use it?", Instructions with Write/Preview,
  **Files folded to one line** with **Add file** as a small dialog, the shared save bar;
- **Add skill** menu: **Write a new skill**, **Add from a library** (P5-SKILLS-4's dialog), **Upload a skill folder**.

The same list and panel serve the **workspace** and **organization** pages (P5-SKILLS-2's frames): there the switch
column is **For <Workspace>/<Org>** (the skill's own on/off) and rows say "Used by N agents".

Decisions: **D-496**, **D-498**, **D-499**, **D-505**, **D-506**(d) (words). Contracts: **C-1173, C-1174, C-1175** (from
P5-SKILLS-1), C-1099 (read one file), C-1035 (`?draft=`).

## Linked requirements (look at every image before coding)

- `docs/design/skills-memory/README.md` (binding), `review.md` S-1..S-7, S-12, proposals skill-1..skill-4, "Also" 3.
- **After:**
  - `after/skill-1.png` / `-light.png` / `.html` — the list: header **Add skill** with its menu (Write a new skill · Add
    from a library · Upload a skill folder), search, chips "All 12 · Daneel's own 6 · From the workspace 2 · From Acme 4 ·
    Off 1", columns **Skill · Source · For Daneel**, rows "Acme style guide / Use it to write anything that leaves the
    team in Acme's style. / Acme / On", the override line "Daneel's version replaces the workspace's · Compare".
  - `after/skill-2.png` / `-light.png` / `.html` — the panel for "Triage alerts": name, source chip, switch, "Use it to …",
    **Edit**, **Try it in chat**, "…"; **Instructions** rendered; **Files · 2** as a tree (SKILL.md 242 chars;
    scripts/firing.sh, Executable, 130 chars, opened in a code view); "Used 3 times this week · last in “Alert storm 26
    Nov”".
  - `after/skill-3.png` / `-light.png` / `.html` — read-only: "Onboarding new hire · Acme · On", the lock line
    "Managed by Acme's admins. You can turn it off for Daneel.", Try it in chat, rendered instructions, files, "Used by 14
    agents in Acme · Daneel used it 0 times this week"; the second frame: a **library** skill (pdf) with "From Anthropic
    skills · pinned to <sha>", **Update available** and **Review update** (for those who may approve).
  - `after/skill-4.png` / `-light.png` / `.html` — "Edit skill": Name, "Folder: summarize-pull-requests", "When should
    Daneel use it?" with "Finish “Use it to …”. Example: write the Friday status report from Jira and GitHub.",
    Instructions Write/Preview, "Markdown · 412 of 24,000 characters", **Files · 2 SKILL.md, template.md · Add file**
    (folded), the SaveBar; below: "Files, opened" (each file: size, Close, Delete, an inline editor) and the **Add a file**
    dialog (File name "Relative to the skill's folder.", Content "Paste the file here, or upload one", Executable
    "Daneel can run it from the computer.", Cancel · Add file). The single-line "When should … use it?" input must not
    wrap like the mockup's div does at 1x.
- **Before:** `before/S01-skills-agent.png`, `S02-skill-open.png`, `S03-skills-default.png`, `S04-skill-new.png`,
  `S05-org-skill-from-agent.png`, `S06-ws-skill-from-agent.png`, `S08-agent-skills-after-library.png`,
  `S09-agent-skills-full.png`, `L10-library-skill-open.png`, `P03-phone-skills.png`.
- `docs/14-web-dashboard.md` §Visual design, agent Skills (~1900–1916), skills form (~2797–2801).

## Wave and order

After **P5-SKILLS-1** (titles, off, usage), **P5-SKILLS-2** (the page frames) and **P5-MEMORY-2** (the shared
`ui/markdown-editor.tsx` Write/Preview editor and the phone sub-screen helper — use them, don't fork them). P5-SKILLS-4
follows (it owns `library.tsx`'s add-from-library dialog).

## Context (what exists today)

- `web/src/components/skills/skills-tab.tsx` (agent page: `SkillsEditor` + `EffectiveSkillsView`),
  `web/src/components/skills/skills-view.tsx` (524 lines: `SkillsEditor` 57–135, `whenLine` 137–141 — the glued label,
  `onFor` 144–147 — keyed on the scope passed in, `CreateSkill` 153–200, `OverriddenIn` 203–218, `SkillReadOnly` 221–232
  — **always passes the organization scope: the S-4 bug**, `SkillEditor` 234–313, `SkillFiles`/`FileTree` 315–404 behind
  `Altitude id="skill.files" level="expert"`, `EffectiveSkillsView` 410–524 with "Skills this agent receives", "From
  apps", "Not applied"). Names: `web/src/lib/skill-name.ts` (`skillDisplayName` re-cases the slug: "Summarize pr"; it also
  capitalizes one-word names, "Pdf", which D-498(b) keeps as written).
  Client `web/src/lib/api/skills.ts` (`readSkillFile` C-1099; `SKILL_PATH_MAX_DEPTH = 8` while the server allows **4**;
  no character-set or 50-file check). Library parts used by the panel: `LibraryBadge`, `PendingUpdatesNotice`,
  `LibraryManagedNotice`, `UpdateReviewDialog`, `LibrarySkillActions` (Review update, **Detach**) in `library.tsx`.
- Try it in chat: `wsRoutes(org, ws).agent(handle, { tab: "chat", draft })` (C-1035), as `connect-journey.tsx:486-488`
  does; regression spec `web/e2e/try-it-in-chat.spec.ts`.
- Primitives: `Markdown`, `TextViewer` (code view), `DiffView` (for Compare), `SwitchField`, `ChoiceChips`, `Menu`,
  `Dialog`/`FormDialog`, `SaveBar`, `Badge`, `When`, `MasterDetail` (`states.tsx:146`), the new `MarkdownEditor`.

## Scope

1. **One list** (agent, workspace and organization pages): search; chips (agent: All · <Agent>'s own · From the
   workspace · From <Org> · Off; workspace/org: All · Written here · From libraries · Off); columns **Skill · Source · For
   <Agent|Workspace|Org>**; a row = display name (title, else the folder name made readable — dashes to spaces, first letter capitalized, a one-word
   name as written: "Internal comms", "pdf" — D-498(b)), the description as a
   sentence (**no** "Use it when" prefix; capitalize the first letter only), one source chip (<Agent> · Workspace ·
   <Org> · Library · <library label> · App), the switch. Agent page: the switch of an inherited skill is the **per-agent
   off** (C-1174), of the agent's own skill its `enabled`; app skills show a read-only "On" (D-455); a skill a higher
   level replaces shows "<Agent>'s version replaces the workspace's · **Compare**" (Compare opens `DiffView` of the two
   instructions). Workspace/org pages: the switch is the skill's own `enabled`, and rows add "Used by N agents". The
   precedence paragraph and the "Not applied" list go; a small "How skills combine" link keeps the explanation (a `Fold`
   or the docs). Sorted by name.
2. **The panel (skill-2)**: selecting a row opens the skill in a right-hand panel beside the list (no inline editor
   between sections, nothing at the bottom of the page; the selection is in the URL, `?skill=<id>`); header: name, source
   chip, the switch; the "Use it to …" line; **Edit** (only where editable), **Try it in chat** (agent page only: opens
   the chat with the draft "Use the “<name>” skill to "), "…" (**Duplicate as <Agent>'s own** where it makes sense,
   **Remove from <Agent|this workspace|Org>…**, **Stop updates and edit** for a library skill with its explanation);
   **Instructions** rendered with `Markdown`; **Files · n** as a tree, each file opening in a read-only code view
   (`TextViewer`) through C-1099, "Executable" tag, size; the usage line from C-1175 ("Used n times this week · last in
   “<conversation>”", "in a conversation" when not readable, "Not used yet"); on workspace/org pages "Used by N agents".
   On a phone the panel is a sub-screen with "‹ Skills".
3. **Read-only (skill-3)**: an organization or workspace skill seen from an agent, a library skill, an app skill: **no
   input fields**; the lock line "Managed by <Org>'s admins. You can turn it off for <Agent>." (workspace: "Managed by
   the workspace's owners …"; app: "Comes with the <App> app."); a library skill adds "From <library> · pinned to
   <short sha>", and when an update waits: **Update available** with **Review update** for people who may approve
   (the existing `UpdateReviewDialog`), nothing for others. Never a Save button. **Fix S-4**: labels come from the
   skill's own tier, never from the page's scope.
4. **Editor (skill-4)** in the panel: **Name** as typed (title) with "Folder: <slug>" derived and shown small (fixed
   after create); "When should <Agent> use it?" (single line) with the hint and example as drawn; **Instructions** with
   the shared `MarkdownEditor`, "Markdown · n of 24,000 characters"; **Files · n <names> · Add file** folded to one line,
   opening to the list where each file can be read and **edited in place** (text files; binary files show size and
   Replace/Delete), Close, Delete (confirm); **Add file** is a dialog: File name ("Relative to the skill's folder."),
   Content (paste, or "upload one"), **Executable** switch, Cancel · Add file; the `SaveBar`. No always-open upload form,
   no native "Choose File". Validate paths as the server does (depth **4**, `[A-Za-z0-9._/-]`, ≤200, no `skill.md`
   segment, ≤50 files) and fix `SKILL_PATH_MAX_DEPTH`. Remove the `skill.files` expert altitude: files are folded, not
   hidden.
5. **Add skill** menu: **Write a new skill** (the editor, empty), **Add from a library** (opens P5-SKILLS-4's dialog;
   until it merges, today's `AddFromLibrary`), **Upload a skill folder** (a folder picker: read `SKILL.md` front matter
   `name`/`description` and body, show what will be created — name, files, size — then create the skill and its files
   through the existing routes; errors per file in plain words before anything is written).
6. **Words (D-506(d))**: Turn off for <Agent> (the switch's accessible name), Remove from …, Stop updates and edit;
   never two of these as neighbouring buttons; display names everywhere except the folder line; delete `whenLine`, and make `skillDisplayName` prefer the title and
   apply D-498(b) otherwise.

## Out of scope

The add-from-library dialog's new design (P5-SKILLS-4); page frames, navigation and libraries (P5-SKILLS-2); any API
change (P5-SKILLS-1).

## Constraints / file ownership

Owns `skills-tab.tsx`, `skills-view.tsx` (rewrite or split into `skills/list.tsx`, `skills/panel.tsx`,
`skills/editor.tsx`, `skills/files.tsx`…), `web/src/lib/skill-name.ts`, `web/src/lib/api/skills.ts`, the panel's use of
`library.tsx`'s notices and `UpdateReviewDialog` (do not restyle the add-from-library dialog), `altitude-registry.ts`
(`skill.files` only). Tokens and shared primitives only; both themes; 390 px without horizontal scroll.

## Acceptance criteria

- With the review's data, the agent page matches `after/skill-1.png`, the panel `after/skill-2.png`, read-only
  `after/skill-3.png` (both frames), the editor and dialog `after/skill-4.png`, in dark and light at 1256 px; the phone
  shows list and panel as two screens.
- Turning "Onboarding new hire" off for Daneel works from the row and the panel, shows in the Off chip, and leaves it on
  for other agents; a workspace skill is never labelled with the organization's words.
- Try it in chat opens the chat with the draft; files can be read, edited, added through the dialog, and a skill folder
  uploaded.
- One list, no duplicate section, no "Not applied", no slug as a title outside the editor.

## Tests

Vitest: list (chips, sources, switches per tier incl. app read-only, override + Compare), panel (rendered instructions,
file tree and viewer, usage line incl. privacy fallback, Try it in chat link), read-only (no inputs, lock lines per tier,
library update states), editor (title → folder, sentence hint, files fold/open/edit, Add file dialog validation, SaveBar),
folder upload (parsing, errors), S-4 regression. Playwright (mocked): agent page list → panel → edit → save; turn off an
inherited skill; library skill panel; workspace and org pages; 390 px both themes; axe clean.

## Tests move with the UI (owner, 2026-11-20 — binding)

1. Before each commit that changes visible text, a label, an `aria-label`, a role, a heading, a route or structure:
   `rg -n "Skills this agent receives|Not applied|Use it when|On for the|Detach|<old selector>" web/tests web/e2e
   scripts/e2e` and update every hit **in the same commit**: `web/tests/skill-form.test.tsx`, `workspace-skills.test.tsx`,
   `skill-library.test.tsx` (its org-label assertion ~93 follows the tier rule), `memory-skills.test.tsx`,
   `refine-20-batch3.test.tsx`, `polish-2-primitives.test.tsx`, export/import/template tests; `web/e2e/agent-content.spec.ts`,
   `agent-tabs.spec.ts`, `altitude.spec.ts`, `skill-libraries.spec.ts`, `try-it-in-chat.spec.ts`, `support/mock-api.ts`;
   production specs.
2. New expectations use the mockup's words exactly; never loosen, skip, `fixme` or delete a test to get green.
3. Run through `scripts/dev/test.sh` only (D-491): vitest files you changed and those rendering them; shared skill UI
   changes, so the **whole** vitest suite once (`scripts/dev/test.sh <lane> web vitest`); every Playwright spec visiting a
   skills page (`--project=mocked`); lint, typecheck, build through `scripts/dev/heavy.sh <lane> …`.
4. Production specs: update by reading; list them. 5. A merge that turns master red with a stale test comes back here.

## Docs to update

`14` (agent, workspace and org Skills: the list, panel, read-only, editor, files, Add skill menu, words), `10` §Skills
(what a person can do: turn off for one agent, upload a folder), PROGRESS and FEATURES rows at every commit.

## Suggested agent/model and reason

Sonnet `nextjs-developer` (the largest web brief of the set: a list/panel/editor rebuild shared by three pages). Split
into commits per part (list, panel, read-only, editor, files, upload).

## Completion report

Built 2026-11-27 on `alexiri/p5-skills-3`. **Not merged, not pushed.** Web only: no API, DTO, migration or Rust change.

**Commits:** `cea396f1` (the work), `b7ab7745` (merge of master: one conflict, the imports of `memory-skills.test.tsx`;
the compact list now uses master's `row-rail`), and the commit that adds this report (one unused import, docs).

### What changed

- `components/skills`: `skills-view.tsx` and `add-skill-button.tsx` removed (with the `create` prop). New `model.ts`
  (the rows and every word, pure), `list.tsx`, `panel.tsx`, `files.tsx` (read tree, Add a file, the editor's files),
  `editor.tsx`, `upload.tsx`, `skills-list.tsx` (the one page for all three levels), `header-slot.tsx`.
- `library.tsx`: `LibraryBadge`, `LibraryManagedNotice` and `LibrarySkillActions` removed; `AddFromLibrary` is opened by
  the menu (the dialog itself is unchanged, for P5-SKILLS-4); the update dialog's title uses the display name.
- `lib/skill-name.ts` (D-498(b), the "Use it to …" line), `lib/skill-folder.ts` (new), `lib/api/skills.ts` (the
  server's path rules, depth 4; per-agent off; create with files; the effective set is re-read before a write resolves).
- Small, outside the brief's list: `agent-page.tsx` (passes the agent's address and workspace; drops `?skill=` when
  leaving Skills), `topic-page.tsx` (the same for the organization's tabs). No shared `ui/` file changed.
  `skill.files` is gone from the altitude registry.

### Readings of the brief to check

1. **List rows show the description as written** (first letter capitalized), without "Use it to". Scope 1 and
   D-498(c) say so; the brief's own description of `skill-1.png` and the mockup show the prefix. The panel has it.
2. The "Use it to …" line is added only for skills written in Aurora. Library and app text, and a description that is
   a sentence of its own ("Use …", "When …", "Someone …", "This …"), stay as written: "Use it to comprehensive PDF
   toolkit" would be wrong, and repository text does not follow Aurora's sentence rule.
3. A switch is named "Turn off for <Agent>: <skill>" so that the switches of a list can be told apart.
4. In the editor, **Add file** and **Delete** of an existing skill are written at once (each is its own confirmed
   action); text edited in place is saved by the save bar. A new skill's files are written on Save.
5. A skill with files (new, uploaded or duplicated) is created **off**, then switched on once every file is there; if
   a file is refused the skill is removed again.
6. **Duplicate as <Agent>'s own** is offered for inherited skills, and for app skills only when they have no other
   files (no route reads an app skill's file for an agent; those files are listed but do not open).
7. An uploaded file is made executable when it starts with `#!` (a browser does not give the bit). `.DS_Store`,
   `Thumbs.db`, `desktop.ini`, `__MACOSX` and `.git` are left out and named. Only `name`, `title` and `description`
   of the front matter are read.
8. A skill another level's skill of the same name would outrank is not a row even when it is off.
9. Choosing the open skill again closes the panel (the mockups draw no close button).
10. The panel offers **Review update** only to who may decide. The dialog's "you can read, not approve" state is no
    longer reachable from a skill page; its test renders the dialog directly.

### Found while testing

- **The Add file dialog saved the whole skill.** A portal's submit bubbles through React to the editor's form; fixed
  with `stopPropagation`, covered by `skill-form.test.tsx`.
- **A resize across 1100 px lost what was typed** (a Playwright full-page capture did it): `ListDetail`'s two layouts
  are different trees. The skills page keeps the layout a form was opened in until it closes. **The Memory page uses
  the same component and probably has the same loss**; not changed here (not this brief's files).
- At 768 px the table's name column had no width; it now follows the table's own width (`@container`).
- `tailwind-merge` reads `text-label` as a colour and drops it beside `text-*` colours, so **every `Badge` renders at
  14 px** app-wide. The skill chips and tags set `text-xs`; `ui/badge.tsx` is not changed.

### Comparison with the mockups

Looked at `after/skill-1..4` (dark and light, and the HTML for the words) and `skill-6` before coding, and against the
built pages at 390, 768, 1256 and 1280 px in both themes (mocked data shaped like `seed.py`, not a seeded cluster).
`SKILLS_SHOTS=<dir> scripts/dev/test.sh <lane> web playwright e2e/skills.spec.ts --project=mocked` writes the
screenshots again (`skill-1-1256-{dark,light}` ↔ `after/skill-1{,-light}.png`, `skill-2-…` ↔ `skill-2`, `skill-3-…`
and `skill-3-library-…` ↔ the two frames of `skill-3`, `skill-4-…`, `-files-`, `-add-file-` ↔ `skill-4`,
`skill-6-{workspace,organization}-…` ↔ the first frame of `skill-6`, `*-390-*` the phone).

| Difference kept | Reason |
|---|---|
| Rail, agent header, Tests / Other agents, "Showing more controls", sample names and numbers | The real shell and mock data (README §How to read 2, 3) |
| List rows and the pdf panel's line have no "Use it to" | Readings 1 and 2 |
| **Add skill**, **Save**, **Add file** are the shared primary button; **Try it in chat** the `approve` variant | Shared `Button` |
| The panel's … button is the shared outline button with `MoreIcon` | Shared primitives, as on Memory |
| Write / Preview are underline tabs inside the box, with "Markdown · n of 24,000 characters" beside them (mockup: segmented tabs above, the count below) | The shared `MarkdownEditor`, which this brief must use and not fork |
| The save bar floats at the bottom of the window with no amber dot (mockup: the panel's foot); a new skill's says "New skill · not saved yet"; an unchanged edit has **Done** | The shared `SaveBar`; a hidden bar leaves nothing to leave the form with |
| "When should … use it?" is one line that scrolls (mockup wraps) | The brief: it must not wrap |
| The **Executable** switch is on the right; the dialog's footer keeps a line for errors | Shared `SwitchField` and `FormDialog` |
| **Update available** is the shared amber chip, then "— 3 files changed upstream." and **Review update** below; no "…" icon in the box | `AttentionChip` is the only amber element (D-384) |
| No "Only <Org>'s admins can review and apply updates." and no disabled button for others | Scope 3: "nothing for others" |
| A library row also carries **Update available** in the list | The pending-updates notice tells people to open a skill marked so |
| Below a table width of 36 rem each row stacks (chip and switch under the text) | No room for three columns (`14` §Visual design) |
| The library panel shows **Instructions** and a file tree (`scripts/` then `merge.py`), not a flat list; every file has its size | One panel for every skill (scope 2: files as a tree) |
| The usage line's icon is the activity icon | No such "…" glyph in the shared set |
| "Acme Labs’s admins" | The mock organization's name in the mockup's pattern |
| Level pages: "How skills combine" under the list; their header and tabs are P5-SKILLS-2's | Scope 1 |
| Sub-screens below 1100 px, not only on a phone | The shared `ListDetail` (as P5-MEMORY-2) |

### Tests

- New: `tests/skills-page.test.tsx` (list, chips, switches per tier, Compare, panel, files, usage and its privacy
  fallback, Try it in chat link, read-only per tier and S-4, library update states, Stop updates, Duplicate, app
  skills), `tests/support/skills.tsx`, `e2e/skills.spec.ts` (28: 390, 768, 1256 and 1280 px, both themes, axe serious
  and critical, no sideways scroll), `e2e/support/skills.ts`.
- Rewritten for the new UI, still-true cases kept: `skill-form.test.tsx` (names, 409 retry; now the editor, files,
  Add a file, folder reading and upload), `workspace-skills.test.tsx` (its effective-set cases moved to
  `skills-page`), `skill-library.test.tsx`.
- Changed: `memory-skills` (path rules to the server's; the old inline upload case is the Add a file case in
  `skill-form`; the total-size case now reads the editor's sentence), `agent-empty-states`, `refine-20-batch3`,
  `skill-libraries-ui`; e2e `skill-libraries` and its support (the menu), `agent-content` (an app skill in the one
  list), `support/mock-api.ts` (an empty effective set; without it the agent page crashed in `agent-tabs`).
- No assertion skipped or loosened. Production specs, `scripts/e2e` and `tests/e2e-hosted`: searched, no skill-page
  string or selector; **no production spec to run nightly** for this brief.

### Commands and results

- Fail first: the new `skills-page`, `skill-form` and `workspace-skills` on `6b56bb21` (a temporary detached
  worktree): 3 files failed, **33 tests failed**; `skill-form` could not import `lib/skill-folder`.
- On the merged tree, `test.sh … web vitest` on 30 files (the four drift guards `audit-actions`, `arbitrary-values`,
  `ui-text`, `labels`, the other source guards, and every file of or rendering a changed component): **431/431**.
  The whole suite was not run (the dispatch's minimal gate; CI runs it).
- `test.sh … web playwright skills agent-tabs altitude try-it-in-chat skill-libraries agent-content
  --project=mocked` on the merged tree: **180/180**; `skills` again at the end: 28/28. Before the merge,
  `agent-tabs stage4-routes stage4-axe ia-navigation` filtered to the agent and skills pages: 40/40.
- `pnpm typecheck`, eslint on the changed files (through `heavy.sh`), `cargo fmt --all --check`,
  `scripts/guards/run-all.sh`, `extract-schema.py --check`: pass. No Rust changed, so no clippy. No local
  `pnpm build` (CI).

### Security notes

No SEC row. No new request type: the per-agent off, file and skill routes decide who may; the buttons only follow the
roles in `/v1/me`. A conversation is named only from the API's `ConversationRef`, else "in a conversation". Names,
descriptions and files from libraries and apps are React text. **For the reviewer:** the panel renders library and app
`SKILL.md` instructions with the shared sanitizing `Markdown` (scope 2), where they were plain text before. Scripts and
handlers are stripped, an outside link or image is a link that opens only when pressed, but a **same-origin** image
path is loaded, as in chat and memory, so opening such a skill can make the viewer's browser `GET` a path of this
origin.
