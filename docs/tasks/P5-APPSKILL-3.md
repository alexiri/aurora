# P5-APPSKILL-3: Apps ship skills — the dashboard (owner, 2026-11-22)

## Goal

After P5-APPSKILL-1/-2 a package ships skills (D-453) and the API serves them (C-1133, C-1136). This
brief shows them to people:

- an instance admin sees, on the plugin page and in the permissions review, what each app gives agents:
  summary, essentials, operations file, and **skills** with their files, scripts marked, changes marked;
- the approve control of a pending version says when its skills changed;
- on an agent's Skills page, app skills appear read-only under "From apps".

The word "Agent guide" leaves the dashboard.

## Decisions that bind this brief

- **D-453** (d), (g), (h); **D-452** (c) (a script change is shown before approval).
- **SEC-150** (plain text only, executables marked); `14 §Visual design` (tokens, shared primitives and
  page patterns, both themes, no horizontal scroll at 390 px).
- Contract **C-1139** (and the shapes of C-1133, C-1136).

## Order and lanes

- One Next.js lane, after P5-APPSKILL-1/-2 have merged. Size S–M.
- Disjoint from P5-APPSKILL-4 (which edits only `plugins/builtin/indico/**` and one test script); they can
  run at the same time.

## Read first

- `docs/API_CONTRACTS.md` **C-1139**, C-1133, C-1136; `docs/14-web-dashboard.md` §Visual design and
  §Instance administration (:2171; the plugin section :2272).
- Code: `web/src/components/admin/agent-guide-section.tsx` (whole file), `plugin-detail-view.tsx`
  (:30–37, :132, :419–423), the approve control of a pending version (find it from
  `plugin-detail-view.tsx`), `web/src/lib/api/plugins.ts` (:62–72), `web/src/components/skills/skills-view.tsx`,
  `web/src/lib/api/skills.ts`, the shared code viewer and `Badge`/`Fold`/`DataTable` primitives.

## Work

