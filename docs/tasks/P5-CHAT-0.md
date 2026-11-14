# P5-CHAT-0: Chat as a messaging client — design boards and the `14` spec — **medium, phase 5, Opus**

## Goal
The owner (2026-11-14): "for phase 5, we need to make the chat interface much nicer. it should be something
more like mattermost, allowing me to reply to a specific message (helps the agent understand the context
better), add formatting, attach files, etc. note there's a chat interface redesign also in the pipeline, so
sync with that." Decision D-228; contracts C-725 (quote-reply), C-726 (files-only message).

This task designs it **on the same canvas as the P5-AGENT redesign** and writes the normative `14` text, so
P5-CHAT-2 and P5-CHAT-3 build from boards and spec, as P5-AGENT-2 does.

## Inputs
- Canvas "Agent page redesign": https://claude.ai/artifact/FjwQq4Au1VbQY3dd8ejEo4 (Design type). The chosen
  boards are **Chosen · Rail expanded** (`project/Rail-Expanded.dc.html`), **Chosen · Rail collapsed** and
  **A · Phone**. Their composer is today's (Attach, textarea, Send): this task is the composer and transcript
  that go into that frame.
- Design system: the Aurora artifact (https://claude.ai/artifact/B4Wdy4cJ9K8YpGJ2Q4Yrr3), `14 §Visual design`.
  Missing primitives are added to the shared set, not drawn one-off.
- `14 §Screens` "Agent chat" (transcript, message actions, attachments, composer, queued follow-ups) and
  "Agent page (phase 5)"; D-227 (renderer); FIX-137 (what the renderer shows).

## Boards to add (new section "Chat as a messaging client (P5-CHAT)", under the chosen boards)
1. **Reply** (1280×800, rail expanded): hover toolbar on an agent message with **Reply**, Copy, More; the
   composer with a reply bar above the text field ("Replying to Daneel" + two-line excerpt + ✕); a sent person
   message with its quote strip above the bubble; a quote whose original was redacted.
2. **Formatting** (1280×800): the formatting row open (Bold, Italic, Strikethrough, Heading, Link, Code, Code
   block, Quote, Bulleted list, Numbered list, Task list, Table, Emoji) with the shortcut in each tooltip; the
   **Write / Preview** switch with the preview rendering a table, a list and code; the emoji pop-over and the
   `:shortcode` suggestion list.
3. **Files** (1280×800): the drop overlay on the thread ("Drop files to attach to this message"); chips for
   three files (one image thumbnail, one uploading with progress, one failed); a files-only message in the
   transcript.
4. **Navigation** (1280×800): scrolled up with the "Jump to latest · 3 new" pill; a message highlighted after
   opening its permalink.
5. **Phone** (390×844): reply bar, formatting row as a horizontally scrolling strip above the keyboard area,
   attach sheet (Files, Photo library, Camera — whatever the browser's file input offers), 44 px targets.
Make the boards interactive where cheap (Reply fills the reply bar; the formatting toggle; Write/Preview), as
the chosen boards are. Both themes checked.

## Spec to write (`14 §Screens` "Agent chat", marked phase 5, D-228)
- Message actions: Reply (agent messages, person messages, files the agent sent; not tool groups, approvals,
  notices, errors), Copy message, Copy link, More (Redact…). Keyboard: a focused message + `R` replies.
- Quote strip: author, two-line clamped excerpt (rendered Markdown, links inert inside it), click / Enter jumps
  to the original and highlights it for 2 s; redacted original → "Original message redacted".
- Reply bar in the composer; Esc or ✕ cancels; kept with the draft.
- Composer: formatting toggle (`Aa`) showing/hiding the row (remembered per browser); shortcuts (Ctrl/Cmd+B,
  I, Shift+X strikethrough, K link, E code, Shift+7 numbered, Shift+8 bulleted, Shift+. quote); toolbar
  buttons wrap the selection or insert a template; Write / Preview (Preview uses the chat renderer);
  Enter sends, Shift+Enter new line, **inside an open code fence Enter inserts a new line**; Shift+Enter on a
  list item continues the list; pasting a URL over selected text makes a link; pasting tab-separated cells
  makes a Markdown table; `:` + 2 letters suggests emoji; emoji picker.
- Files: drop anywhere on the thread pane, paste from the clipboard (images get a name like
  `Pasted image 2026-11-14 10.02.png`), several at once; Send enabled with files and no text (C-726).
- Drafts: text, reply target and uploaded-but-unsent attachment ids kept per conversation in browser storage
  (survive reload; cleared on send; storage failures ignored).
- Transcript: "Jump to latest" pill with the number of new messages while scrolled up; permalink
  `?c=<conversation>&m=<seq>` scrolls to and highlights the message (older history loaded as needed).
- 390 px: every control 44 px, no horizontal page scroll; axe expectations.
Add the primitives this needs to the shared set (e.g. `Toolbar`/`ToggleButton`, `EmojiPicker`,
`DropZone`) in `14 §Visual design` and the design-system artifact's component list.

## Owns
The canvas artifact (new boards and notes only; chosen boards untouched), `docs/14-web-dashboard.md`
(Agent chat, Visual design primitives), the Aurora design-system artifact if a primitive is added.

## Acceptance criteria
- [ ] Five boards on the canvas in both themes, interactive where listed; chosen boards unchanged.
- [ ] `14` text as above, consistent with C-725/C-726 and D-227; owner has looked at the boards.
- [ ] P5-CHAT-2/3 briefs updated if the boards changed anything they say.

## Suggested agent/model and reason
Opus (design and spec in one pass, as the P5-AGENT boards were); the owner reviews the boards before
P5-CHAT-2 starts.
