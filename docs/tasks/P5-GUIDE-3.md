# P5-GUIDE-3: Agent guides — dashboard section in the plugin view and review (owner, 2026-11-13)

Third task of "app guides on demand". Normative spec: `docs/24-app-guides.md` §9, `14`
(Instance administration → Plugins), `14 §Visual design`; contract **C-648**; SEC-117. Depends on
**P5-GUIDE-2** (the `agent_guides` field and the file route, with `schema.d.ts` regenerated).
It may start as soon as C-648 is merged, against the generated types.

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
