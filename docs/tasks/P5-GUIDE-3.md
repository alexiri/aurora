# P5-GUIDE-3: Agent guides — dashboard section in the plugin view and review (owner, 2026-11-13)

Third task of "app guides on demand". Normative spec: `docs/24-apps-for-agents.md` §9, `14`
(Instance administration → Plugins), `14 §Visual design`; contract **C-648**; SEC-117. Depends on
**P5-GUIDE-2** (the `agent_guides` field and the file route, with `schema.d.ts` regenerated).
It may start as soon as C-648 is merged, against the generated types.

**Amendment of 2026-11-15 (D-295, D-296):** C-648 carries `files[].kind` and `operations`.
Show recipes in their own group above the reference files, with a "Recipe" badge (the shared
badge primitive); show the operations file (name, size, operation count, change mark) when
`operations` is not `null`, opening in the same plain-text viewer. When P5-GUIDE-6 has not
merged yet, `operations` is always `null` and the row is simply absent.

## What it delivers

Instance admins see what a package tells agents: on the package page and, for a pending
version, inside the permissions review.

## Scope

1. `web/src/components/admin/plugin-detail-view.tsx` (and the review component it uses): an
   **Agent guide** section, one block per app of `PluginPackage.agent_guides`:
   - the app name and id, the **summary** (or "No summary: the app description is used"),
   - the **essentials** (`instructions`) in the shared preformatted block,
   - the **files** table: name, title, size, and a change badge (`Added` / `Changed` /
     `Removed`) when `changed` is not null; `summary` / `instructions` changes get the same
     badge next to their heading,
   - the sentence: *"Agents that use this app can read this text. It is written by the
     publisher."*
   - An app with no summary, no essentials and no files shows "This app ships no agent guide."
2. **Viewer**: selecting a file fetches
   `GET /v1/admin/plugins/{id}/agent-guides/{app_id}/{file}` and shows it as **plain text** in
   the shared code / preformatted primitive: no Markdown rendering, no `dangerouslySetInnerHTML`,
   no link detection. Loading, error (404, network) and empty-file states. Long lines scroll
   inside the viewer; the page never scrolls horizontally at 390 px.
3. In the **review** of a pending version the section is part of the mandatory reading: it sits
   with the other review sections and does not add a second confirmation.
4. Use tokens and the shared primitives only; if a primitive is missing (for example a
   scrollable preformatted viewer with a size cap), add it to the shared set and use it here.

## Out of scope

Any API change (report a contract problem instead of working around it). Editing guides.
A guide view for members or on agent pages. Rendering Markdown.

## Ownership

`web/src/components/admin/plugin-detail-view.tsx`, the review component, a new shared primitive
if needed, their vitest files, one Playwright spec. Not `plugins-view.tsx` (the paused P5-ASRC
lane edits the list for its Source column and badges).

## Acceptance criteria

- [ ] vitest: section renders for an app with everything, an app with nothing, and each change
      mark; a guide whose content is `<script>`, `<img onerror>`, a Markdown link and a
      `javascript:` URL is shown as inert text.
- [ ] vitest: the file fetch happens on selection only, and its error state is visible.
- [ ] Playwright (stage 4, real stack) against `fixture-guide`: open the package, see both
      files, open `api.md`, see its first line; pending-version review shows the section.
- [ ] 390 px: no horizontal page scroll with a 64 KiB file of 400-character lines; both themes;
      keyboard reachable (file list is a list of buttons, viewer region labelled).
- [ ] `tsc`, lint, the web unit suite, the production build. Report exact commands and results.

## Notes for the implementer

- Change files only with the Edit/Write tools. No `Co-Authored-By` lines in commits.
- Suggested agent: `nextjs-developer` (Sonnet). Size S–M.

## Completion report (lane `p5-apps`, worker 35, 2026-11-18)

**Adapted to master:** after P5-IA the package page is `web/src/app/admin/apps/packages/[id]/page.tsx` →
`PluginDetailView` → `PluginReview` (the review is the page; there is no separate component). The section is a
`Collapsible` (`id="guide"`, "What agents can read") under an "Agent guide" heading, between Permissions and the
action buttons; `packageHasGuide` adds `guide` to the sections that must be expanded before Approve is enabled
(`required`), so it is one more section to read, not a second confirmation. A package with no guide text has no
section. The section also shows on the page of an active or disabled package (same component).
- New: `web/src/components/admin/agent-guide-section.tsx`, the shared primitive `web/src/components/ui/text-viewer.tsx`
  (labelled, focusable, height-capped `<pre>`; text only), `useAgentGuideFile` in `web/src/lib/api/plugins.ts`
  (`parseAs: "text"`, enabled only for a selected file). `PluginAgentGuide` is optional in the generated type (the
  list and the lifecycle responses carry `[]`; see the P5-GUIDE-2 report).
- Recipes are their own group above Reference files; the operations row appears when `operations` is not null (always
  null until P5-GUIDE-6); removed files are listed apart (they have no row in `files`).

**Evidence.** `web/tests/agent-guide-section.test.tsx` (7 tests): everything / nothing / no summary, every change mark,
hostile text (`<script>`, `<img onerror>`, a Markdown link, a `javascript:` URL) inert (no element created, no link,
`window.__pwned` unset), the file fetched on selection only (zero requests before), 404 shows the error state, the empty
note, approval gated on the section. `web/e2e/agent-guide.spec.ts` (mocked API, 5 tests): the package page in light and
dark at 1280 and 390 px with a 64 160-byte file of 400-character lines scrolling inside its viewer, no horizontal page
scroll, axe (serious/critical) clean, hostile file inert; the review gated. The real-stack Playwright run against
`fixture-guide` is not done here (needs the stack): the mocked spec covers the same screens.

**Gate level: per task.** `pnpm typecheck` clean; `pnpm lint` 0 errors (6 warnings, none in these files);
`vitest run tests/agent-guide-section.test.tsx tests/plugins-admin.test.tsx tests/arbitrary-values.test.ts
tests/design-guards.test.ts` 24 passed; `playwright test e2e/agent-guide.spec.ts --project mocked` 5 passed
(`AURORA_E2E_WEB_PORT=4181 …`). Full vitest, build and the whole Playwright suite: batch gate.
