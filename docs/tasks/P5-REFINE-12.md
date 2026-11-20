# P5-REFINE-12: Agent settings: instructions, memory, skills, tests, new agent and templates, Other agents, the missing AI key (owner, 2026-11-20)

## Goal

UX review 5 walked everything under an agent's Settings (what used to be Behaviour and Access) and found forms that ask
for things Aurora could decide, words from the engine and a dead end:

- **A memory needs a Key** (an immutable slug), a Title, a Description and "Content (Markdown)"; the list shows the key
  twice and never the title (behaviour-1).
- **A skill shows `SKILL.md`**, file paths, an Executable checkbox and a native "Choose File" for everyone (behaviour-2).
- **The eval "What is 6 times 7?" fails** with "The reply did not end with an ANSWER: line" and blames the agent; the judge
  is saved as "Ready" and times out on the first question; the counters stay at 0 while rows already show Failed
  (behaviour-3, behaviour-4).
- **Advanced** shows "Harness: Claude Code", "Large (opus)", "Use the endpoint's setting (none)" and `scn-pong (litellm)`
  (behaviour-5).
- **Four save patterns** and the avatar saves silently (behaviour-6); **"Evals", "Judge key", "Checked as", "Refused
  writes", "Configuration: 2cb53c0856f4…", "Run of Just now"** (behaviour-7); the avatar is a plain circle with an 11 px
  "Remove" (behaviour-8).
- **Peers is a dead end** with no button (channels-1).
- **New agent defaults to "Bring your own (BYO)"**, which the page says only owners can create; "Hosted" is unexplained;
  the field is "Brief" (channels-2); creating an agent closes the dialog and opens nothing (channels-4).
- **A second agent can't answer** because it got no AI key, and the fix is hidden in a collapsed "Not given to Rex (1)"
  (access-3, priority 10).

The owner's decisions for this brief: **D-413** purpose templates live in the **web bundle**
(`web/src/lib/onboarding/templates.ts`), no API, not org-editable; **D-411** people create agent links (the "Link an
agent" picker); **D-414** starter keys apply to every new hosted agent and agents created before a key get it when it is
added; **D-406** altitudes (Use / Set up / Expert); the renames of **D-407**.

## Linked requirements

- **Artifact (source of truth, owner's comments applied):** <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt>
  - `#f-behaviour-1` … `#f-behaviour-8`, `#f-channels-1`, `#f-channels-2`, `#f-channels-4`, `#f-access-3` (the Access card
    and header button); recommendation 5 "No blank forms" (<https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt#above>,
    `UXR5/layer-above.md` rec 5); rec 1 (altitudes), rec 12 (Peers moves to Settings › Other agents); priority 2 (one save
    pattern) and priority 10 (a newcomer's agent should just work), `UXR5/priorities-themes.md`.
- **Local reference folder (read-only, workers cannot open the artifact):**
  `/home/alex/orca/workspaces/aurora/design-ux-review-5/`. Write it as `UXR5/` from here on. `UXR5/findings.md` is the
  authoritative text; yellow dashed notes in a mockup are commentary.
- **Per finding** (After dark `UXR5/after/<key>.png`, light `UXR5/after/<key>-lt.png`, markup `UXR5/after/src/<key>.html`,
  Before `UXR5/before/<key>-b<n>.png`, screens `UXR5/screens/<id>.png`):
  - **behaviour-1** memory: After `UXR5/after/behaviour-1.png`, `-lt`, `src/behaviour-1.html`; Before
    `UXR5/before/behaviour-1-b1.png`; screens `B060-memory-new.png`, `B062-memory-bad-key.png`, `B063-memory-saved.png`.
  - **behaviour-2** skill: After `behaviour-2.png`, `-lt`, `src/behaviour-2.html`; Before `behaviour-2-b1.png`; screens
    `B070-skill-new.png`, `B073-skill-created.png`, `B075-skill-page-full.png`.
  - **behaviour-3** the ANSWER line: After `behaviour-3.png`, `-lt`, `src/behaviour-3.html`; Before `behaviour-3-b1.png`;
    screens `B079-eval-new-question.png`, `B088-run-result.png`.
  - **behaviour-4** judge and live run: After `behaviour-4.png`, `-lt`, `src/behaviour-4.html`; Before `behaviour-4-b1.png`,
    `behaviour-4-b2.png`; screens `B085-judge-saved.png`, `B087-run-progress.png`, `B089-run-final.png`.
  - **behaviour-5** Advanced: After `behaviour-5.png`, `-lt`, `src/behaviour-5.html`; Before `behaviour-5-b1.png`; screen
    `B055-advanced-open.png`.
  - **behaviour-6** one save pattern: After `behaviour-6.png`, `-lt`, `src/behaviour-6.html`; Before `behaviour-6-b1.png`,
    `behaviour-6-b2.png`; screens `B054-instructions-saved.png`, `B056-avatar-upload.png`, `B085-judge-saved.png`.
  - **behaviour-7** Tests wording: After `behaviour-7.png`, `-lt`, `src/behaviour-7.html`; Before `behaviour-7-b1.png`,
    `behaviour-7-b2.png`; screens `B083-evals-page.png`, `B089-run-final.png`.
  - **behaviour-8** picture: After `behaviour-8.png`, `-lt`, `src/behaviour-8.html`; Before `behaviour-8-b1.png`; screen
    `B056-avatar-upload.png`.
  - **channels-1** Link another agent: After `channels-1.png`, `-lt`, `src/channels-1.html`; Before `channels-1-b1.png`,
    `channels-1-b2.png`; screens `C035-peers.png`, `C039-rex-peers.png`, `C040-agent-actions-menu.png`.
  - **channels-2** New agent: After `channels-2.png`, `-lt`, `src/channels-2.html`; Before `channels-2-b1.png`,
    `channels-2-b2.png`; screens `C036-new-agent.png`, `C002-agents.png`.
  - **channels-4** after Create: After `channels-4.png`, `-lt`, `src/channels-4.html`; Before `channels-4-b1.png`; screen
    `C038-agent-created.png`.
  - **access-3** the missing key: After `access-3.png`, `-lt`, `src/access-3.html`; Before `access-3-b1.png`, `access-3-b2.png`;
    screens `C041-rex-chat-cant-answer.png`, `C044-rex-agent-access.png`, `C045-rex-access-expanded.png`, `A111-org-apps.png`.
  - **Rec 5** (the table of defaults and the template description): `UXR5/layer-above.md` rec 5; screens
    `C036-new-agent.png`, `B060-memory-new.png`, `B070-skill-new.png`, `B079-eval-new-question.png`.
- **Decisions:** D-413, D-411, D-414, D-406, D-407, D-405 (precedence: this brief, then the owner's decisions, then the
  mockup, then the fix text). Existing: D-385 (model on Instructions), D-289..D-294 and D-300 (evals).
- **Contracts used (read the merged text):** P5-REFINE-10's **C-1085** (`linkable`, create/delete/direction of links),
  P5-REFINE-5's starter key (C-1061..C-1066), P5-REFINE-8's reason on an ask (**C-1081**), C-1035 (`?draft=`),
  C-864/C-872 (evals). **Small Rust in this brief**, three **amendments to shipped contracts, no new number** (scope 9).
- **Normative docs:** `docs/14-web-dashboard.md` § Agent page (phase 5), § Agents, accounts and access §8; `docs/10 §Evals`.

## Wave and order

- **Wave 2, lane C, after P5-REFINE-3** (the altitude primitive) and after **P5-REFINE-2** (the save bar, `SecretOnce`,
  `ConfirmDialog`, `labels.ts`, `TechnicalDetails`) and **P5-REFINE-11** (the Settings shell, sub-nav and the header's
  action slot; **11 merges first**). Lane C: 3 → **12** → 16.
- **Items that wait for a backend brief** (do the rest first and record the order in the completion report):

  | Item | Needs |
  |---|---|
  | 1. New agent dialog and templates, 2. Instructions, 3. Memory, 4. Skills, 5. Tests UI (words, layout) | nothing |
  | 5b. Tests: live counters, per-row Re-run, judge tested on save | this brief's own small Rust (scope 9) |
  | 6. Other agents with "Link an agent" | **P5-REFINE-10** |
  | 7. The missing-key button and Access card | **P5-REFINE-5** |
  | 8. After Create → the new agent's chat with the template's first message | nothing (C-1035 exists) |

## Context

All paths under `web/src/` unless stated; line numbers for master c0fe7d10.

- **Settings pages and shell:** `components/agents/agent-page.tsx` (page bodies :268–320; P5-REFINE-11 reworks the header,
  the Settings button and the sub-nav), `sections.ts` (P5-REFINE-11).
- **Instructions:** `components/agents/instructions-tab.tsx` (353 lines; the card :150–215: `AvatarField` :158, Name :160,
  Instructions :167, Model :172, "may message you first" :206, the collapsed `<summary>Advanced: model roles, effort, agent
  program</summary>` :217 with Harness :221, roles via `model-roles-fields.tsx`, Effort :275, the dark "Unsaved changes" bar
  :292 with Discard and Save); `avatar-field.tsx` (compact :46–80 with the 512 KiB hint :64 and :126, Remove link :73–80);
  `model-roles-fields.tsx` (labels "Large/Medium/Small", "Subagents use", `lib/model-roles.ts:32`).
- **Memory:** `components/memory/memory-tab.tsx` (`MemoryTab` :38, the form :145–190 with Key :174, Title :177,
  Description :180, Content (Markdown) :183, `SizeMeter` :186, key rule :136 `MEMORY_KEY_RE`), `lib/api/memory.ts`
  (`MEMORY_KEY_MAX = 64`, `MEMORY_KEY_RE = /^[a-z0-9]+(-[a-z0-9]+)*$/`, `MEMORY_CONTENT_MAX = 100_000`). API
  `POST /v1/agents/{agent}/memories { key, title?, description?, content }` (`services/aurora-api/src/domains/memory/dto.rs:99`);
  the index line is the description, else the title, else the first content line (`memory/service.rs:96`), so **no
  description is needed**.
- **Skills:** `components/skills/skills-tab.tsx` and `skills-view.tsx` (417 lines: `SkillsEditor` :50 shared by agent,
  workspace and organization scopes, `CreateSkill` :126 with Name :149 (immutable slug), Description :152, `SKILL.md` :155,
  `SkillEditor` :198, `SkillFiles` :262 with the file path, upload, text and the **Executable** checkbox :311,
  `EffectiveSkillsView` :357 with the precedence paragraph :378). API `POST /v1/agents/{agent}/skills { name (kebab slug,
  ≤64, `aurora-*` reserved), description (one line ≤500), body, enabled? }` (`domains/skills/dto.rs:158`).
- **Evals (Tests):** `components/agents/evals/` (`evals-tab.tsx`, `questions-section.tsx` ("Checked as" :86),
  `question-dialog.tsx` (`ANSWER_HELP` :20/:138), `judge-section.tsx` (`Judge key` :61, `JUDGE_OWNER_ONLY` :18, the save
  :47), `runs-section.tsx`, `run-detail.tsx` ("Run of …" :267, "Refused writes" :93/:305, "Configuration:" :319),
  `run-state.ts` (`progressRatio` :27), `copy.ts` (reason words :25), `lib/api/evals.ts` (`useEvalRuns` :86,
  `useEvalRun` :104 with a 5 s refetch while active)). Backend: `services/aurora-api/src/domains/evals/`
  (`scoring.rs:10–21` **already appends the ANSWER instruction** to numeric and text questions, C-862; `service.rs:450`
  `put_settings` runs `judge_check` which only checks grant and usability, **not a model call**; `dto.rs:167` `EvalRun.counts`;
  `crates/aurora-db/src/repo/evals.rs:540` writes `questions_passed/failed/errored/not_scored` **only when the run
  finishes**, which is why the counters stay 0 while rows fail; `dto.rs:330` `StartEvalRun {}` has no fields).
- **Other agents:** `components/peers/peers-tab.tsx` (118 lines: `LinkRow` :16 with "Their policy: ask" and "My policy for
  X" select, `Links`, `Tasks` with `Badge` status, `PeersTab` :98 with one empty state), `lib/api/peers.ts` (+ the hooks
  from P5-REFINE-10).
- **New agent:** `components/agents/create-agent-dialog.tsx` (134 lines: `kind` state defaults to `"byo"` :33, Name :96,
  Handle :99, Kind select :107 ("Bring your own (BYO)", "Hosted"), `byoBlocked` :21–29, Brief :123, `FormDialog`,
  `AgentRevealDialog`). The page that opens it: `components/agents/agents-view.tsx`.
- **Access card and header action:** `components/agents/access-tab.tsx` (77 lines), `agent-summary.tsx`,
  `components/accounts/accounts-list.tsx` (the collapsed "Not given to {agent}" :252), `lib/api/credential-grants.ts`
  (`useAddCredentialGrants` :87, `useModelReadiness` :38), `components/agents/agent-state.tsx` (P5-REFINE-11 adds an
  `action` slot).
- **Templates file:** none yet: new `web/src/lib/onboarding/templates.ts` (D-413). Neighbours: `lib/onboarding/guide.ts`
  (`SAY_HELLO` text at :123).
- **Existing tests:** `web/tests/agent-*.test.tsx`, `memory*.test.tsx`, `skills*.test.tsx`, `evals*.test.tsx`,
  `peers*.test.tsx`; Playwright `web/e2e/agent-config.spec.ts`, `agent-evals.spec.ts`, `agent-access.spec.ts`.

## Scope

Words are the mockup's. "Matches" = both themes at 1256 px and 390 px.

1. **New agent (channels-2, channels-4, rec 5, D-413).** Mockup `UXR5/after/channels-2.png`, `-lt`, `src/channels-2.html`;
   Before `UXR5/before/channels-2-b1.png`, `-b2.png`.
   - The dialog "New agent": **Name** ("Rex"); **Handle (optional)** (filled in from the name as the person types, "Cannot be
     changed later."); **Where it runs**: **"Runs on Aurora (recommended)"** with "Aurora hosts the agent and its computer
     for you." selected by default; **Instructions (optional)** "What Rex should do and how." (the field is called
     **Instructions**, not Brief); a disclosure **"More options: bring your own agent (a program you run yourself; workspace
     owners only)"** that holds BYO (disabled with the reason when `byoBlocked`); **Cancel** and **Create agent**.
     BYO is only offered when `hostedAllowed` is false or the person opens the disclosure; when hosted is not allowed the
     dialog says so in one sentence and BYO is the only choice (keep today's `byoBlocked` logic).
   - **Purpose templates** (rec 5, D-413). Above Name a row of choice chips **"Start from"**: **General assistant ·
     Research · Ops helper · Calendar secretary · Writer · A blank agent**. Choosing one fills the **name suggestion** (only
     when Name is empty), the **Instructions** (6–10 lines, editable), and remembers a **suggested first message**. The
     default is **General assistant**; "A blank agent" leaves Instructions empty with the mockup's placeholder. **A
     template never changes access** (it sets instructions, not grants).
   - **The templates live in `web/src/lib/onboarding/templates.ts`** (no API, not org-editable): an exported typed array
     `{ id, label, nameSuggestion, instructions, firstMessage, schedule?: { cron, prompt } }` and `templateFor(id)`. The
     optional `schedule` is **data for P5-REFINE-14's schedule form** (rec 5: "prompt from a template per purpose"); this
     brief does not create schedules. Write five templates. Example (General assistant): name `Assistant`; instructions in
     plain second person ("You are {name}, a helpful assistant for {person}. Answer in short, clear sentences. Ask a question
     when a request is unclear. Say when you are not sure. Never change anything outside this chat without asking first."
     split in 6–10 lines); first message "Hi {name}, what can you do for me?". Write the other four in the same style (Research:
     finds and summarizes with sources; Ops helper: watches and reports, asks before acting; Calendar secretary: reads the
     calendar, proposes times; Writer: drafts and edits with the person's voice); each ends with an instruction to ask
     before sending or changing anything. Unit-test that every template has all fields and fits the API's `brief` limit.
   - **After Create (channels-4).** Mockup `UXR5/after/channels-4.png`: close the dialog, show **"Rex is ready"** (toast),
     and **go to the new agent's chat** (`/o/<org>/w/<ws>/agents/<handle>`) with the line **"Say hello to Rex."** and the
     composer ready ("Message Rex…"); the template's suggested first message is offered through the chat draft hand-off
     (C-1035 `?draft=`, composer filled and focused, **not sent**). A BYO agent opens on its Computer: Connect page as today.
   - The **kind** shows only inside "More options"; the **handle** field stays visible as in the mockup.
   - D-414 (after P5-REFINE-5): the new hosted agent already has the starter key; if the organization has none, the chat
     says "<Agent> has no AI model yet" (P5-REFINE-11 item 2). Nothing to do here but not to hide it.

2. **Instructions (behaviour-5, behaviour-6, behaviour-8).** Mockups `UXR5/after/behaviour-5.png`, `behaviour-6.png`,
   `behaviour-8.png` (and `-lt`).
   - **behaviour-8:** the avatar is a **visible "Change picture" button** next to the Name, with **Remove** as a real small
     button and the hint **"PNG, JPG or WebP, up to 512 KiB. Shown round."** (**the mockup says 2 MB; the API limit is 512
     KiB** (`avatar-field.tsx:64`): keep the true limit, and say so in the completion report).
   - **behaviour-5:** **Model** is one select, "Workspace default · Pong", with the help "The model Daneel starts with.".
     Model labels are **display names only**: no "(litellm)", no endpoint suffix (use the endpoint's display name from
     the API; raw ids only in Technical details). The **Advanced** fold keeps its place on this page and is shown at the
     **Expert altitude** (wrap in `Altitude level="expert"`, P5-REFINE-3): it opens with **"Advanced is for admins and power
     users. Most people never need it."**, then the roles named **Strongest model · Everyday model · Fastest model**, each
     with the option **"Same as the default"** (never "Use the endpoint's setting (none)"), Subagents use, Effort and
     the read-only Harness line ("Agent program: Claude Code"). Warnings stay off this page (D-385).
   - **behaviour-6:** the page uses the **shared save pattern of P5-REFINE-2**: the floating bar "Unsaved changes" with
     **Discard** and **Save** appears on the first change; after saving it shows **"Saved"** for 2 s (toast pattern as in
     the mockup); errors scroll to the first bad field and the bar says "2 fields need fixing". Remove the local dark bar
     (`instructions-tab.tsx:292`). The **picture** and any instant control show the inline "Saved" next to them.
   - Keep what P5-POLISH-6 built (one card, max 720 px; the "may message you first" switch with its mapping).

3. **Memory: one box (behaviour-1).** Mockup `UXR5/after/behaviour-1.png`, `-lt`, `src/behaviour-1.html`; Before
   `UXR5/before/behaviour-1-b1.png`; screens `B060`, `B062`, `B063`.
   - The tab segments read **Instructions · Memory · Skills · Tests** (P5-REFINE-1/11); the page: **Search memories**, the
     list, and the form.
   - **New memory form:** one big text box labelled **"What should Dani remember?"** with the help **"Plain words are fine.
     Dani reads this before it answers."**; an optional **Title** ("(optional)"); **Save** and **Cancel**. No Key, no
     Description, no "Content (Markdown)". Keep the `SizeMeter` as a quiet count.
   - **The key is made by Aurora:** from the title when there is one, else from the first words of the text; kebab-case
     (`MEMORY_KEY_RE`), at most 64 characters, ASCII (transliterate or drop other characters; if nothing is left use
     `note`), made **unique** against the agent's keys by a numeric suffix (`bobs-tea-2`). The person never sees it (it
     appears only in Technical details for admins). A refusal `409`/exists from the API retries once with the next suffix.
   - **The list shows titles** (the title, else the first line of the text, 60 characters) with a one-line preview, never
     the key twice. Opening a memory shows the same one-box form (title, text) and its revision history as today.
   - Content larger than the API limit is refused with a sentence, not a raw code.

4. **Skills: name, "use it when…", instructions (behaviour-2).** Mockup `UXR5/after/behaviour-2.png`, `-lt`,
   `src/behaviour-2.html`; Before `UXR5/before/behaviour-2-b1.png`; screens `B070`, `B073`, `B075`.
   - The skill form: **Name** ("Weekly report"), **Use it when…** ("someone asks for a weekly status report"; the API's
     one-line `description`), **Instructions** (the `body`; plain text area, the word `SKILL.md` appears only in Technical
     details), **On for Dani** (the `enabled` switch), **Save** and **Delete**. The list row shows the name and "Use it
     when asked for a weekly report".
   - **The skill's folder name is made by Aurora** from the Name (kebab-case, ≤ 64, not starting with the reserved
     `aurora-`, unique by suffix); the display name is the stored name with hyphens as spaces and the first letter capital.
     The folder name shows in the Advanced fold.
   - **Advanced: files and scripts** (a collapsed fold, **Expert altitude**): the file list, "Add a file" with path, upload and
     text, the **Executable** checkbox, and the **precedence paragraph** ("One skill per name. The agent's own skill takes
     precedence over a workspace skill, which takes precedence over an organization skill.") that today comes first on
     `EffectiveSkillsView`. Skills the agent receives from the workspace and organization stay listed under the form as
     "Skills this agent receives" (only when there are some; P5-POLISH-6's rule).
   - `skills-view.tsx` is **shared** with the workspace and organization pages (`workspace-skills.tsx`,
     `org-skills.tsx`, P5-REFINE-16's copy changes). Change the shared form so all three get the new shape, keep their
     props working, and **do not** edit those two files.

5. **Tests: the tab, the judge and the run (behaviour-3, behaviour-4, behaviour-7).** Mockups `UXR5/after/behaviour-3.png`,
   `behaviour-4.png`, `behaviour-7.png` (and `-lt`).
   - **Words (behaviour-7; the renames come from P5-REFINE-1, check they are in):** the tab and page are **Tests**; a run is
     **"Test run, 19 Nov 23:58"** (`formatWhen`, never "Run of Just now"), with "Started by Bob Builder. Finished just now.";
     the list link **"All test runs"**; the summary tiles **Pass rate · Passed · Failed · Couldn't be checked**; the
     reviewer is **"Checked by: Company key (the reviewer key)"** (never "Judge key", "Checked as"); **"Refused writes"** moves under a
     **Details** disclosure; the **configuration hash** is hidden in Technical details (the `TechnicalDetails` primitive).
   - **behaviour-3 (the ANSWER line).** The platform **already appends** "End your reply with a final line in exactly this
     form: ANSWER: <value>" to every number and text question (`scoring.rs:10`, C-862). What was wrong is what the person
     sees. In the question dialog show, under the expected answer, **"Aurora adds 'Finish with a line: ANSWER: ...' to the
     question for you."** (replace `ANSWER_HELP`), and in a result row **never blame the agent**: when the line is
     missing say **"Daneel's reply had no ANSWER line, so Aurora could not compare it. The reply is shown below."** and
     show the reply excerpt; a wrong answer reads **"Answer was "pong", expected 42"**; a match **"Matches 42"**. The results
     table columns are **Question · Answer · Why** with a **Passed/Failed** badge, as the mockup.
     **Do not** accept the number anywhere in the reply (the designer's note: that passes wrong answers).
   - **behaviour-4 (judge and run).** Mockup `UXR5/after/behaviour-4.png`:
     - **The judge is tested when saved.** After Save the page says **"Judge saved and tested: it answered in 3 s."** or, when
       the test fails, **"Judge saved, but the test failed: <reason in words>. Judged questions will not be scored until it
       works."**, from the **`test`** field of the amended settings response (scope 9a).
     - **Live counters:** while a run is active show **"Running · 1 of 2 done · started by Bob Builder"** with a **Cancel
       run** button, and the counters **Passed 0 · Failed 1 · Not scored 1** update as rows finish (from the live `counts`
       of scope 9b; keep the 5 s refetch). A row still waiting says "Running…" or "Waiting for the judge".
     - **A Re-run button on every result row** (**"Re-run"**), which starts a new test run of that one question
       (scope 9c). It is disabled with its reason while a run is active.

6. **Other agents (channels-1, D-411; after P5-REFINE-10).** Mockup `UXR5/after/channels-1.png`, `-lt`,
   `src/channels-1.html`; Before `UXR5/before/channels-1-b1.png`, `-b2.png`.
   - Settings › **Other agents** (the page `?tab=peers`; the sub-nav entry is Expert, P5-REFINE-11). Empty state:
     **"Daneel isn't linked to another agent"** / **"Linked agents can hand each other tasks. The tasks they exchange
     appear here."** / button **Link another agent**.
   - **Dialog "Link another agent":** **Agent** (a picker from `GET /agents/{agent}/linkable`, each option "Rex · Ops
     workspace"; agents already linked, frozen or BYO are shown **disabled with the reason**: "Already linked", "Paused",
     "Runs on its own computer"; an empty list says "There is no other agent you can link. Create one first." with a link
     to Agents); **Who may hand tasks to whom?** as three radio cards: **Both ways** ("Daneel and Rex can each give the
     other tasks."), **Daneel can hand tasks to Rex** ("Rex only works on what Daneel sends."), **Rex can hand tasks to
     Daneel** ("Daneel only works on what Rex sends."), via `directionSentence` of P5-REFINE-10; **Cancel** and **Link
     agents**. The first radio is the default.
   - **The list** (when there are links): one row per link: the peer's name and workspace, the direction sentence, "Linked by
     Bob Builder" (or "Opened by Daneel" for an agent-created link), **My policy** as a select with words (**Ask me first ·
     Allowed · Blocked**, from `labels.ts`; `allow` needs an approver, so the option is disabled with "Only an approver can
     allow this" for others), a **row menu**: **Change direction…** (the same three cards), **Remove link…** (a
     `ConfirmDialog`: "Daneel and Rex will stop handing each other tasks. Tasks in progress between them are cancelled.").
     The old text "Their policy: ask" becomes "Rex asks you first" / "Rex allows it" / "Rex blocks it".
   - **Tasks** stay below the list as sentences ("Daneel handed a task to Rex · completed · 19 Nov 23:58"; the report text
     under it), not "Outgoing / Budget 3/10".
   - Errors in words: not allowed, paused, already linked, stale ("Someone changed this link. The latest version is loaded;
     try again.").

7. **The missing AI key (access-3; after P5-REFINE-5).** Mockup `UXR5/after/access-3.png`, `-lt`, `src/access-3.html`;
   Before `UXR5/before/access-3-b1.png`, `-b2.png`; screens `C041`, `C044`, `C045`.
   - **Header:** when the agent has no usable AI key (the state of P5-REFINE-11 item 2) and the organization's starter key
     exists but was not given to it, put **one primary action next to "Can't answer: no AI key"** in the header's `action`
     slot: **"Give Rex the Company key"** (the key's own name; `useAddCredentialGrants` for the organization's AI key
     account). One click gives it, the header turns to **Ready** and a toast says "Rex can answer now". For someone who
     cannot grant it, **Ask an admin** (the `AskAdmin` control, subject `ai_key`, with the optional reason of C-1081).
   - **Access page:** a card at the top, as the mockup: **"Rex can't answer yet: it has no AI key."** / **"Your
     organization's Company key goes to every new agent automatically, and Rex was created before it was set."** (shown only
     when that is the cause) / buttons **Give Rex the Company key** (primary) and **Choose another** (scrolls to **Add an
     AI key** under Apps and accounts). The collapsed "Not given to Rex (1)" row stays but is no longer the only way.
     When the starter key was added **after** the agent was created, P5-REFINE-5 gives it to the agent itself; the card
     then never shows and the account row reads **"Given to Daneel and 1 other agent"** (from its count field).
   - Words use the key's real name, never "the Company key" literally.

8. **Altitude (D-406).** At Use: Instructions, Apps & access, Memory, Skills. At Expert (the **Show more controls** switch):
   Tests, Other agents, the Advanced fold of Instructions, **Advanced: files and scripts** of a skill, the raw folder name,
   the memory key. A deep link to a Tests or Other agents page works at any altitude (the nav hides the entries, the page
   is never refused).

9. **Small Rust: three additions to the evals domain (lane C owns it for this brief).** Amend the shipped contracts in
   `docs/API_CONTRACTS.md` **before coding** (amendments, **no new C-numbers**):
   a. **C-872 / `PUT /v1/agents/{agent}/eval-settings`:** when the judge changed, after `judge_check` passes make **one
      real, minimal model call** through `PlatformLlm` (C-869, purpose `eval_judge`, `max_tokens` tiny, timeout
      `EVAL_JUDGE_TIMEOUT`) and return it as `judge.test: { result: "ok" | "failed", took_ms: integer, reason: string | null
      }` (reason: a short fixed word such as `rejected`, `unreachable`, `timeout`, `not_served`; never the provider's text).
      **The settings are saved either way** (a failed test is reported, not a refusal). The call writes no transcript and
      is audited as part of `eval.settings.update` (`metadata.judge_test: ok|failed`). A caller who did not change the
      judge gets `test: null`.
   b. **C-864 / `EvalRun.counts`:** while a run is `queued` or `running`, `counts` is **computed from the results rows**
      (pass, fail, error, not_scored so far) instead of the stored columns that only the finish writes
      (`repo/evals.rs:540`); `counts.total` is unchanged. A finished run still reads the stored values.
   c. **C-864 / `POST /v1/agents/{agent}/eval-runs`:** the body gains an optional `question_ids: uuid[]` (1..=20; each must be a
      question of the agent, else 422 `invalid`) to run only those questions; absent = all enabled questions as today. The
      run's `question_set_version` and fingerprint are computed as for any run. The caps (`eval_run_active`, the daily limit)
      apply unchanged.
   - Tests in `services/aurora-api/tests/it/` (extend the evals ITs): judge test ok and failed (with the mock LLM), settings
     saved in both cases, `test: null` when unchanged; counts mid-run read from results; `question_ids` runs only those and
     refuses a foreign id; `permission_matrix.rs` unchanged (same routes). `web/openapi.json` and `schema.d.ts` regenerated.

10. **`labels.ts` and copy.** Every status and outcome word on these pages comes from `lib/labels.ts` (P5-REFINE-2); the
    guard test must pass (no snake_case or UUID outside `<code>`/Technical details): eval outcome codes, link policies,
    skill and memory sources.

## Out of scope

- The Settings shell, the Settings button, the sub-nav and the header's action slot (P5-REFINE-11).
- The Automations list, schedules, webhooks, channels (P5-REFINE-14); using the template's `schedule` data to prefill a
  schedule form (P5-REFINE-14).
- Workspace and organization skills pages' own copy (P5-REFINE-16/17); the workspace Accounts and AI keys pages
  (P5-REFINE-13/18).
- Export/Import an agent (P5-REFINE-19).
- The backend of starter keys (P5-REFINE-5) and of agent links (P5-REFINE-10).

## Constraints / file ownership

- **Owns:** `components/agents/{instructions-tab,avatar-field,model-roles-fields,create-agent-dialog,access-tab,agents-view}.tsx`,
  `components/agents/evals/**`, `components/memory/**`, `components/skills/{skills-tab,skills-view}.tsx`,
  `components/peers/**`, `lib/onboarding/templates.ts` (new), `lib/api/{peers,evals,memory,skills}.ts` (client changes),
  and in Rust `services/aurora-api/src/domains/evals/**` + `crates/aurora-db/src/repo/evals.rs`.
- Do **not** edit `agent-page.tsx`, `sections.ts`, `agent-state.tsx` beyond **one line** that passes the header action
  (P5-REFINE-11 merges first), `status-rail/**`, `components/chat/**`, `components/accounts/**` (read only), `schedules`,
  `webhooks`, `channels`.
- Tokens and shared primitives only (`docs/14 §Visual design`); add a missing primitive to the shared set. Both themes; no
  horizontal scroll at 390 px. Heavy commands through `/home/alex/orca/workspaces/aurora/heavy.sh <lane> <command>`.

## Acceptance criteria

1. **New agent** matches `UXR5/after/channels-2.png` (dark) and `-lt` at 1256 and 390 px: Runs on Aurora is the default,
   BYO is under More options, the field is Instructions, the template chips fill it, and Create opens the new agent's chat
   with "Rex is ready", "Say hello to Rex." and the template's first message in the composer, not sent
   (`UXR5/after/channels-4.png`).
2. **Instructions** matches `behaviour-5.png`/`behaviour-6.png`/`behaviour-8.png`: Change picture button and the hint, plain
   model names, Advanced only at Expert, one save bar, "Saved" for 2 s.
3. **Memory** matches `behaviour-1.png`: one box, optional title, key invisible and unique, the list by title.
4. **Skills** matches `behaviour-2.png`: name, "Use it when…", instructions; files, Executable and the precedence rule under
   Advanced (Expert).
5. **Tests** matches `behaviour-3.png`, `behaviour-4.png`, `behaviour-7.png`: the words, the ANSWER note and the failure
   reasons that do not blame the agent, "Judge saved and tested", live "1 of 2 done" counters, Re-run per row.
6. **Other agents** matches `channels-1.png`: empty state, the dialog with the three direction cards, the list with the
   policy select and the row menu.
7. **Access** matches `access-3.png`: the header action and the Access card; one click gives the key.
8. Rust: the judge test, the live counts and `question_ids` behave as scope 9 with the listed ITs; the existing evals tests pass.
9. Gates: `pnpm typecheck`, `pnpm lint`, targeted vitest, targeted Playwright (mocked) with axe and no console errors, both
   themes, 1280 and 390 px; `cargo fmt --check`, clippy `-D warnings` for aurora-api and aurora-db, targeted `cargo test`
   for evals; `pnpm gen:api` with no diff. GitHub CI is the full gate. Report counts.

## Tests

Targeted only. Heavy commands through `heavy.sh`.

- **Vitest** (`web/tests/`): the template data (every field, length), `templateFor`; the create dialog (default hosted, BYO
  under More options and disabled with its reason, template fills Name and Instructions only when empty, draft hand-off on
  success); memory key generation (title, first words, non-ASCII, empty, 64-char cut, collision suffix, retry on 409);
  skill folder-name generation and the reserved prefix; the Tests wording and the ANSWER-line reason sentences; run
  progress "1 of 2 done"; the Re-run request body; the link dialog (disabled options with reasons, direction sentences,
  the three bodies); the Access card shows only when the starter key is not given; Altitude wrapping of the fold, the
  skill files and the Tests entry.
- **Playwright (mocked)** (`web/e2e/`), 1280 and 390 px, both themes, axe, no console errors:
  - New agent with a template → the new agent's chat with the draft (extend `agent-config.spec.ts`);
  - write a memory in one box; create a skill by name and "use it when";
  - add an eval question, run it, see "1 of 2 done", Re-run a row (extend `agent-evals.spec.ts`);
  - Instructions edit → save bar → "Saved"; avatar change shows "Saved";
  - Link another agent and remove the link (mocked P5-REFINE-10 routes);
  - Give the starter key from the header and from the Access card (extend `agent-access.spec.ts`).
- **API integration** (Rust): as scope 9.

## Tests move with the UI (owner, 2026-11-20 — binding)

The owner: "Make sure the briefs not only change the website, but also the tests that test it! I don't want a ton of
broken tests because a button was renamed and the tests look for the old label." (P5-REFINE-1 merged with old labels
in tests, and master's CI broke until the CI Watcher fixed it in `ee7bfa0d`.) So, in **every commit** of this brief:

1. **Find every test that sees what you change.** Before committing a change to visible text, a label, an
   `aria-label`, a role, a heading, a route or `?tab=` id, a page's structure, an API field, an error code or message,
   search all test trees for the **old** strings, selectors and paths, and for the components you touched:
   - `web/tests/**` (vitest + Testing Library);
   - `web/e2e/**` (Playwright: the mocked default project **and** the `production` project that `scripts/e2e/*.sh`
     run against a real stack — `first-run.sh`, `kind.sh`, `files.sh`, `ssh.sh`, `channels.sh`, `approvals-park.sh`…);
   - `scripts/e2e/*.sh` themselves (they grep pages and call routes);
   - Rust tests that assert a message, an error code or a response shape (`services/*/tests/**`, `crates/*/src/**`
     `#[cfg(test)]`, `services/aurora-api/tests/it/**`);
   - fixtures and mocks (`web/e2e/fixtures/**`, `web/tests/**/mocks*`, MSW handlers, `web/openapi.json` snapshots).
   A plain `rg -n "<old text>|<old selector>" web/tests web/e2e scripts/e2e services crates` per change is the minimum.
2. **Update them in the same commit as the change.** New expectations use the new words exactly as the mockup shows
   them. Prefer `getByRole(…, { name })` or a `data-testid` over raw text where the text is not the point of the test;
   where the words are the point (they are the fix), assert the new words. Never loosen an assertion so that both the
   old and the new label pass, never `.skip`/`test.fixme`/delete a test to get green; a test is removed only when the
   feature it covers is removed, and the completion report names it and why.
3. **Run what you touched, plus everything that references it.** Through `heavy.sh`: every vitest file you changed
   **and** every vitest file that renders a component you changed; every Playwright spec (mocked project) that visits a
   page you changed. **Exception to the targeted-tests rule for this set (owner, 2026-11-20):** a commit that renames
   or restructures shared UI (a primitive, the app shell, the rail, the agent page, a label used on several pages) runs
   the **whole** web vitest suite once (`cd web && /home/alex/orca/workspaces/aurora/heavy.sh <lane> pnpm exec vitest run` (not `pnpm --dir web …`, which fails here)) before handing over. Production
   specs (`--project production`) can't run in a lane without a real stack: update them by reading, and list them in
   the completion report so the nightly / P5-REFINE-20 runs them.
4. **New behaviour gets new tests** (the brief's Tests section); old tests are **kept and updated**, not replaced.
5. **The completion report lists** every test file changed (and why), every test removed (and the removed feature),
   the commands run with their results, and the production specs that only the nightly will run.
6. A merge that turns master's CI red because a test still expects old UI is a send-back to this lane, not the CI
   Watcher's job.

## Docs to update

- `docs/API_CONTRACTS.md`: the three amendments (C-872, C-864 ×2) in their entries.
- `docs/10-agents-conversations.md` § Evals (the live counts, a judged-run re-run, the judge test) and
  `docs/14-web-dashboard.md` § Agent page: Settings pages (Instructions, Memory, Skills, Tests, Other agents), § 1 New agent
  and the template data, the vocabulary rows added.
- `docs/PROGRESS.md`, `docs/FEATURES.md` rows (behaviour-1..8, channels-1/2/4, access-3 UI; note which items waited for
  which backend brief).

## Suggested agent/model and reason

Sonnet (`nextjs-developer`) for the web; the small Rust (scope 9) fits the same lane because it is three bounded changes in
one domain with ITs; if the Orchestrator prefers, split item 9 to `rust-developer`. Not Haiku: many forms, a key-generation
rule and a security-adjacent grant button.

## Completion report

(Fill in when done; P5-REFINE-20 reads this.)

- **Items done** (scope items 1–10, each with commit id; say which were done after which backend merge):
- **Contract amendments** (C-872, C-864 ×2) as landed, and any difference from scope 9:
- **Deviations from this brief, and why** (the avatar limit, the template default, anything the mockup showed that the API
  cannot do):
- **Templates:** the five templates' final text is in `web/src/lib/onboarding/templates.ts`; list any the owner should
  review:
- **Screenshots compared** (path beside its mockup, both themes, 1256 and 390 px): behaviour-1…behaviour-8, channels-1,
  channels-2, channels-4, access-3:
- **Gate counts:** typecheck, lint, vitest, Playwright, axe; cargo fmt/clippy/tests; gen:api:
- **Open questions for the owner:**
