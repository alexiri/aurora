# Skills & Memory review — findings and proposals

Source of truth for the P5-SKM briefs. Each proposal has Before screenshots (`before/`, today's Aurora) and an After mockup (`after/<key>.html`, open it in a browser, `?light` for the light theme; rendered as `after/<key>.png` and `after/<key>-light.png`).

Captured on master 8a5b97e2 · throwaway kind cluster aurora-skm (never aurora-dev) · mock model · 1256 px and 390 px · 2026-11-27.

Bob Builder's agent Daneel was given a realistic working set: 12 memories (two locked, two edited twice, one with no title, a Markdown checklist and a table) and 15 skills from every source — 5 of its own (one off, one replacing a workspace skill, two with files), 3 from the workspace, 4 from the organization and 2 from a git library (Anthropic's public skills repository, added through the interface). Then every memory and skill screen was used as Bob (owner of his workspace, member of Acme) and as Alice (owner of Acme, instance admin), on a desktop and a phone.

## In short

- **Both pages are forms first, reading second.** A memory opens straight into a 140 px raw-Markdown box that scrolls inside itself; a skill opens as five input fields stacked into the middle of a 2,700 px page. Nobody can just read what Daneel knows.
- **Skills appear twice, under two names.** “This agent's skills” and “Skills this agent receives” list the same skills, once as “Summarize pr” and once as `summarize-pr`; inherited skills open at the very bottom of the page, off-screen.
- **You can't tell where anything came from.** No memory says who wrote it or when Daneel last used it; History shows `bob@example.test (user, api)`; the chat said “Used 1 tool · Deny … done” when Daneel tried to remember something.
- **What you can't change looks like what you can.** Organization and library skills render as normal, enabled-looking inputs (with a Save button on the library skill); there is no way to turn an inherited skill off for one agent.
- **The pages are hard to find and inconsistent.** Three different layouts for the same list (agent, workspace, organization); the workspace Skills page is hidden from its own owner and the organization's is a tab under Agents behind “Show more controls”.

## Memory: findings

