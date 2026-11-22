# P6-TPL-4: Agent templates — the dashboard: Administration › Agent templates (git-address Add, review page), the organization's Templates tab, More templates… and the four-step page, the agent's Template section with helper files (owner, 2026-11-21 and 2026-11-22, phase 6, D-435, D-436, D-456, D-457, C-1114..C-1117, C-1142, C-1143, SEC-146)

## Why
These are the screens of `docs/tasks/P6-TEMPLATES.md`:
- an instance admin adds template repositories by their git address, as for a skill library, and reviews every
  version before it is offered, while the previous version stays in use;
- organization owners turn templates off;
- anyone who can create agents makes one from a template in four steps, with library skills where the organization
  has them;
- an agent made from a template shows its helper files and is offered newer versions part by part.

Read first:
- `P6-TEMPLATES.md`;
- `API_CONTRACTS` **C-1114..C-1117** (the screens), C-1107..C-1112, C-1142 and C-1143 (the routes they call; the
  OpenAPI types are generated);
- **C-1129** and the P5-SKLIB-3 components (`AddLibraryDialog`, `EditLibraryDialog`, the status words): the Add and
  Edit dialogs here are the same pattern, so reuse or generalize those components, and do not write a third address
  form;
- `14 §Visual design` (tokens, primitives, page patterns, both themes, no horizontal scroll at 390 px) and
  `14 §Information architecture` (topic and tab registry);
- `14 §Agents, accounts and access`: `AccountsList`, `AccessControl` C-938, the connect journey C-914, summary
  sentences C-986, `WhoCan` C-981;
- `14 §Agent settings pages` (New agent, Start from, D-413; the Manage page, D-447) and `§Export and import an agent`;
- `17` SEC-146 (untrusted text as data).

## Scope

### 1. Administration › What agents may do › Agent templates (C-1114)
- Add the tab `templates` after `defaults` in `TABS.adminAgentPolicy` (`web/src/lib/routes.ts`), labelled "Agent
  templates", with the tour and visibility of the other tabs of that page.
- **The sources list**: repository, branch or "default branch", folder, `sha7`, schedule, last checked with status,
  template count. **Check now**, **Edit**, **Remove** (its confirm says agents keep working).
- **Add** is the two-step dialog of C-1114, built on the library dialog's components (`scope`-like props, or a shared
  `RepositoryAddressFields` extracted from it):
  - Step 1: address, branch or tag, folder, and the private fold with token and user name, then **Check**.
  - Step 2: what Aurora found (repository name and description, branch and `sha7`, the packages address as text, each
    template with version, state and problems in words), then **Add repository**. **Back** keeps the fields.
  - `template_url_invalid` shows on the address field in words per `details.reason`; `template_unreachable` shows
    above the fields.
- **The templates list**: the chips of C-1114 ("Waiting for review" with the count, "Waiting for its package" with
  since when, "Removed from the repository", the head-problems line), the apps and libraries it needs, and
  **Withdraw** / **Offer again**.
- **Review page** (`/admin/internet-access/templates/<template_id>/<version>`) shows the full `TemplateContentView`:
  - instructions;
  - each skill (use-it-when, body, files with sizes, the text of text files, "binary, <size>" otherwise, Executable);
  - **helper files** (the same file view; executables always as text);
  - **skill references** (kind, library repository with branch and folder, skill name, why, and the C-1114 sentence);
  - memories (a locked one marked "Locked: <Agent> can read it but not change it", C-1151);
  - automations (cadence in words, and the cron in Technical details);
  - apps (name or "Not installed here", level in words or the tool lists by tool name, the why);
  - inputs.
  **About this template** (the README) comes first, as source text with a **Preview** toggle that renders it as
  people will see it. Above it all come the heading "Text from <source name> at <sha7>" and "The package matches the repository at <sha7>".
  **Every repository text is rendered as plain text**: no Markdown rendering, no links made clickable, no HTML. What
  the admin reads is what the agent gets. With an approved version, show a diff per part (an existing diff primitive
  if the repo has one, else add one to the shared set). For a newer version, the sentence "People keep using
  <current version> until you approve this one." **Approve** sends `content_sha256` (on 409, reload and say the
  version changed); **Reject** asks to confirm.

### 2. Organization › Agents › Templates (C-1115)
Add the tab `templates` in `TABS.orgAgents`: the list with one switch per template and the sentence of C-1115. It is
for org owners and admins only; others get `WhoCan`.

### 3. New agent from a template (C-1116)
- **More templates…** goes in the New agent dialog's **Start from** row (after the built-in chips) when
  `listWorkspaceTemplates` returns any. It opens the gallery (a dialog or page per `14`'s patterns) of
  `TemplateCard`s, with the not-ready sentences for apps and for libraries.
- **The template's details view** (D-458, C-1116) opens when a card is chosen, before anything else. It shows the
  name, avatar and summary; **About this template**, the README rendered; what it needs; what it runs on its own;
  its helper files by name; and the version and source. Then **Use this template** or **Back**.
- **README rendering**: one small `TemplateReadme` component that wraps the shared `Markdown`
  (`web/src/components/ui/markdown.tsx`) with the C-1116 rules: no raw HTML; images not loaded (shown as their alt
  text); only `https:`/`mailto:` links, opened in a new tab and marked external; relative links as plain text;
  headings shifted under the page's heading. If `Markdown` cannot be configured that way through its props, add the
  options to it (its other callers unchanged) rather than writing a second renderer. It is used in the details
  view, in the fold above the steps, in the admin review's Preview and in the update dialog.
- The page `/o/<org>/w/<ws>/agents/new/<template_id>` has a closed fold **About this template** (the README) above
  four steps (a stepper from the shared set; add one if
  missing): **About**, **Accounts**, **Automations**, **Review**, exactly as C-1116.
  - **Accounts**, per entry:
    - the why;
    - an `any_of` choice;
    - the accounts the plan lists, in the `AccountsList` row style, with health;
    - **Connect a new account**, which opens the connect journey (C-914) in its account-only form (App → Account →
      Sign in → Done, with no agent and no access step) and returns to this step with the new account selected. Add
      that form to the journey if it does not exist; it must not grant anything;
    - `AccessControl` preselected to the template's level, capped with "Limited by <org>" and the `custom_only` note;
    - dropped tools, said in one line;
    - for an optional entry, a skip ("Skip — <Agent> won't use <app>").
  - **Review**:
    - the summary sentences of C-986 for what <Agent> will be able to do;
    - the instructions, skills, helper files and memories, in full, as plain text, folded per part (locked memories
      marked as in the admin review, C-1151);
    - **Skills from libraries**: each available reference with its library (label and repository), its content at the
      commit, as plain text and folded, and for optional ones a switch "Add <skill>" (on by default);
    - unavailable optional references as "Not added: needs the skill library <repo>";
    - organization references as "Uses your organization's <name> skill".
  - **Create** (C-1111) sends `skill_refs` for the switched-on references, with the plan's `library.id` and
    `commit_sha`. Field errors are mapped to their step. A 409 reloads the plan with the sentence of C-1116. On
    success, show each webhook secret once (`SecretOnce`), then open the agent's chat with the toast "<Agent> is
    ready".
- Phone width: every step is usable at 390 px without horizontal scroll.

### 4. The agent's Template section (C-1117)
It goes on the agent's Settings › Manage page (D-447), above Export and import. It is shown only for agents with
`template` or with helper files.
- The sentences of C-1117.
- **Helper files**: the count, **View** (a dialog listing path, size and Executable, with the text of text files as
  plain text, via `getAgentHelper`) and **Remove** (one or all, with a confirm).
- **Review changes**, a dialog that starts with the new version's README in a fold, then lists:
  - each part (instructions, skills, helper files, library skills), with its state in words and a full diff;
  - the checkboxes and defaults of C-1117;
  - for a new library skill, its availability (or the "Needs the skill library" sentence and no checkbox);
  - the inputs needed;
  - the apps added, with **Give access** links to the Access tab's journey.
- **Apply**, **Skip this version**, **Stop following this template** (with a confirm).

### 5. Words
Add the new words to `14`'s vocabulary table: Agent template, Template repository, Waiting for review, Waiting for its
package, Offered in <org>, More templates…, Helper files, Skills from libraries. Keep the vocabulary test's exempt
lists empty.

## Not in this brief
Save as template (P6-TPL-5); any Rust.

## Tests move with the UI (owner, 2026-11-20 — binding)
In every commit, find every test that sees what you change, and update it in the same commit. That means
`web/tests/**`, `web/e2e/**` (the mocked **and** the `production` projects), `scripts/e2e/*.sh`, fixtures and mocks.
At least run `rg -n "<old text>|<old selector>" web/tests web/e2e scripts/e2e services crates` per change.

Never loosen, `.skip`, `test.fixme` or delete a test to get green.

Run through `heavy.sh`:
- every vitest file you changed, and every one that renders a component you changed;
- every mocked Playwright spec that visits a page you changed.

A change to shared UI runs the **whole** vitest suite once
(`cd web && /home/alex/orca/workspaces/aurora/heavy.sh <lane> pnpm exec vitest run`). Shared UI here means the New
agent dialog, the agent Manage page, the library dialogs you generalize, the stepper or diff primitives, and the routes
registry.

Production specs are updated by reading, and listed in the completion report.

## Acceptance criteria
- [ ] Vitest for each screen:
  - the admin tab: sources; the two-step Add with Check, Back and errors in words; check; edit with token replace and
    remove; remove;
  - the review page: plain-text rendering of hostile Markdown, HTML and links; every part shown in full, helpers and
    references included; the diff; the "keep using" sentence; approve with the hash; the 409 path;
  - the organization tab;
  - the gallery: ready and not-ready cards, for apps and for libraries;
  - the details view: the README rendered, what it needs, Use this template, Back;
  - `TemplateReadme` with a hostile README (raw HTML, a `<script>`, a `javascript:` link, a remote image, a relative
    link, a very long line at 390 px): none is active or loaded, and nothing scrolls horizontally;
  - each of the four steps: the `any_of` choice; an existing account; connect-new returning with the account; access
    capped and custom; skipping an optional entry; library skills switched on and off; an unavailable optional
    reference; field errors routed to their step; the 409 reload;
  - the Template section: helpers view and remove; states for every part kind; default checkboxes; apply; skip;
    detach.
- [ ] Library dialogs: their existing tests stay green after the shared fields are extracted.
- [ ] Playwright (mocked project): add a source (Check, then Add) and approve a version; turn a template off in an
  organization; create an agent from a template end to end, starting from the gallery's details view (one existing account, one connected in the journey, one
  skipped, one library skill added) and land in its chat with the webhook secret shown once; apply part of an update
  that includes the helper files. All at 390 px and 1280 px, with both themes for the review page and the four steps.
- [ ] `pnpm exec tsc --noEmit`, lint, the vocabulary and route guards, and `pnpm build`. The routes registry test
  covers the new tabs and paths.

## Ownership and sequencing
- **Owns:**
  - `web/src/app/**` pages for the routes above, and `web/src/components/templates/**` (new);
  - the New agent dialog's Start from row;
  - the agent Manage page's new section;
  - the connect journey's account-only form;
  - the shared repository-address fields extracted from the library dialogs (library behaviour unchanged);
  - `routes.ts` entries, and the shared primitives you add;
  - `14`'s vocabulary rows and the section for these screens.
- Parts 1 and 2 can start when P6-TPL-2 is merged. Parts 3 and 4 need P6-TPL-3's and P6-TPL-6's routes: mock them
  from the contracts before those merge, and finish after. One web lane.

## Numbers
D-435, D-436, D-456, D-457, D-458, D-463(d), C-1114..C-1117, C-1142, C-1143, C-1151 (review mark), SEC-146. No
migration.

## Suggested agent/model and reason
`nextjs-developer` (Sonnet). Several new screens on existing primitives (the library dialogs among them), and one
multi-step flow that reuses the connect journey and the access control.