1. Replace `AgentGuideSection` with **`AgentContentSection`** ("For agents"): per app the summary, the
   essentials (plain text, preformatted), the operations file with its count and a View link, and a
   **Skills** list: name, description, size, a fold per skill with its files (path, size, "Script" badge
   for executables), change marks (`added`, `changed`, `removed`, `mode` → "now a script" / "no longer a
   script"). A file opens in the shared code viewer as **plain text** from
   `adminGetPluginAppSkillFile` / `adminGetPluginOperationsFile`. The review sentence of C-1139.
2. The pending version's approve control: the one-line skills summary of C-1139 above the button, linking
   to the section.
3. Skills view: a "From apps" group for `tier: "app"` items ("From the <App name> app · <version>"), no
   edit, disable or delete controls; shadowed app skills in the shadowed list naming the winner; handle
   `id: null` everywhere the view keyed by id.
4. Remove the old hook `useAgentGuideFile` and every "Agent guide" label.

## Tests move with the UI (owner, 2026-11-20 — binding)

Every commit updates every test that sees what it changes: `web/tests`, `web/e2e` (mocked **and**
production projects), `scripts/e2e`, fixtures; never skip, `fixme`, delete or loosen. Start from
`rg -n "Agent guide|agent-guide|AgentGuide|agent_guides|useAgentGuideFile|recipe" web scripts/e2e`.
Known: `web/tests/agent-guide-section.test.tsx` (→ `agent-content-section.test.tsx`),
`web/e2e/agent-guide.spec.ts` (→ `agent-content.spec.ts`), the skills-view tests and e2e, the mocked API
fixtures for `PluginPackage` and effective skills.

## Acceptance

1. Vitest: the section renders summary, essentials, operations count, skills, files, "Script" badges and
   every change mark; a file whose text contains HTML and Markdown renders inert; the approve line counts
   added/changed/removed and scripts; the skills view shows app items read-only with no action buttons,
   and a shadowed app skill names its winner.
2. Mocked Playwright: the plugin page and the review of a pending version with changed skills, both
   themes, 390 px without horizontal scroll (`expectThemedAccessible`); the agent's Skills page with an
   app group.
3. No "Agent guide" text left in `web/src` (`rg -n "Agent guide" web/src` empty).
4. Gates (through `/home/alex/orca/workspaces/aurora/heavy.sh <lane> …`): `tsc`, lint, vitest on the
   touched files, the two Playwright specs (mocked); CI is the full gate.

## Out of scope

- Hiding app skills per organization (never: owner, D-455; the view offers no switch); editing an app skill (never: change the package).

## Completion report

Files changed, test files changed or renamed, commands and results, screenshots of both themes at 390 px
and desktop for the section and the skills view.

### Done 2026-11-22 (lane B, branch `alexiri/p5-polish-b`, Sonnet)

**Files changed.** `web/src/components/admin/agent-content-section.tsx` (renamed from `agent-guide-section.tsx`: `AgentContentSection`,
`SkillsChangedLine`, `skillsChangeSummary`, `packageHasAgentContent`), `plugin-detail-view.tsx` (heading "For agents", fold "What agents get",
section id `agents`, anchor `#for-agents`, the skills line inside the summary card above Approve for a pending version),
`web/src/components/skills/skills-view.tsx` (app items under "From apps", app items and shadowed app skills read-only, the winner named,
`id: null` no longer filtered or keyed by id). The hook is `useAgentFile` (named so by -2; no `useAgentGuideFile` was left).
**Shape.** Per app: summary, essentials (plain-text viewer), operations file with count and a button to open it, **Skills** as one fold per skill
(name, size, description, "Script" count; open at first when the skill changed) with its files (size, "Script", marks `Added`/`Changed`/`Removed`,
`mode` → "Now a script" / "No longer a script"; a file that changed content and bit shows both), removed skills and removed files listed with
"Removed". Marks on files only for a `changed` skill (an added or removed skill carries one mark). A script counts once in the approve line even when
listed as `changed` + `mode`. Skills view: "From apps" list, each row "From the <App> app · <version>", a "Read the skill" fold with the body as plain text and
the files (Script marked); no button, switch or checkbox of its own (D-455). Shadowed app skills in "Not applied" say "Shadowed by the agent skill" /
"Shadowed by the <App> app's skill". File contents are only ever React text children (never Markdown/HTML).
**Words.** `rg -n -i "agent guide|agent-guide|agentGuide|agent_guides|What agents can read" web/src web/tests web/e2e scripts/e2e` is empty. The remaining
`recipe` hits are the credentials' injection recipe (`RecipeKind`, `buildRecipe`: how a website secret is injected), a different thing, no visible text; untouched.
**Tests.** Renamed `web/tests/agent-guide-section.test.tsx` → `agent-content-section.test.tsx` (marks, mode pair both ways, removed file, summary counts, inert HTML,
the line above Approve incl. none for no skill change and none once active) and `web/e2e/agent-guide.spec.ts` → `agent-content.spec.ts` (package page 390/1280 × both themes with
scripts, marks, line, plain text, axe; plus the agent's Skills page 390/768/1280 × both themes with the app group, read-only, shadowed winner, axe). `web/tests/workspace-skills.test.tsx`:
two new tests (app group, app-over-app winner). No production Playwright spec or `scripts/e2e` script references the changed strings or routes. None removed or loosened.
**Commands.** `pnpm typecheck` ok; eslint on the six touched files ok; `vitest run` whole web suite: 250 files, 3473 tests passed;
`playwright test e2e/agent-content.spec.ts --project=mocked`: 11 passed; `bash scripts/guards/run-all.sh`: all guards passed. Screenshots not taken (kept out of git); axe and the 390 px overflow checks run in both themes.
**Not done / honest notes.** `docs/14` and FEATURES/PROGRESS rows are not updated by this commit (Orchestrator tracking). A bug the e2e found and fixed before commit: a `changed` + `mode` file was counted as two scripts.