- **M-1** (Bug) Search results show raw HTML: the snippet reads “<b>Rotation</b> changes every Monday…”; and the editor keeps showing a memory (Q4 budget) that isn't among the results. Before: `before/M06-memory-search.png`
- **M-2** (Gets in the way) A memory opens directly in edit mode: the content is a 140 px raw-Markdown textarea with its own scrollbar; headings, checklists and tables are never rendered. Before: `before/M03-memory-open.png`, `before/M04-memory-open-full.png`
- **M-3** (Gets in the way) No provenance: rows say only “Updated just now”; nothing says who wrote a memory (Bob, an API key, Daneel), in which conversation, or whether Daneel ever uses it. Before: `before/M01-memory-list.png`
- **M-4** (Gets in the way) The list shows a description (“Internal words and what they mean”) but neither the new nor the edit form has a Description field; an untitled memory shows its whole content as its title. Before: `before/M08-memory-new.png`, `before/M09-memory-untitled.png`
- **M-5** (Gets in the way) History is raw: “#2 (latest) - bob@example.test (user, api) - 1 min ago”, no diff between versions, “Remove content of #2” disabled with no reason, and it opens below the form. Before: `before/M05-memory-history.png`
- **M-6** (Gets in the way) When Daneel saves a memory, the chat shows “Used 1 tool · Deny” and “done”; no memory was saved and nothing says why or what it tried to remember. (Here the mock model's tool call was refused; the interface problem is the same either way.) Before: `before/C01-chat-memory-saved.png`
- **M-7** (Polish) Space is spent on the wrong things: a full-width “Memories in this workspace are written in English.” banner on top, and an empty right column saying “Select a memory to edit it.” Before: `before/M01-memory-list.png`
- **M-8** (Polish) Create says “Lock it” (checkbox), edit says “Locked” (switch); the lock state isn't visible in the editor header. Before: `before/M08-memory-new.png`, `before/M07-memory-locked.png`
- **M-9** (Gets in the way) On a phone the editor opens below the whole list: with 12 memories you scroll past all of them to see the one you tapped. Before: `before/P01-phone-memory.png`, `before/P02b-phone-memory-open-bottom.png`
- **M-10** (Polish) “Show more controls” adds only “Technical details · Memory key: aurora-migration” to a memory. Before: `before/M11-memory-expert.png`, `before/M12-memory-technical.png`

## Memory: proposals

### mem-1 — A calm list and a reading view

**Today:** Today a memory opens as a cramped form, the list carries no context, and half the page is a banner and an empty column.

**Proposal:** Two columns that use the width. The list gets filter chips (All · By you · By Daneel · Locked), recent-first order and a source line (“Added by you · 2 days ago”, “Learned by Daneel in “Release prep””). The right side shows the selected memory **rendered** — headings, checklists, tables — with Edit, Lock and a “…” menu, and a line saying who added it, how often it changed and when Daneel last used it. The language banner moves to workspace settings. *Designer's note:* Lock is rare, so it could live in the “…” menu rather than beside Edit; and with the Settings sub-nav on the left the reading view is narrow (about 430 px at 1256 px) — moving the sub-nav into a row above the content would give it room.

**Fixes:** M-2, M-3, M-7

**Before:** `before/M01-memory-list.png`, `before/M03-memory-open.png`

**After:** `after/mem-1.html` (live HTML, exact text and layout), `after/mem-1.png`, `after/mem-1-light.png`

### mem-2 — Editing that fits the content

**Today:** Editing happens in a 140 px box, the description can't be edited, and create and edit use different lock controls.

**Proposal:** Edit mode keeps the same column: Title, a one-line **Description** (“when should Daneel use this?”), a tall editor with Write / Preview, the size as quiet text, one **Locked** switch for both create and edit, and the shared save bar. The memory's key (`aurora-migration`) sits under the title as a small chip with a copy button, in reading and edit mode, so nobody needs “Show more controls” to find it. A new memory has a **Key** field under the title: filled in from the title as you type (“Where the runbooks live” → `where-the-runbooks-live`), editable until the first save, fixed after.

**Fixes:** M-2, M-4, M-8, M-10

**Before:** `before/M04-memory-open-full.png`, `before/M08-memory-new.png`

**After:** `after/mem-2.html` (live HTML, exact text and layout), `after/mem-2.png`, `after/mem-2-light.png`

### mem-3 — Search that shows the match

**Today:** Search snippets show raw `<b>` tags and the editor keeps an unrelated memory open.

**Proposal:** Matches are highlighted in the snippet; a count and Clear sit above the results; the first match opens on the right, scrolled to the highlighted line.

**Fixes:** M-1

**Before:** `before/M06-memory-search.png`

**After:** `after/mem-3.html` (live HTML, exact text and layout), `after/mem-3.png`, `after/mem-3-light.png`

### mem-4 — History you can read

**Today:** History lists e-mails and raw sources, has no diff, and opens below the form.

**Proposal:** History replaces the reading view: a timeline with names and context (“Daneel changed it · yesterday in “Release prep””), a **diff** of the selected version against the one before, and Restore this version. The diff is the shared `DiffView` (`web/src/components/ui/diff-view.tsx`, built by FIX-250 for the template review and already used by the skill library update review), not a new component: same line numbers, folded unchanged lines and word highlights everywhere.

**Fixes:** M-5

**Before:** `before/M05-memory-history.png`

**After:** `after/mem-4.html` (live HTML, exact text and layout), `after/mem-4.png`, `after/mem-4-light.png`

### mem-5 — Say in the chat what was remembered

**Today:** When Daneel saves a memory the transcript says “Used 1 tool · Deny” and “done”.

**Proposal:** A small card in the transcript: “Daneel remembered · Grafana service account · needs read access to the prod folder” with View and Undo — and when saving fails, the reason and “Save it yourself”.

**Fixes:** M-6

**Before:** `before/C01-chat-memory-saved.png`

**After:** `after/mem-5.html` (live HTML, exact text and layout), `after/mem-5.png`, `after/mem-5-light.png`

### mem-6 — Memory on a phone

**Today:** On a phone the selected memory appears below the whole list.

**Proposal:** The list and the memory are two screens: tap a row, read it on its own screen with a back arrow and Edit.

**Fixes:** M-9

**Before:** `before/P01-phone-memory.png`, `before/P02b-phone-memory-open-bottom.png`

**After:** `after/mem-6.html` (live HTML, exact text and layout), `after/mem-6.png`, `after/mem-6-light.png`


## Skills: findings

- **S-1** (Gets in the way) Two lists of the same skills: “This agent's skills” (title case, “Summarize pr”) and “Skills this agent receives” (slugs, `summarize-pr`), with a precedence paragraph between them and a “Not applied” list below. Before: `before/S01-skills-agent.png`, `before/S03-skills-default.png`
- **S-2** (Gets in the way) Every description reads wrong: “Use it when Draft a short email in Bob's tone.” — the label is glued to a sentence written as a statement. Before: `before/S01-skills-agent.png`, `before/S07-ws-skills.png`
- **S-3** (Gets in the way) Opening a skill puts its editor inline between the two lists (the page grows to 2,700 px); opening an inherited one puts it at the very bottom, off-screen, with no scroll to it. Before: `before/S02-skill-open.png`, `before/S06-ws-skill-from-agent.png`
- **S-4** (Bug) A workspace skill's switch says “On for the organization's agents” (the label of the organization tier). Before: `before/S06-ws-skill-from-agent.png`
- **S-5** (Gets in the way) Read-only skills look editable: Acme's skill opens as normal-looking inputs and a switch (all disabled, no explanation); a library skill even shows a Save button. Before: `before/S05-org-skill-from-agent.png`, `before/L10-library-skill-open.png`
- **S-6** (Gets in the way) No way to turn an inherited skill off for one agent: Acme's “Onboarding new hire” applies to Daneel whether Bob wants it or not. Before: `before/S01-skills-agent.png`
- **S-7** (Gets in the way) Files: the add-file form is always open with the browser's “Choose File / No file chosen”; existing files can only be replaced or deleted, never viewed; “Replace as executable” is a link. Before: `before/S02-skill-open.png`
- **S-8** (Gets in the way) Three layouts for one list: stacked with the editor inline (agent), list beside an empty “Select a skill to edit it.” column (workspace), stacked under tabs (organization). Before: `before/S01-skills-agent.png`, `before/S07-ws-skills.png`, `before/S10-org-skills.png`
- **S-9** (Gets in the way) Hard to find: the workspace Skills page isn't in its own owner's rail until “Show more controls”; the organization's skills are a tab under Manage Acme › Agents, also hidden by default, even for the owner. Before: `before/S09-org-agents-default.png`, `before/S10-org-skills.png`
- **S-10** (Gets in the way) The library picker lists 8 skills that can't be added (in red, some “about 0 characters”) among the 11 that can; it offers no preview of a skill's instructions before adding. Before: `before/L07-add-from-library.png`
- **S-11** (Polish) The libraries table scrolls sideways at 1256 px: status and actions (Check now, Edit, Pause, Delete) are off-screen; there is no way to browse a library's skills from its row; the suggested label is just “skills”. Before: `before/L04-library-added.png`, `before/L03-library-checked.png`
- **S-12** (Polish) Nothing says whether a skill is ever used, or lets you try it; “Detach”, “Remove” and “Delete” mean three different things on the same pages. Before: `before/L10-library-skill-open.png`, `before/S02-skill-open.png`
- **S-13** (Polish) Administration's skill settings (libraries, refresh interval, size cap) sit at the bottom of a 3,100 px “Defaults” page. Before: `before/A02-admin-agents-skills.png`

## Skills: proposals

### skill-1 — One list of everything Daneel can do

**Today:** Two lists of the same skills, two spellings of every name, a precedence essay, and no way to switch off an inherited skill.

**Proposal:** One list with search and chips (All · Daneel's own · From the workspace · From Acme · Off). Each row: the display name, a “Use it to …” sentence, a source chip and an **On/Off switch for this agent** — inherited skills included. Overrides show in place (“Daneel's version replaces the workspace's · Compare”). **Add skill** offers Write a new skill or Add from a library. *Note:* turning an inherited skill off for one agent needs a per-agent setting the data model doesn't have yet; until then the switch would change it for everyone, so it must not be offered.

**Fixes:** S-1, S-2, S-6

**Before:** `before/S01-skills-agent.png`, `before/S03-skills-default.png`

**After:** `after/skill-1.html` (live HTML, exact text and layout), `after/skill-1.png`, `after/skill-1-light.png`

### skill-2 — A skill opens in a side panel, readable first

**Today:** A skill opens as a form in the middle of a 2,700 px page, inherited skills at the very bottom.

**Proposal:** The list stays put; the skill opens in a right-hand panel: name, source and switch, its “Use it to …” line, the instructions **rendered**, its files as a small tree you can open, how often it was used, and Edit, Try it in chat and a “…” menu.

**Fixes:** S-3, S-12

**Before:** `before/S02-skill-open.png`, `before/S06-ws-skill-from-agent.png`

**After:** `after/skill-2.html` (live HTML, exact text and layout), `after/skill-2.png`, `after/skill-2-light.png`

### skill-3 — Read-only skills say so

**Today:** Organization and library skills look like editable forms; one even has a Save button; a workspace skill is labelled as an organization one.

**Proposal:** No fields at all for what you can't change: a lock line (“Managed by Acme's admins. You can turn it off for Daneel.”) with the per-agent switch; a library skill says where it comes from, which commit it is pinned to and whether an update is waiting.

**Fixes:** S-4, S-5

**Before:** `before/S05-org-skill-from-agent.png`, `before/L10-library-skill-open.png`

**After:** `after/skill-3.html` (live HTML, exact text and layout), `after/skill-3.png`, `after/skill-3-light.png`

### skill-4 — Writing a skill, files included

**Today:** Files sit behind an always-open upload form with the browser's file picker and can't be viewed.

**Proposal:** The editor keeps the panel: Name (the folder name derived and shown small), “When should Daneel use it?”, Instructions with Write / Preview, and Files folded to one line (“Files · 2 — SKILL.md, template.md” with **Add file**) so they don't crowd the editor; opened, each file can be read and edited in place, and Add file is a small dialog (name, paste or upload, Executable).

**Fixes:** S-7

**Before:** `before/S04-skill-new.png`, `before/S02-skill-open.png`

**After:** `after/skill-4.html` (live HTML, exact text and layout), `after/skill-4.png`, `after/skill-4-light.png`

### skill-5 — Adding from a library

**Today:** The picker mixes 8 unavailable skills into the list and shows nothing of a skill before adding it.

**Proposal:** Libraries on the left, addable skills on the right with search, the unavailable ones folded into “8 can't be added · Why?”, and a preview of the selected skill (rendered instructions, files) with **Add to Daneel**.

**Fixes:** S-10

**Before:** `before/L07-add-from-library.png`

**After:** `after/skill-5.html` (live HTML, exact text and layout), `after/skill-5.png`, `after/skill-5-light.png`

### skill-6 — Skills have a home at each level

**Today:** Organization skills are a hidden tab under Agents; libraries are a table that scrolls sideways; the workspace page uses another layout.

**Proposal:** Manage Acme gets its own **Skills** entry with tabs Skills · Libraries; the Skills tab is the same list as the agent's (with “Used by 14 agents”), libraries become cards with their state and actions visible. The workspace page uses the same list too.

**Fixes:** S-8, S-9, S-11

**Before:** `before/S10-org-skills.png`, `before/L04-library-added.png`, `before/S07-ws-skills.png`

**After:** `after/skill-6.html` (live HTML, exact text and layout), `after/skill-6.png`, `after/skill-6-light.png`

## Also

- **Move skill settings to a Skills section in Administration.** The library switch, refresh interval and size cap live at the bottom of “What agents may do › Defaults”. Give them their own place next to the instance's libraries. Before: `before/A02-admin-agents-skills.png`, `before/A01-admin-skill-libraries.png`
- **Decide who sees Skills by default.** A workspace owner who writes skills should see the workspace Skills page without “Show more controls”, and an organization owner the organization's — keep the switch for Tests and other expert pages. Before: `before/S09-org-agents-default.png`
- **Use one set of words.** Turn off / Remove from this agent / Delete everywhere; never Detach next to Remove; show skill names as their display name, the folder name only in the editor. Before: `before/L10-library-skill-open.png`

## All screens

Every screen captured for the review is in `before/` (1256 px wide desktop, 390 px phone):

- `before/A01-admin-skill-libraries.png`
- `before/A02-admin-agents-skills.png`
- `before/C01-chat-memory-saved.png`
- `before/L01-libraries-empty.png`
- `before/L02-add-library-dialog.png`
- `before/L03-library-checked.png`
- `before/L04-library-added.png`
- `before/L05-library-row-scrolled.png`
- `before/L06-org-new-skill.png`
- `before/L07-add-from-library.png`
- `before/L08-library-skill-preview.png`
- `before/L09-org-skills-with-library.png`
- `before/L10-library-skill-open.png`
- `before/L11-light-memory.png`
- `before/M01-memory-list.png`
- `before/M02-memory-list-full.png`
- `before/M03-memory-open.png`
- `before/M04-memory-open-full.png`
- `before/M05-memory-history.png`
- `before/M06-memory-search.png`
- `before/M07-memory-locked.png`
- `before/M08-memory-new.png`
- `before/M09-memory-untitled.png`
- `before/M10-memory-delete.png`
- `before/M11-memory-expert.png`
- `before/M12-memory-technical.png`
- `before/P01-phone-memory.png`
- `before/P02-phone-memory-open.png`
- `before/P02b-phone-memory-open-bottom.png`
- `before/P03-phone-skills.png`
- `before/S01-skills-agent.png`
- `before/S02-skill-open.png`
- `before/S03-skills-default.png`
- `before/S04-skill-new.png`
- `before/S05-org-skill-from-agent.png`
- `before/S06-ws-skill-from-agent.png`
- `before/S07-ws-skills.png`
- `before/S08-agent-skills-after-library.png`
- `before/S09-agent-skills-full.png`
- `before/S09-org-agents-default.png`
- `before/S10-org-skills.png`
