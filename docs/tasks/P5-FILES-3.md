# P5-FILES-3: Files viewer — text editor with conflict handling, image preview

## Goal
Opening a file on the Files page shows an editor for UTF-8 text (save with optimistic concurrency and a clear conflict dialog), a preview for raster images, and a download card for everything else — without ever rendering agent-written content as HTML or loosening the dashboard's CSP (D-316(f)(g)).

## Linked requirements
- D-316; C-905 (and C-898's `GET …/text` and `PUT` preconditions); SEC-126.
- `14` agent page **Files** entry item 6 (viewer) and the CSP bullet at the top of `14` (unchanged by this task); `18 §Files` **Web** bullet (editor parts).

## Context
- P5-FILES-2's page: `web/src/components/agents/files-tab.tsx`, `web/src/components/agents/files/**`, `web/src/lib/api/files.ts` — replace its placeholder "open file" card.
- Editor: CodeMirror 6 (`@codemirror/state`, `@codemirror/view`, `@codemirror/commands`, `@codemirror/language`, and only the language packages for the fixed set in `14`: JS/TS, JSON, Markdown, Python, Rust, TOML (legacy-modes stream parser is acceptable), YAML, HTML, CSS, shell). Pin exact versions at least two weeks old; add them in `web/package.json` + `pnpm-lock.yaml` (this lane owns them this wave). Load the editor as a lazy chunk (`next/dynamic` or `import()`), so the Files listing does not pay for it.
- CSP: CodeMirror injects `<style>` elements; pass the page's nonce through `EditorView.cspNonce` (find how the dashboard exposes its nonce to client components — the terminal's wterm styles and `components/ui/markdown.tsx` are the references). `style-src-attr 'unsafe-inline'` already exists (D-176); do not add anything to the CSP. If CodeMirror needs more, stop and report.
- Keyboard rule: Tab indents (`indentWithTab`); Esc, then Tab within 1.5 s moves focus to the next control and announces "Left the editor." — copy the terminal pane's implementation in `terminal-tab.tsx`.
- Line endings: set `EditorState.lineSeparator` from `line_ending` so CRLF files are saved as CRLF.

## Scope
### In scope
Editor per `14` item 6 (header with name, unsaved marker, Save + Ctrl/Cmd+S, Download; theme from tokens in both themes; line numbers; language by extension; read-only mode for `writable: false`); Save → `PUT` with `If-Match: "<etag>"`, on 200 keep the new etag; 409 `file_changed` → `Dialog` with Reload / Overwrite (`PUT` without `If-Match`) / Copy my text (clipboard, stay); unsaved-changes guard on in-app navigation (`ConfirmDialog`) and `beforeunload`; New file (from P5-FILES-2's toolbar) → name `FormDialog`, empty editor, first Save with `If-None-Match: *` (409 `file_exists` → inline message); `file_not_editable` reasons → the "No preview" card with Download and the reason in words; image preview for PNG/JPEG/GIF/WebP ≤ 10 MiB by magic bytes of the fetched content (`GET …/content` via `fetch`, check the first bytes, then a `data:` URL with that fixed MIME; anything else, SVG included, → "No preview"); the "Left the editor." note beside the editor.

### Out of scope
Backend changes; diff/merge views; non-UTF-8 encodings; editing files over 1 MiB.

## Constraints / file ownership
- Owns: `web/src/components/agents/files/viewer/**` (new), the open-file hook-up in `files-tab.tsx`, `web/package.json` + `pnpm-lock.yaml` (the CodeMirror packages only), `web/tests/files-viewer*.test.tsx` (new), the editor steps added to `web/e2e/files.spec.ts`.
- No CSP change; no `blob:`/new-tab navigation to agent bytes; no HTML rendering of file content anywhere.

## Acceptance criteria (failing-first)
1. vitest: dirty marker and guard; Save sends `If-Match`; the conflict dialog's three actions each send/keep what they say; CRLF preserved on save; New file uses `If-None-Match: *`; image magic-byte check (PNG/JPEG/GIF/WebP accepted, SVG and an HTML body named `.png` refused); `file_not_editable` reasons in words; read-only mode has no Save.
2. Playwright: open a `.rs` file, edit, Ctrl+S, reload shows the change; change the file through the API in between → conflict dialog → Overwrite; open an SVG and an HTML file → no rendering (text editor for the HTML text file, "No preview" for the SVG), **no CSP violation reported** in either theme; Esc-then-Tab leaves the editor; axe clean; 390 px without horizontal scroll.
3. The Files listing's JS chunk does not include CodeMirror (check the build's chunk manifest in the test or report the sizes).
4. `pnpm -C web typecheck`, `lint`, `test`, `build`; `bash scripts/guards/run-all.sh` pass.

