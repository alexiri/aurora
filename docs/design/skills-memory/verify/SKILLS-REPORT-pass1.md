# Skills verification, pass 1 (P5-SKILLS-5)

Date: 2026-11-27. Reviewer lane, no product code changed (no direct fixes made).

## Result

**Not clean.** 27 rows: **21 Match, 4 Minor difference, 2 Needs changes.**

The list, panel, read-only states, editor, upload, per-agent switch, usage and navigation work on a real stack. The
main failure is **adding from a real library**: with the review's own repository, 6 of the 11 skills offered can't be
added, the dialog opens on one of them, and the sizes it shows are wrong. A second **broken** item sits outside the
registry, in the git library code from before this work: **editing a library's branch, tag or folder stops updates from
being proposed**, and can leave an offered "update" that would downgrade a skill.

## Environment

- Commit checked: `d2783670` (local master at the start). It holds P5-SKILLS-1..4 and FIX-276. It does **not** hold the
  FIX-277 code (that commit is FIX-277's brief; the code merged later).
- CI: the P5-SKILLS-3/-4 push (`e3bb8b54`, run 46067067858) failed only in `web`, only on
  `e2e/fix-251-template-update-journey.spec.ts` (dark, 390 px). The same spec also failed on `4a2a4174`, before
  P5-SKILLS-3, so it looks flaky. `ae473292` passed.
- Own throwaway kind cluster `aurora-mem5`, port 4530, `--llm-mock`, brought up through `heavy.sh`.
  **Still up**, for P5-MEMORY-5 pass 2. `aurora-dev` and `aurora-skm` were not touched; no volume was pruned.
- Playwright (Chromium), 1256 and 390 px, dark and light, locale en-GB. Per walked page: console and CSP, `/v1` 4xx,
  sideways overflow (the page, and scroll boxes in the page and in dialogs), axe wcag2a/aa. Some one-off interaction
  probes ran at one width or theme, some without axe.
- Data: `seed.py`. The "Anthropic skills" library was added **through the interface** from the real repository, at
  `e4bb5d9`, the mockup's commit. `internal-comms` went on Acme (Alice). **`pdf` can't be added** (see C-1), so Bob added
  `brand-guidelines` to Daneel instead; the review's own Before screens show no `pdf` on Daneel either. Alice can't add
  to Bob's agent, so Bob did that step. A second agent, **Giskard**, in Bob's workspace. Carol co-owner, Dave member.
- **Changes only in the throwaway cluster:**
  - To record real skill uses I loaded a scratch copy of the mock with three extra scenarios (a `skill_get` call, Claude
    Code's `Skill` tool, a shell read of a `SKILL.md`). Nothing in the repository changed. A redeploy of the mock
    brings back the repository's version.
  - To get a pending library update I pointed the library at an older upstream commit (`ef740771`). That showed B-1;
    I then cleared `skill_libraries.last_head_sha` **once** in the cluster's database so the refresh would compare.
    Later I set the branch back to `main`.

### Pages checked for errors

No CSP violation and no axe wcag2a/aa finding on any walked page or dialog. No page scrolled sideways. Scroll boxes that
did, at 390 px: the diffs in **Compare** and in the update review (C-5), and code blocks inside rendered instructions
(the ordinary Markdown code block; not counted). No unexpected `/v1` 4xx on the skill pages. The emergency probe got the
agent shell's 403s (`sandbox`, `answer-readiness`, `accounts`).

## Registry

Captures for rows that are not Match are in [`skills-pass1/`](skills-pass1/). Match rows were compared with captures
kept in the lane's scratch folder.

| Row | Verdict | What was checked / evidence |
|---|---|---|
| skill-1 | **Minor difference** (C-3, C-4, C-5) | One list; Add skill menu with the three entries; chips with counts (All 12 · Daneel's own 6 · From the workspace 2 · From Acme 4 · Off 1) and the Off filter; Skill · Source · For Daneel; display names; source chips; switch named "Turn off for Daneel: …"; "Daneel's version replaces the workspace's · Compare" (`DiffView`); no "Not applied", no essay; "How skills combine". Captures `skill-1-*` |
| skill-2 | Match | Panel beside the list; `?skill=` survives a reload; choosing the row again closes it; header, switch, "Use it to …", Edit, **Try it in chat** (opens Chat with "Use the “Triage alerts” skill to " in the composer), "…"; rendered instructions; file tree with code view; Executable tag (on an uploaded script); "Used 1 time this week · last in “Alert storm 26 Nov”"; "Not used yet" |
| skill-3 | Match | Workspace, organization and library skills: no inputs, no Save; lock line per tier; per-agent switch; library source and pin; "Update available — 1 file changed upstream." and **Review update** for who may approve. Menus: inherited "Duplicate as Daneel's own"; own "Remove from Daneel…"; library "Stop updates and edit" (with its explanation) and "Remove from Daneel…". S-4's wrong label is gone. App skills: not checked live |
| skill-4 | Match | Name with "Folder: …" derived; one-line "When should Daneel use it?" (an `input`) with the hint; Write/Preview; size line; Files folded, then opened with in-place edit, Close, Delete; **Add a file** with Executable (the file was added and the editor stayed open); a 5-level path is refused in words; save bar; a new skill gets its title in the list. **Upload a skill folder**: preview, `.DS_Store` left out, `#!` script marked Executable, created; a second upload says why before writing anything |
| skill-5 | **Needs changes** (C-1, C-2) | Layout, counts, search, Added tag, folded "8 can't be added · Why?", preview, footer, **Add to Daneel** / **Add to Acme** (both added and opened the new skill), no **Add a library** for a member, phone screens: all as drawn. But the split and the sizes are wrong on the real repository. Captures `skill-5-*` |
| skill-6 | **Minor difference** (C-6) | Manage Acme › Skills without any switch; Skills · Libraries; the shared list with "Used by n agents" and "Written here"/Library chips; cards with Check now, Edit and "…" (Browse skills · Pause · Delete…), Browse works, no sideways scroll; all six old addresses redirect. Captures `skill-6-*` (a card with a branch set, and one on the default branch) |
| Also 1 | Match | Administration › Skills: instance libraries and the three library settings; nothing about skills on the Defaults page |
| Also 2 | Match | Bob sees **Skills** in his workspace rail, and Alice Manage Acme › Skills, with "more controls" hidden |
| Also 3 | Match | "Turn off for …", "Remove from …", "Stop updates and edit"; no "Detach" on skill pages; display names, folder only in the editor and upload preview |
| S-1 | Match | One list |
| S-2 | Match | No glued "Use it when" on the skill pages (see further ideas for templates) |
| S-3 | Match | Side panel, in the address |
| S-4 | Match | A workspace skill's labels name the workspace |
| S-5 | Match | No fields or Save for read-only skills |
| S-6 | Match | See D-499 |
| S-7 | Match | Files folded and readable; no native file picker on the page |
| S-8 | Match | The same list on agent, workspace and organization |
| S-9 | Match | See Also 2 |
| S-10 | **Needs changes** (C-1) | Unaddable skills are still mixed among the addable ones. Captures `skill-5-*` |
| S-11 | **Minor difference** (C-7) | No table, actions visible, Browse from the card. The suggested label is still "skills". Captures `s-11-*` |
| S-12 | Match | Usage lines and Try it in chat; "Detach" gone |
| S-13 | Match | See Also 1 |
| D-499 | Match | `onboarding-new-hire` turned off for Daneel from the row: Off 2, the panel says Off. Giskard still has it. It is gone from Daneel's `~/.claude/skills` (listed in the sandbox Pod). With the workspace `meeting-minutes` off, an organization skill of that name applied. Bob, Carol and Dave (own access) 204; Alice (organization admin without it) 403 `workspace_access_required`; the agent's own skill 409 `skill_not_inherited`. App skills: not checked live |
| D-505 | Match | Real turns recorded one use each for `skill_get`, **Claude Code's own `Skill` tool** and a shell read of `SKILL.md`. This settles P5-SKILLS-1's "not verified against the pinned binaries" for Claude Code. `used_by_agents` was 2 after Daneel and Giskard. Bob and Carol get the conversation's title; Dave gets `conversation: null` and the panel says "last in a conversation" |
| SEC-158 | Match | Row in `docs/17-security.md`; its named tests exist in `skills_use.rs` and `skill_library.rs`. They pass by CI (`ae473292`); not run locally |
| Tests | Match, with a note | No `.skip`, `.fixme`, `.only` or `#[ignore]` added since `8a5b97e2`; the only test file removed is the Memory one noted in the Memory report. No skill-page test expects a retired word (the hits are channel "Detach", internet "Not applied yet" and a template diff label). **Note:** CI was not green at each merge (see the Memory report for P5-SKILLS-1's `audit-actions` run, and the `fix-251` flake above) |
| Rules | **Minor difference** (C-5) | Both themes, shared primitives, save bar, confirms, top-anchored dialogs, no page scrolling sideways at 390 px. The diff boxes do. Captures `skill-1-compare-*-390`, `rules-review-update-dark-390` |

**A conflict in my own brief, for the owner.** My registry says rows show "“Use it to …” sentences", and `skill-1.png`
draws that prefix. P5-SKILLS-3 scope 1 and D-498(c) say rows show the description **as written**, which is what is
built. By the precedence rule I counted it as Match.

## Change list

### P5-SKILLS-1 (listing data) with P5-SKILLS-4 (the dialog)

**C-1 · broken · rows skill-5, S-10 · `after/skill-5.png`**

- Differs: the browse listing judges a skill from a partial read, so skills that can't be added are listed as addable,
  with understated sizes. On `anthropics/skills` at `e4bb5d9`, listing size against what the preview computes:

  | Skill | Listed | Real |
  |---|---|---|
  | algorithmic-art, mcp-builder, pdf, slack-gif-creator, web-artifacts-builder | addable | **can't be added**: over 32,000 characters |
  | theme-factory | addable | **can't be added**: a file over 65,536 bytes |
  | brand-guidelines | 2,235 | 13,259 |
  | frontend-design | 9,390 | 19,249 |
  | internal-comms ("6 files · 1 KB" in the dialog) | 1,511 | 21,981 |
  | webapp-testing | 3,913 | 22,055 |
  | doc-coauthoring (one file) | 15,815 | 15,342 |

  So: **6 of 11 are dead ends**; the dialog **opens on one** (Algorithmic art, the first row), showing an error and a
  greyed-out Add; the fold says 8 where 14 can't be added; the check dialog's "11 skills can be added" and the card's
  "11 skills" are wrong the same way; "n files · k KB" is wrong; and `blocked: budget` uses the same estimate.
- Not new in kind (P5-SKILLS-1's report, choice 6, says the size is the listing's estimate; the invalid check on the
  listing is older), but it defeats the point of S-10 and skill-5.
- Should be: what the list calls addable can be added. The listing applies the same total, per-file and body rules as
  the add, on the real content (read what that needs, within the existing fetch caps, and cache it per commit), so the
  fold, both counts, the sizes and `budget` follow. If some skill can't be judged within the caps, it shows no size and
  is not counted as addable. Test with a repository whose `SKILL.md` is small and whose other files are over the limit,
  seen failing first. The dialog must open on a skill that can be added.
- Files: `services/aurora-api/src/domains/skill_libraries/` (catalogue and browse), `web/src/components/skills/add-from-library.tsx`.
- Captures: `skill-5-*` (what opens), `skill-5-deadend-*` (`pdf`).

**C-2 · words · row skill-5**

- Differs: the preview's reason is the raw rule text, lower case: "the body and files total more than 32000
  characters". The fold shows the same, and for `docx` a cut-off list of file paths.
- Should be (brief scope 1, "the reason in plain words"): a sentence, for example "This skill is larger than 32,000
  characters, the most a skill may hold." / "Some of its file names use characters Aurora can't store."

### P5-SKILLS-3

**C-3 · rough · row skill-1 · `after/skill-1.png`**

- Differs: row descriptions are not limited. `internal-comms` takes 9 lines and `brand-guidelines` 7 at 1256 px (rows
  about 157 and 140 px against about 62 px for a short row; mockup rows are all one height, about 67 px). The
  organization list is the same.
- Should be: at most two lines with an ellipsis; the panel keeps the full text. `web/src/components/skills/list.tsx`.
- Captures: `skill-1-*`.

**C-4 · flow · row skill-1 (also skill-3)**

- Differs: the notice's count doesn't match the rows it points at. Agent page: "**1** update … Open a skill marked
  “Update available”" while **two** rows are marked; opening the second (Internal comms, an Acme skill) shows Bob no
  update at all. Organization page: "**2** updates" while **one** row is marked (the other belongs to Daneel).
- Should be: on each page the notice counts the rows on that page that this person can review, and a row this person
  can't review doesn't invite them to open it (no chip, or a chip whose panel says who can review it).
- Captures: `skill-1-*-1256` (1 against 2), `skill-1-notice-light-1256` (2 against 1).

**C-5 · rough · rows skill-1, Rules**

- Differs: at 390 px the **Compare** diffs scroll sideways inside their boxes (+281 and +386 px), cutting the sentences
  being compared. The library **update review** does the same (+253 px) and cuts its "Show n more lines" buttons.
- Should be: both pass `DiffView`'s `wrap` (with FIX-277's hanging indent). The update review is the older SKLIB
  dialog; it is on the same screens, so fix it in the same change.
- Captures: `skill-1-compare-*-390`, `rules-review-update-dark-390`.

### P5-SKILLS-2

**C-6 · words · row skill-6 · `after/skill-6.png`**

- Differs: a library with no branch set reads "**Default branch**"; the mockup reads "Branch **main**". With a branch set
  it already does. The lane's reason (the API doesn't say the default branch's name) is a missing field, not a brief or
  decision; the check dialog already shows "main (default branch)".
- Should be: keep the resolved branch name and show "Branch main". Needs an API field.
- Captures: `skill-6-*` (second card).

**C-7 · words · row S-11 · `before/L03-library-checked.png`**

- Differs: after Check, the suggested **Label** for `github.com/anthropics/skills`, folder `skills`, is still "skills".
- Should be: a name that identifies the repository, such as "anthropics/skills".
- Captures: `s-11-*`.

## Found outside the registry

### Git skill libraries (older than P5-SKILLS-1..4; needs a new FIX)

**B-1 · broken · editing a library's source**

- What happens: `PATCH /v1/skill-libraries/{id}` with a new branch, tag, folder or address stores the new head as
  `last_head_sha` (`service.rs` `patch`). The next refresh sees an unchanged head and returns before comparing
  (`refresh.rs`, "An unchanged head costs this one call").
  1. **No updates are proposed** for the skills already added until upstream moves again. Reproduced: pointing the
     library at `ef740771`, where `brand-guidelines` and `internal-comms` differ, left 0 pending; after clearing
     `last_head_sha` by hand, the next refresh proposed both.
  2. **Stale proposals stay.** After setting the branch back to `main` (`e4bb5d9`, the pinned commit), both proposals
     from the old commit stayed, through later refreshes. Review update still offers `e4bb5d9 → ef74077`; **approving it
     would downgrade the skill** to the content of a branch the library no longer follows.
- Should be: a change of address, branch, folder (and anything else that changes what is read) makes the next refresh
  compare, and drops or replaces proposals made from the old source. Test with a patch followed by a refresh, both
  directions, seen failing first.
- Capture: `library-stale-update-dark-1256`.

### Emergency reader

This build predates the FIX-277 code, so Alice (with an open grant) was still sent from the agent's Skills page to the
workspace's manage page (Memory report C-1). **The emergency reader's Skills pages are not verified here.** I'll walk
them on the redeployed cluster in P5-MEMORY-5 pass 2.

## Regressions looked for

- Export: the dialog counts the skills; the zip holds them, with `title:` where set, a library skill's files and an
  uploaded script. Import and templates were not run.
- The agent page's other tabs and the chat's draft from Try it in chat: no errors seen.
- Update review (`DiffView`): opens with the right commits; see C-5 and B-1. I did not approve or reject anything.

## Further ideas (not counted)

- The template screens still say "**Use it when:** …" before a skill's text
  (`web/src/components/templates/new-agent/review-step.tsx`, and the admin template review's diff label), the wording
  S-2 retired on the skill pages.
- Because of the 32,000-character and per-file limits, 6 of this well-known repository's skills that pass the other
  rules, `pdf` among them, can't be added at all. The owner may want to know; the mockup assumes `pdf` can.
- A library whose "branch" is a full commit id shows all 40 characters on its card.

## Not checked on the live stack

- App skill rows and panels (no app with skills was connected to an agent).
- The emergency reader (above), a failing or paused library card, an over-budget library, import and templates.
- Adding to a workspace through the dialog (agent and organization were done).

## For pass 2

Re-check C-1..C-7 (rows skill-1, skill-5, skill-6, S-10, S-11, Rules) with their neighbours, B-1 once its FIX merges, and
the emergency reader's skill pages if P5-MEMORY-5 pass 2 has not already settled them.
