# Skills & Memory review — the design package (read this first)

The owner accepted every proposal of the Skills & Memory review on 2026-11-27 (**D-496**: "everything in your proposals in
the artifacts looks great"). This folder is the complete, offline copy of that review. **It is the visual and textual
target of the P5-MEMORY-1..5 and P5-SKILLS-1..5 briefs.** Nothing here needs a network or a claude.ai login: the owner's
artifact (<https://claude.ai/artifact/7JhM1jESJLZmBR8G7r41ZW>, private) holds the same content, but workers cannot open
it and do not need to.

## What is here

| Path | What it is | How to use it |
|---|---|---|
| `review.md` | All findings (M-1..M-10, S-1..S-13), every proposal's "Today" and "Proposal" text, the "Also" items, the list of screens | Read the part your brief names, in full, before coding |
| `after/<key>.png` | The **After** mockup, dark theme, 1200 px wide (1x) | **Look at it** with your image reader (the Read tool shows PNGs) before you write code and again before you hand over |
| `after/<key>-light.png` | The same mockup, light theme | Check both themes |
| `after/<key>.html` | The mockup as live HTML (uses `../kit.css`) | Open in a browser (Playwright `page.goto("file://…/after/<key>.html")`, add `?light` for the light theme) to measure spacing or read **exact text**; or read the markup for the words and structure |
| `before/<shot>.png` | Today's Aurora (1256 px desktop, 390 px phone), as captured for the review | What is being replaced; the findings point at these |
| `kit.css` | The mockup kit (every rule scoped under `.mock`) | Only to render the HTML; **never copy it into the app**: build with the app's tokens and shared primitives (`docs/14-web-dashboard.md` §Visual design) |
| `seed.py` | The script that created the review's data through the API | Verifier briefs and anyone reproducing the screens use it (below) |

Keys: `mem-1` … `mem-6` (Memory) and `skill-1` … `skill-6` (Skills). Each brief lists its keys, findings and files.

| Key | Proposal | Brief(s) |
|---|---|---|
| mem-1 | A calm list and a reading view | P5-MEMORY-2 (backend fields: P5-MEMORY-1) |
| mem-2 | Editing that fits the content (key chip, Key on create) | P5-MEMORY-2 |
| mem-3 | Search that shows the match | P5-MEMORY-2 (highlights: P5-MEMORY-1) |
| mem-4 | History you can read | P5-MEMORY-3 (backend: P5-MEMORY-1) |
| mem-5 | Say in the chat what was remembered | P5-MEMORY-4 (events: P5-MEMORY-1) |
| mem-6 | Memory on a phone | P5-MEMORY-2 |
| skill-1 | One list of everything the agent can do | P5-SKILLS-3 (switch, names, usage: P5-SKILLS-1) |
| skill-2 | A skill opens in a side panel, readable first | P5-SKILLS-3 |
| skill-3 | Read-only skills say so | P5-SKILLS-3 |
| skill-4 | Writing a skill, files included (files folded) | P5-SKILLS-3 |
| skill-5 | Adding from a library | P5-SKILLS-4 (blocked reasons, budget: P5-SKILLS-1) |
| skill-6 | Skills have a home at each level | P5-SKILLS-2 (list: P5-SKILLS-3) |
| Also 1–3 | Administration › Skills; who sees Skills by default; one set of words | P5-SKILLS-2, P5-SKILLS-3 |

## How to read a mockup (binding for every brief of this set)

1. **Precedence (D-496):** your brief → the owner's decisions (D-496..D-499, D-505, D-506) → the After mockup → the proposal text in
   `review.md`. A difference the brief or a decision requires is correct; anything else that differs from the mockup is
   a defect the verifier (P5-MEMORY-5, P5-SKILLS-5) sends back.
2. **In scope is the content area** the proposal is about: the list, the panel, the editor, the dialog, the card, and
   the navigation entries the brief names. **The app shell is illustrative:** the left rail's entries, the agent
   header (`@daneel`, the status), and the Settings sub-nav entries (Tests, Other agents) differ between mockups
   because they were drawn with "Show more controls" on or off. Keep the real shell; change navigation only where a
   brief says so (P5-SKILLS-2).
3. **Sample data is illustrative** (names, dates, counts, "2 days ago"). Words that are **interface** (labels, buttons,
   headings, hints, empty states, the source lines' patterns such as "Added by you · 2 days ago") are exact: use them
   as written, with the agent's and organization's real names in place of Daneel and Acme.
4. **Yellow notes** (`.ann`, a yellow sticky label) are commentary for the reader, not UI.
5. **Use the app's shared primitives and tokens**, not the kit's classes: `SaveBar`, `Switch`/`SwitchField`, `Menu`
   ("…"), `Badge`, `ChoiceChips`, `Markdown`, `DiffView`, `HighlightedText`, `CardGrid`, `Dialog`, `useCopy`… If a
   pattern in a mockup has no primitive yet (the Write/Preview editor, the phone sub-screen), the brief says who adds
   it to the shared set (`web/src/components/ui/`) — add it once, there, and use it everywhere.
6. **Both themes, 1256 px and 390 px, no horizontal scroll at 390 px** (`14` §Visual design). Mockups show desktop
   unless a phone frame is drawn; at 390 px follow the page patterns of `14` (stacked, sub-screens, tables as cards).
7. **Compare before you hand over:** capture your screen at 1256 px in dark and light (and 390 px where the mockup has a
   phone, or the page is a list+detail), put it beside `after/<key>.png` / `-light.png`, and list every remaining
   difference in the completion report with the reason (a brief or decision that requires it). The verifier does the
   same comparison and sends back what you did not explain.

## Reproducing the review's data

On a **throwaway** kind cluster (never `aurora-dev`, `localhost:4000` or its `aurora` database):
`AURORA_DEV_CLUSTER=<name> AURORA_DEV_PORT=<free port> deploy/dev/kind-dev.sh up --llm-mock`, then
`kind-dev.sh bootstrap-admin alice@example.test`, sign in Bob Builder (owner of his personal workspace, member of the
organization) through the development sign-in, create the agent **Daneel**, make an API key for Bob and an organization
key for Alice, and run

    python3 -I docs/design/skills-memory/seed.py http://localhost:<port> <bob_key> <alice_org_key> <agent_id> <workspace_id> <org_id>

It creates 12 memories (two locked, two changed twice, one untitled, a Markdown checklist and a table) and skills at
the agent (5, two with files, one off), workspace (3) and organization (3) levels. Then, through the interface as Alice,
add the library "Anthropic skills" (`https://github.com/anthropics/skills`, folder `skills`), add **internal-comms** to
the organization and **pdf** to Daneel. Tear the cluster down when done (`kind-dev.sh down` with the same variables).