## Verification
Per-task gates of `docs/tasks/LANE_RULES.md`; batch gate with web lint/test/build and the Playwright spec.

## Dependencies
- Blocked by: P5-FILES-2 merged (or committed earlier in the same lane).
- Unblocks: P5-FILES-4.

## Suggested agent/model and reason
`nextjs-developer` (Sonnet): a new third-party editor under a strict CSP with concurrency handling. Size M.

## Handoff notes (from the P5-FILES-2 worker, commit `cdc060bf` on `alexiri/p5-platform`)
- Hook-up points: `web/src/components/agents/files-tab.tsx` keeps `viewing: {path, size?}` (set by `open(entry)` for a file, or derived when the listing answers 422 `not_a_directory` for the URL's `&path=`) and renders `files/file-placeholder.tsx` for it — replace that component with the viewer (keep the breadcrumb above it and the `onBack` → `go(parentPath(path))` contract). "New file" (NameDialog kind `file`) currently does `putFile(agent, path, "")` (empty file, `If-None-Match: *`); switch it to open the empty editor (first Save with `If-None-Match: *`).
- `web/src/lib/api/files.ts` already has: `putFile(agent, path, body: Blob|string, {replace?, ifMatch?, onProgress?, signal?})` (XHR; `ifMatch` sends `If-Match`, no `If-None-Match`; omit both = pass `replace: true`), `filesErrorMessage` (words for `file_not_editable` reasons, `file_changed`, `file_exists`, …), `downloadUrl`, `humanSize`, `baseName`, `parentPath`, `breadcrumbs`. Add `GET …/files/text` (`FileText` is in the generated schema) there; the typed client gives `api.GET("/v1/agents/{agent}/files/text", …)`. The conflict's `details.current_etag` is on `ApiError.details`.
- Primitives you will want: `Dialog`/`ConfirmDialog` (unsaved-changes guard), `Notice`, `Badge` (Read-only), `Card` (No preview), `buttonVariants` (Download link). The terminal's Esc-then-Tab chord and "Left the editor." note live in `terminal-tab.tsx` (`ESCAPE_CHORD_MS`).
- Test conventions that bit me: vitest page tests stub `fetch` and open row menus with `fireEvent.pointerDown(trigger, {button: 0, ctrlKey: false})` + `keyDown Enter`; Playwright's project is `--project=mocked` (not chromium), run with `AURORA_E2E_WEB_PORT=4185 AURORA_E2E_WEB_PROD_PORT=4186`; `request.postData*()` is empty for file-backed bodies (`setInputFiles`), and button names need `exact: true` where a row menu is called "Actions for <name>". The mocked spec's in-memory `/workspace` (`Fs` class in `web/e2e/files.spec.ts`) is the thing to extend for `…/files/text` and `If-Match` (move it to `web/e2e/support/files.ts` when P5-FILES-3 needs it too).
- Not done yet by this lane after FILES-2: P5-FILES-3, P5-FILES-4, the batch gate (nothing of the lane's work from P5-FILES-1 on has had the full crate suites, `UPDATE_SCHEMAS=1`, OpenAPI drift, workspace clippy, `hosted.sh`, `kind.sh` yet), FIX-156 → FIX-155, P4-SSH-1 → P4-SSH-2. Disk was 77 GB free at the end of FILES-2 (prune `target/debug/deps` older than 12 h and `target/*/incremental` first).
- Process note: I used `sed -i` once for a one-line import fix in `files-tab.tsx` (against the Edit-only rule; no other file was changed that way).

## Completion report
- Files changed:
- Tests/checks:
- Remaining risks:
