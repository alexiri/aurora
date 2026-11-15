# P5-FILES-2: Files page — browse, upload, download, manage

## Goal
A **Files** page in the agent page's Computer section (`?tab=files`, between Terminal and Previews) where a person who can use the terminal browses the sandbox's `/workspace`, uploads files and folders (button and drag and drop), downloads, creates folders, renames and deletes — on the shared primitives, in both themes and at 390 px (D-316).

## Linked requirements
- D-316; C-904 (and the error codes of C-898..C-901 it maps to words); SEC-126 (names rendered as text; downloads only through the attachment route).
- `14` agent page **Files** entry (items 1–5 and 7, the Sections line and the status rail's Computer part), `14 §Visual design` (tokens, primitives, page patterns, both themes, no horizontal scroll at 390 px); `18 §Files` **Web** bullet (the parts that are not the editor).

## Context
- Section and tab wiring: `web/src/components/agents/agent-page.tsx` (availability object, `terminalEnabled`, the `tab ===` switch), `computer-tab.tsx`, `terminal-tab.tsx` (freeze banner, "Waking…" handling), `status-rail/parts.tsx` (Computer part: "Open computer", "Terminal").
- API client: the generated types in `web/src/lib/api/schema.d.ts` (P5-FILES-1 regenerated them) and the per-domain modules in `web/src/lib/api/*.ts` (add `files.ts`). Uploads need progress: follow the chat attachment upload (`web/src/components/chat/attachments.tsx`, `composer.tsx`) — the raw XHR/fetch pattern with `traceparent` (`14` API client bullet) and its chip states.
- Primitives: `web/src/components/ui/{data-table,menu,drop-zone,dialog,confirm-dialog,notice,badge,card,button,toolbar,icons}.tsx`; FormDialog and EmptyState per `14`. A breadcrumb and an upload tray do not exist yet: add them to `components/ui/` as shared primitives (documented in `14`'s primitives table in the same commit), not one-off markup.
- Folder upload: `<input type="file" webkitdirectory>` and `DataTransferItem.webkitGetAsEntry()` for drops; create folders with `POST …/files/ops { op: "mkdir", parents: true }` first.
- Download: a plain link to `GET /v1/agents/{agent}/files/content?path=` (same-origin, cookie session; the response is always an attachment). Never `window.open`, never a `blob:` navigation.

## Scope
### In scope
The page per `14` items 1–5 and 7: path bar with breadcrumbs and Go to path, `&path=` in the URL; toolbar (Upload menu, New folder, New file — opens P5-FILES-3's editor when it exists; until then creates an empty file with `PUT` + `If-None-Match: *` and shows it in the listing — Refresh, Show hidden with `localStorage` `aurora.files.hidden` wrapped in try/catch); listing (`DataTable`, icons, symlink targets, read-only badge, truncated notice, empty state, row `Menu` with Open/Download/Rename/Delete/Copy path; cards at 390 px); uploads (queue of 3 concurrent, tray with progress/cancel/failure chips, clash dialog Replace/Keep both/Skip with "Do this for the remaining N", folders recreated); Rename `FormDialog`; Delete `ConfirmDialog` (`danger`, count, `too_many_entries` sentence); states (waking with retries per `retry-after` up to twelve attempts, frozen banner, `files_not_supported` notice with reason, `files_unavailable` retry, every error code as a sentence); the Sections tab, availability (exactly the Terminal page's) and the status-rail "Files" link. Opening a file shows a placeholder card with Download until P5-FILES-3 lands (it replaces that card).

### Out of scope
The editor and image preview (P5-FILES-3); any backend change (report contract gaps instead).

## Constraints / file ownership
- Owns: `web/src/components/agents/files-tab.tsx` (new) and `web/src/components/agents/files/**` (new), `web/src/lib/api/files.ts` (new), `agent-page.tsx` and `status-rail/parts.tsx` (the Files wiring only), new shared primitives in `web/src/components/ui/` (breadcrumb, upload tray) and their rows in `docs/14`'s primitives table, `web/tests/files-*.test.tsx` (new), `web/e2e/files.spec.ts` (new).
- Tokens and shared primitives only; no new dependencies.

## Acceptance criteria (failing-first)
1. vitest: breadcrumbs and `&path=` round trip; Show hidden filter; clash dialog's three choices and "Do this for the remaining N"; upload queue concurrency (3), cancel, failure chip text for 413/409/403; error code → sentence table covers every code of C-898..C-901; availability (hidden for BYO, non-`can_use`, `features.terminal` off; frozen banner).
2. Playwright (`web/e2e/files.spec.ts`, against the dev stack with a hosted agent): list, upload a file and a folder by drag and drop, rename, delete a folder recursively, download (the response has `content-disposition: attachment`); axe clean in both themes; no horizontal scroll at 390 px with a 120-character file name.
3. `pnpm -C web typecheck`, `lint`, `test`, `build` pass; `bash scripts/guards/run-all.sh` passes.

## Verification
Per-task gates of `docs/tasks/LANE_RULES.md` (web: typecheck + the related vitest files); batch gate with web lint/test/build and the Playwright spec.

## Dependencies
- Blocked by: P5-FILES-1 merged (routes and generated types).
- Unblocks: P5-FILES-3, P5-FILES-4.

## Suggested agent/model and reason
`nextjs-developer` (Sonnet): multi-component page with upload queueing and new shared primitives. Size M.

## Completion report
- Files changed:
- Tests/checks:
- Remaining risks:
