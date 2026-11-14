# P5-CHAT-3: Composer — formatting, preview, files, drafts, emoji — web — **medium–large, phase 5, Next.js**

## Goal
The composer side of D-228, built to the P5-CHAT-0 boards **Formatting**, **Files** and **Phone** and the
`14` "Agent chat" composer text (normative).

## Depends on
P5-CHAT-2 (same `composer.tsx`, reply bar already in), P5-CHAT-1 (C-726 files-only), FIX-137 (renderer for
Preview; its `package.json` change merged first). Not concurrent with another lane on `web/package.json`.

## Scope
1. **Formatting row**, toggled by `Aa` (state remembered per browser, storage failures ignored): Bold,
   Italic, Strikethrough, Heading, Link, Code, Code block, Quote, Bulleted list, Numbered list, Task list,
   Table, Emoji. Buttons wrap the selection (or insert a placeholder and select it) and are undoable with the
   browser's undo (use `execCommand('insertText')` or equivalent so the textarea's undo stack keeps working —
   check it in Chromium, Firefox and WebKit). Tooltips show the shortcut.
2. **Shortcuts**: Ctrl/Cmd+B, I, Shift+X, K, E, Shift+7, Shift+8, Shift+.; Enter sends; Shift+Enter new line;
   inside an unclosed ```` ``` ```` fence Enter inserts a new line (Ctrl/Cmd+Enter sends there); Shift+Enter
   on a list item continues the list (`- `, `1. ` incremented, `- [ ] `), and on an empty item ends it.
3. **Write / Preview** switch rendering the draft with the chat renderer (FIX-137), same width as a bubble.
4. **Smart paste**: URL over a selection → `[selection](url)`; tab-separated rows (spreadsheet copy) → a
   Markdown table with the first row as header; everything else pasted as plain text (never HTML).
5. **Files**: drop zone over the whole thread pane with the overlay; clipboard paste of files and images
   (named `Pasted image <date> <time>.<ext>`); multiple files; per-file progress and failure chips as `14`
   says; Send enabled with files and no text (C-726); the existing upload API and quota errors unchanged.
6. **Drafts**: per conversation — text, reply target, uploaded-but-unsent attachment ids — in browser storage
   (try/catch; works without it), restored on return, cleared on send; a draft's attachments that were purged
   meanwhile are dropped with a notice.
7. **Emoji**: `:` + two letters opens suggestions (keyboard navigable, Enter/Tab inserts the Unicode
   character); the picker from the formatting row. Use a small, pinned emoji data set (licence and size in the
   report; lazy-loaded).
8. **Phone**: the formatting row is a horizontally scrolling strip; 44 px targets; no horizontal page scroll.
Accessibility: the formatting row is a `toolbar` with roving focus; every button has an accessible name with
its shortcut; the suggestion list is a `listbox` tied to the textarea (`aria-activedescendant`).

## Owns
`components/chat/composer.tsx` (whole file, after P5-CHAT-2), new `components/chat/composer/**`
(formatting, paste, drafts, emoji), new primitives agreed in P5-CHAT-0 under `components/ui/`,
`web/package.json` + lockfile (emoji data, if a package), tests below.

## Acceptance criteria
- [ ] Unit: every formatting action with and without a selection; each shortcut; fence-aware Enter; list
      continuation and exit; URL-over-selection; TSV → table (incl. pipes escaped in cells); drafts
      save/restore/clear with storage throwing; emoji suggestion keyboard flow.
- [ ] Playwright (`e2e/chat-composer.spec.ts`): format → Preview → send renders the same in the transcript;
      drop two files and paste an image → three chips → files-only send; reload keeps the draft; 390 px; axe
      clean in both themes; undo after a toolbar action restores the text (Chromium at least, others noted).
- [ ] Bundle-size delta and licences in the report; `pnpm typecheck`, `pnpm lint`, `pnpm test`, `pnpm build`.

## Suggested agent/model and reason
Sonnet (`nextjs-developer`); Haiku can take the emoji data wiring once the composer structure is in.
