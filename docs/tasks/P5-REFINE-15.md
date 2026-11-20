# P5-REFINE-15: Computer: files first, terminal that tells the truth, SSH, phases in words, technical details for admins (owner, 2026-11-20)

## Goal

The Computer tab is a place with the person's files in it, not a sandbox report. UX review 5 found, in bullets:

- A member sees "asleep / Wake it now" and then Technical details with the image name, the runner's cluster IP and
  `waiting_supervisor`; it stays on "waiting" after the computer woke.
- When Files are on, the first thing they show is `AGENTS.md`, `CLAUDE.md` and `memory` (the agent's internals, "Read-only"
  with no reason), mixed with the person's uploads; Show hidden exposes `.aurora` and `.claude` without a warning.
- The terminal's 30-second idle sleep killed a running command and started a new shell in the same scrollback; the
  terminal loops "Reconnecting... (attempt 4 of 4)" with a blinking cursor and no error; Files say "The computer
  restarted; retrying..." forever with Upload enabled.
- Files cannot move a file into a folder; New file edits a file that does not exist and Back drops it without a warning;
  no feedback for upload, rename, delete or download; the phone row menu sits on top of the Size column; big text files
  say "No preview for this file"; CSV shows raw text.
- SSH hard-codes `~/Downloads/id_ed25519-cert.pub`; a second download becomes "id_ed25519-cert (1).pub"; the certificate
  expires after 15 minutes and the Download button disappears on reload; the user name is the agent's UUID.
- The light-theme terminal prompt is pale green on white (about 1.6:1). A phone terminal has no Esc, Tab or Ctrl keys and
  offers "Open in new window". Once awake the computer offers only Restart, with no "sleeps in 20 s".

Owner decisions that apply: **D-405** (this set; precedence: this brief, then D-405..D-419 and the artifact comments,
then the mockup, then the finding's fix text), **D-406** (Use / Set up / Expert, one "Show more controls" switch),
**D-408** (the header status follows the computer's readiness; Terminal and Files show the same line and connect by
themselves), **D-412** (members see computer phases in words: Off, Starting, On, Stopping, Failed; the raw phase is a
secondary line at Expert; the sandbox image and runner address are **instance-admin only**), **D-416** (an open terminal or
SSH session keeps the computer awake; a restart prints a notice in the terminal; the runner re-decides its stream
listener on re-registration).

## Linked requirements

- **Artifact (source of truth):** <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt>; anchors `#f-<key>`. Workers
  cannot open it; use the local copy.
- **Local copy `UXR5/`** = `/home/alex/orca/workspaces/aurora/design-ux-review-5/`: `findings.md` (authoritative text),
  `after/<key>.png` (dark) and `after/<key>-lt.png` (light) are the visual spec, `after/src/<key>.html` the markup (the
  PNG wins if they differ), `before/<key>-b<n>.png` the Before crops, `screens/<id>.png` the full screenshots, `notes/`
  the walkers' notes. Yellow dashed notes are commentary, not UI.
- **Findings (anchor `https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt#f-<key>`):**
  - `computer-1` · After `UXR5/after/computer-1.png`, `computer-1-lt.png`, `after/src/computer-1.html` · Before
    `UXR5/before/computer-1-b1.png`, `computer-1-b2.png` · Screens `UXR5/screens/C134-terminal-longrun.png`,
    `C135-terminal-interrupted.png`, `C136-terminal-after-wake.png`, `A201-agents-computers-after-terminal.png`.
  - `computer-2` · After `computer-2.png`, `computer-2-lt.png`, `src/computer-2.html` · Before `computer-2-b1.png`,
    `computer-2-b2.png` · Screens `C103-terminal-failed.png`, `C106-terminal-after-wait.png`, `C111-files-after.png`.
  - `computer-3` · After `computer-3.png`, `computer-3-lt.png`, `src/computer-3.html` · Before `computer-3-b1.png`,
    `computer-3-b2.png` · Screens `C150-row-menu.png`, `C151-rename.png`, `C153-move-by-rename.png`.
  - `computer-4` · After `computer-4.png`, `computer-4-lt.png`, `src/computer-4.html` · Before `computer-4-b1.png` ·
    Screens `C154-new-file.png`, `C155-new-file-editor.png`, `C145-edit-txt.png`.
  - `computer-5` · After `computer-5.png`, `computer-5-lt.png`, `src/computer-5.html` · Before `computer-5-b1.png` ·
    Screens `C165-m-files.png`.
  - `computer-6` · After `computer-6.png`, `computer-6-lt.png`, `src/computer-6.html` · Before `computer-6-b1.png`,
    `computer-6-b2.png` · Screens `C112-ssh.png`, `C117-ssh-cert.png`, `C161-ssh-active.png`.
  - `computer-7` · After `computer-7.png`, `computer-7-lt.png`, `src/computer-7.html` · Before `computer-7-b1.png` ·
    Screens `C050-computer-technical.png`, `C051-computer-technical-full.png`.
  - `computer-8` · After `computer-8.png`, `computer-8-lt.png`, `src/computer-8.html` · Before `computer-8-b1.png` ·
    Screens `C162-light-terminal.png`.
  - `computer-9` · After `computer-9.png`, `computer-9-lt.png`, `src/computer-9.html` · Before `computer-9-b1.png`,
    `computer-9-b2.png` · Screens `C140-files.png`, `C156-show-hidden.png`.
  - `computer-10` · After `computer-10.png`, `computer-10-lt.png`, `src/computer-10.html` · Before `computer-10-b1.png`,
    `computer-10-b2.png` · Screens `C143-uploaded.png`, `C146-saved-txt.png`, `C160-download.png`.
  - `computer-11` · After `computer-11.png`, `computer-11-lt.png`, `src/computer-11.html` · Before `computer-11-b1.png`,
    `computer-11-b2.png` · Screens `C149-preview-big.png`, `C147-preview-csv.png`.
  - `computer-12` · After `computer-12.png`, `computer-12-lt.png`, `src/computer-12.html` · Before `computer-12-b1.png` ·
    Screens `C122-m-terminal.png`, `C164-m-light-terminal.png`.
  - `computer-13` · After `computer-13.png`, `computer-13-lt.png`, `src/computer-13.html` · Before `computer-13-b1.png`,
    `computer-13-b2.png` · Screens `C048-computer-waking.png`, `C049-computer-awake.png`.
- **Layer-above rec 11** (`UXR5/layer-above.md`, anchor `https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt#above`): Files
  first, the state line, Terminal and SSH at Expert, Technical details by role. Rec 1 (altitudes), rec 15 (Pause/Resume
  words: this page's "Sleep now" is not a pause; it is a computer action), rec 13 (words come from `lib/labels.ts`).
  The runner bug section of `UXR5/priorities-themes.md` (anchor `#bug`): the backend fix is P5-REFINE-4, this brief shows
  what the person sees while it is broken.
- **Decisions:** D-405, D-406, D-408, D-412, D-416. **Contracts:** C-1057..C-1060 (P5-REFINE-4: computer readiness,
  phases in words and member-safe details, Sleep now and "sleeps in" data, certificate expiry and the SSH name) in
  `docs/API_CONTRACTS.md`; C-1045/C-1046 (wake route, `idle_stop_after_secs`), C-898..C-904 (files), C-790..C-795 (SSH).
  **Security:** SEC-141 (what a member may see of a computer) applies to item 7.
- **Docs:** `docs/14-web-dashboard.md` §Agent page, §Terminal tab, §Files, §SSH; `docs/12-supervisor-harness.md` (which files
  Aurora writes for the agent: `AGENTS.md`, `CLAUDE.md`).

## Wave and order

- Lane A, after **P5-REFINE-11** (the agent page: tabs Chat · Automations · Computer, the Settings button and the
  header status line) and **P5-REFINE-4** (backend). Also after **-1** (renames), **-2** (primitives: `labels.ts`,
  `ResponsiveTable`, `TechnicalDetails`, `useUndoToast()`), **-3** (`<Altitude>`).
- **Items that wait for P5-REFINE-4** (do the others first; keep today's behaviour for the dependent part and say so in the
  completion report):
  - item 1's state line source and phase words, item 7 (member-safe details and admin-only fields), item 8 (Sleep now, the
    "sleeps in" countdown), item 3's "The computer restarted" notice (needs the runner/API to tell the pane a new shell
    started), item 6's certificate expiry and the `aurora ssh <handle>` command if the handle mapping exists.
- Items 2 (terminal copy and stop), 4 (files), 5 (editor), 9 (palette, phone keys) have no backend dependency.
- Lane A continues with P5-REFINE-19 (expert speed), which needs this page's sub-nav.

## Context

All paths are under `web/src/`; line numbers are master `c0fe7d10`.

- **Agent page sections** `components/agents/sections.ts:67–73`: the `computer` section holds the pages `computer`, `terminal`,
  `files`, `previews`, `ssh`, `connect` (BYO). `isPageAvailable` :91–100 (terminal and files exist exactly where
  `terminal.enabled` is on; ssh where `features.ssh`). `agent-page.tsx:313–319` renders them.
- **Computer page** `components/agents/computer-tab.tsx` (376 lines): the state sentence and actions :222–260 (`Wake it now`
  :238, `Restart` :243, `Try again` :248, the "isn't set up" line :255), `UsageCard`, `TerminalShellSection` :31, Ports
  :272, the Restart confirm :296, the **Technical details** `<details>` :312–371 that shows `Image:` :317 to everyone,
  `StartAttempt` :74, `StartLog` :129, "Awake because" :321, Processes :333. Hooks `useSandbox` (5 s poll),
  `useWakeSandbox`, `useRestartSandbox` in `lib/api/conversations.ts:153–180`.
- **Files** `components/agents/files-tab.tsx` (382 lines): `FilesBrowser` :72, hidden state :77, `isManaged` use :120/:164
  (`lib/api/files.ts:122` = only `/workspace/.aurora`), the toolbar :172–192 (Upload, New file, "Show hidden"), upload tray
  :219, drag and drop :248, empty state :278–285, row actions :293, `FilesError` :355. `components/agents/files/file-list.tsx`
  (`RowMenu` :79–106: Open, Download, Rename, Delete, Copy path; `DataTable` with `max-md:block` :115), `dialogs.tsx`
  (`NameDialog` :64: one name, no slash), `use-files-path.ts`, `upload-queue.ts`, viewer `files/viewer/file-viewer.tsx`
  (`dirty` :146, "Unsaved changes" :210, Save :218, Back to folder :87/:385, `beforeunload` :157, `status` line :243 which
  shifts the editor), `viewer/editor.tsx`. API `lib/api/files.ts` (`isHidden` :124 = name starts with ".").
  **The API already moves a file:** op `rename` with a full `to` path, e.g. `/workspace/odir/mine` (integration test
  `services/aurora-api/tests/it/files.rs:952`); only the web's name dialog refuses a slash. **No backend work for Move.**
- **Terminal** `components/agents/terminal-tab.tsx` (673 lines): `TerminalTab` :147 (the "Open in new window" button :164),
  `TerminalPane` :177, `MAX_ATTEMPTS = 4` :101, `MAX_WAKE_ATTEMPTS = 12` :107, `CLOSE_COPY` :75, the status text :388–401
  ("Waking…", "Connecting…", "Reconnecting… (attempt n of m)"), the "Not connected" line :450, the ended Notice with Reconnect
  :456–476, paste and right-click menu. `terminal-window.tsx` (the pop-out). Test mock: `e2e/support/terminal.ts`.
- **Terminal colours** `app/globals.css:212–263`: light tokens `--aurora-term-color-0..15`, `--term-pane-*`; the pane sets
  `--term-*` at :265.
- **SSH** `components/agents/ssh-page.tsx` (213 lines): `CertificateDownload` :30–47 (`download="id_ed25519-cert.pub"`),
  `SshContent` :61, the `aurora ssh ${agentId}` block :28 (it prints the UUID), the plain command :18 hard-coding
  `~/Downloads/id_ed25519-cert.pub`, the known_hosts block :119. API `lib/api/ssh.ts` (`useMintSshCertificate`,
  `SshCertificate` has `valid_before`, `username`, `host`, `port`, `known_hosts`).
- **Shared:** `components/ui/{responsive-table,technical-details,confirm-dialog,segmented,menu,select,notice,status-line}.tsx`
  (P5-REFINE-2), `useUndoToast()`, `lib/labels.ts`, `formatWhen`, `<Altitude>` (P5-REFINE-3), the instance-admin check
  (`lib/api/roles.ts`, `useMe().data?.user.is_instance_admin`; confirm the field name in `lib/api/auth.ts`).
- **Tests:** `tests/computer-tab*.test.tsx`, `tests/files*.test.tsx`, `e2e/files.spec.ts`, `e2e/terminal.spec.ts`,
  `e2e/ssh.spec.ts`, `e2e/support/{files,terminal}.ts`.

## Scope

1. **Computer opens on Files, with one line of state (rec 11; D-408, D-412).**
   Anchors `#above` rec 11, `#f-computer-9`, `#f-computer-13` · After `UXR5/after/computer-9.png`, `computer-9-lt.png`,
   `computer-13.png`, `computer-13-lt.png`; markup `after/src/computer-9.html`, `computer-13.html` · Before
   `computer-9-b1.png`, `computer-13-b1.png` · Screens `C047-computer.png`, `C140-files.png`, `C049-computer-awake.png`.
   - The Computer tab's sub-navigation is a segmented row **Files · Terminal · Connect · Previews · Details**. The tab opens
     on **Files** whenever Files exist (terminal feature on). Terminal, Connect (SSH) and Previews are **Expert**
     (`<Altitude level="expert">`: without the switch the row shows **Files · Details** and "n more controls hidden · Show");
     for a person with the switch they are all there. When the instance has the terminal off, the tab opens on
     **Overview** (today's state card) with the existing "isn't set up" sentence (`computer-tab.tsx:255`) and the
     "ask an admin" line by role (D-407).
   - **One line of state above every sub-page** (Files, Terminal, Connect, Details), the same line the header shows
     (D-408; P5-REFINE-11 owns the header): a `StatusLine` with the phase in words and what follows:
     - "Dani's computer is **on** · sleeps after 20 min of quiet" (phase On; "sleeps in 20 s when idle" while idle, item 8);
     - "Dani's computer is **off** · it starts when Dani needs it" (Off);
     - "**Starting** Dani's computer…" (spinner; "Usually under 20 seconds.");
     - "**Stopping**…";
     - "Dani's computer **couldn't start**: <reason in words>" with **Try again** (Failed).
     The words are `lib/labels.ts` entries for the phase enum (Off, Starting, On, Stopping, Failed); no raw phase anywhere
     outside Expert/Technical details. Source: the readiness/phase field of C-1057..C-1060 (**waits for P5-REFINE-4**); until
     then derive from `SandboxView.state` (`running`→On, `starting`→Starting, `stopping`→Stopping, `failed`→Failed,
     `stopped`/`unprovisioned`→Off).
   - Actions on the line: **Wake it now** and **Restart** only at Set up and above (`<Altitude level="setup">`; a member who
     only uses Files sees none), **Sleep now** when On (item 8). Files and the Terminal **connect by themselves** (they wake
     the computer; today's behaviour): no "Wake it now" is needed to use them.
   - `Details` is today's Overview body (usage card, ports, Technical details) without the state card duplicated.
   - Old `?tab=terminal|files|ssh|previews|computer` ids keep opening the right sub-page.

2. **Terminal: says what happened; stops after the last retry (computer-1, computer-2).**
   Anchors `#f-computer-1`, `#f-computer-2`, `#bug` · After `UXR5/after/computer-1.png`, `computer-1-lt.png`,
   `computer-2.png`, `computer-2-lt.png`; markup `after/src/computer-1.html`, `computer-2.html` · Before `computer-1-b1.png`,
   `computer-1-b2.png`, `computer-2-b1.png`, `computer-2-b2.png` · Screens `C134`, `C135`, `C136`, `C103`, `C106`.
   - **After the last retry** (`MAX_ATTEMPTS` or `MAX_WAKE_ATTEMPTS`) the pane **stops**: no "Reconnecting…", no blinking cursor,
     no accepted typing. It shows, centred: **"Couldn't connect to the computer"** / "Tried 4 times. The computer didn't answer." /
     **Try again** / a small **Report it** link (opens the Technical details copy, Expert). The "Not connected — typing is not
     sent" line stays. The `ended` Notice at :456–476 is the place; make the retry loop end in it for the 4/12-attempt
     case too and add the sentence. Reproduce with the mocked websocket (`e2e/support/terminal.ts`) closing with 1006 five
     times; today the pane keeps "Reconnecting".
   - **The restart notice.** When the pane reconnects and the computer started a **new shell** (the API tells it, C-1057..1060;
     **waits for P5-REFINE-4**), the pane prints one line into the scrollback in muted text: "— The computer restarted at
     14:02; your running commands were stopped. —" (time in the person's zone). Before the backend merges, do not
     print it; do not infer it from the host name.
   - **Above the pane**, one sentence replaces the old explanation: "An open terminal keeps the computer awake (up to 4 hours),
     so a running command is never cut off." (the 4 hours comes from the instance setting, not hard-coded; the sentence is
     shown only when the setting is on). **Designer's note (follows):** keeping the computer awake is the fix (backend, D-416);
     the restart line is the fallback.
   - "Open in new window" is **hidden below 640 px** (computer-12) and is an Expert item.

3. **Files: the agent's own files are grouped and explained (computer-9).**
   Anchor `#f-computer-9` · After `UXR5/after/computer-9.png`, `computer-9-lt.png`, `after/src/computer-9.html` · Before
   `computer-9-b1.png`, `computer-9-b2.png` · Screens `C140`, `C156`.
   - The root folder shows two groups: **Your files** (everything else) and **Dani's own** (the agent's instruction and memory
     entries: `AGENTS.md`, `CLAUDE.md`, `memory`), under a one-line explanation: "The grey files are Dani's instructions and
     memory. Aurora keeps them for you; edit them on the Behaviour page." (link "Behaviour page" goes to Settings › Instructions
     per D-408). The group "Dani's own" is collapsed by a header row on a phone and shown grey on a desktop; each entry has a
     **Managed** badge (not "Read-only" without a reason) and a second line: "Instructions, kept by Aurora" / "Memory, kept by
     Aurora". Keep the list in one constant `AGENT_OWN_ENTRIES` in `lib/api/files.ts` next to `isManaged`; check which files
     Aurora writes in `docs/12-supervisor-harness.md` (lines 96, 162, 313) and the runner's rendering code and say what you
     found in the completion report. They stay **read-only by default**; Open shows them (read-only), Download works.
   - Hidden entries stay hidden. **Show hidden** is an **Expert** control (`<Altitude level="expert">`) and **warns before it
     opens a hidden file** (an editable one, such as `.claude/settings.json`): a notice under the list "Hidden files hold
     settings Aurora and the agent rely on. Edit them only if you know why." shown while Show hidden is on, and a confirm
     dialog the first time a hidden file is opened for editing ("Edit a hidden file? ... [Cancel] [Edit anyway]").
   - The folder at the top is named for the person: breadcrumb root reads "Your files with Dani" (path `/workspace` stays in
     Copy path).

4. **Files: Move to…, stacked phone rows, toasts, big files and CSV, failed state (computer-2, 3, 5, 10, 11).**
   Anchors `#f-computer-2`, `#f-computer-3`, `#f-computer-5`, `#f-computer-10`, `#f-computer-11` · After
   `UXR5/after/computer-2.png`, `computer-3.png`, `computer-5.png`, `computer-10.png`, `computer-11.png` (each with `-lt`);
   markup `after/src/computer-2.html`, `computer-3.html`, `computer-5.html`, `computer-10.html`, `computer-11.html` · Before
   `computer-2-b1.png`, `computer-2-b2.png`, `computer-3-b1.png`, `computer-3-b2.png`, `computer-5-b1.png`,
   `computer-10-b1.png`, `computer-10-b2.png`, `computer-11-b1.png`, `computer-11-b2.png` · Screens `C111`, `C150`, `C151`,
   `C153`, `C165-m-files.png`, `C143`, `C146`, `C160`, `C149`, `C147`.
   - **Failed state (computer-2).** When the folder cannot load (the computer did not answer after the retries), the page
     shows "Couldn't connect to the computer" / "Tried 4 times. The computer didn't answer." / **Try again**, **Upload** and
     **New file** are **disabled** (not hidden) with the line "Nothing loaded. Actions come back when the computer answers.".
     No endless "The computer restarted; retrying…" (`FilesError` :355).
   - **Move to… (computer-3).** The row menu (`RowMenu`) gets **Move to…** between Rename and Delete (writable entries only):
     a dialog "Move fruit.csv" / "Choose a folder in workspace." with a folder tree (one level at a time, breadcrumb, folders
     only, `.aurora` excluded) and **Cancel** · **Move here**; it sends the existing `rename` op with a full `to` path (no
     replace; a clash shows the existing clash dialog of `dialogs.tsx:197`). **Drag and drop**: a file row can be dragged onto
     a folder row (highlighted drop target, keyboard alternative is the dialog); dropping from outside still uploads
     (`files-tab.tsx:248`). Rename keeps refusing a slash, with the hint "To move it, use Move to…".
   - **Phone rows (computer-5).** Below 900 px each entry is a stacked row (`ResponsiveTable`): type tile, name, then "3.2 KiB ·
     Just now" under it, the menu button at the right edge **in its own column**, never over the size. At 390 px no text is
     clipped, the menu is 44 px, the row height does not change with a long name (it wraps).
   - **Toasts (computer-10).** Upload, rename, delete, download and Move give a toast: "Uploaded report.csv", "Renamed to
     fruit.csv", "Deleted note.txt" with **Undo** where the file can come back (Delete: only if the API can restore; if it
     cannot, no Undo and the confirm dialog stays), "Downloaded fruit.csv", "Moved fruit.csv to docs". Use `useUndoToast()`
     (a toast without Undo when there is nothing to undo; if the helper requires an action, extend it with an optional one in
     P5-REFINE-2's file and say so). The **uploads panel collapses when done** to a line "Uploads: 1 file done · Hide"; it no
     longer pushes the list below the fold. **"Saved" in the editor is a toast** and the `status` line at `file-viewer.tsx:243`
     is removed, so the editor does not move.
   - **Big text files and CSV (computer-11).** A text file over 1 MiB opens a **read-only preview of its first 1 MiB** with the
     note "Showing the first 1 MiB of 3.4 MiB." and **Download the whole file**; use a ranged read if the API supports it,
     else the first bytes of the download stream (check `GET .../files/content`; if neither is possible report it and keep
     "No preview" with the Download button). A `.csv` opens a **table view** (first 200 rows, first 50 columns, header row,
     "fruit.csv · table view · read only") with a "Show as text" switch. Cells are text, never HTML.

5. **Editor: a dirty dot, Save only with changes, ask before leaving (computer-4).**
   Anchor `#f-computer-4` · After `UXR5/after/computer-4.png`, `computer-4-lt.png`, `after/src/computer-4.html` · Before
   `computer-4-b1.png` · Screens `C154`, `C155`, `C145`.
   - A dot (●) after the file name while there are unsaved changes (`file-viewer.tsx:208–214`; the text "Unsaved changes"
     moves to the dot's title and a visually hidden label); **Save** is enabled only with changes (it is, partly: verify and
     fix for a New file, whose Save should be enabled only once something is typed).
   - **Back to folder** with unsaved changes opens **"Leave without saving?"** / "notes.txt has changes that aren't saved." /
     **Keep editing** · **Discard** · **Save and leave**. The same for switching file, for the breadcrumb and for the sub-nav
     (the page's own links), plus the existing `beforeunload`.
   - **Designer's note (follows):** the mockup chose the warning on leave plus the dot, **not** creating an empty file at once
     (it would leave stray empty files). A New file is created only on its first Save.

6. **SSH: lead with the Aurora command; a certificate that stays (computer-6).**
   Anchor `#f-computer-6` · After `UXR5/after/computer-6.png`, `computer-6-lt.png`, `after/src/computer-6.html` · Before
   `computer-6-b1.png`, `computer-6-b2.png` · Screens `C112`, `C117`, `C161`.
   - Title "Connect to Dani's computer". First block: "With the Aurora command line:" **`aurora ssh daneel`** with **Copy**. The handle,
     not the UUID, is in the command; if the CLI cannot resolve a handle yet (P5-REFINE-4's answer, C-1057..C-1060), show the
     id **hidden behind Copy command** (the block shows `aurora ssh …` and Copy puts the full command on the clipboard).
   - Second block "Plain ssh": the certificate state line **"Certificate valid for 13 more minutes (until 00:10)"** (a live
     countdown from `valid_before`, in the person's time; "Expired · Get a new certificate" when past), **Download**, and the
     command `ssh -i ~/.ssh/aurora-daneel -o CertificateFile=~/.ssh/aurora-daneel-cert.pub daneel@ssh.aurora.acme.example`
     with **Copy** (user = handle when the terminator maps it, else the id behind Copy command, as above; host from
     `host`). One line under it: "Download stays until the certificate expires, and always saves under the same name."
   - **Stable name:** the file is offered as `aurora-<handle>-cert.pub` (the `download` attribute), the command uses the same
     path, and a line says "Save it as `~/.ssh/aurora-daneel-cert.pub`". (A browser may still add "(1)" if the file exists;
     the line tells the person to replace the old one.)
   - **The Download survives a reload** until `valid_before`: keep the minted certificate (it is a public certificate, not a
     secret) in `sessionStorage` keyed by agent and key id, ignoring storage errors; remove it at expiry. Minting again is one
     button, "Get a new certificate" (rate limit message in words: `ssh_mint_rate_limited`).
   - The sessions list (`SshSessionsList`) and known_hosts block stay, under Expert (`<Altitude level="expert">`).
   - Waits for P5-REFINE-4 only for the handle mapping and the API's expiry field; the countdown from `valid_before` exists today.

7. **Technical details: phases for members, the rest for instance admins (computer-7; D-412, SEC-141).**
   Anchor `#f-computer-7` · After `UXR5/after/computer-7.png`, `computer-7-lt.png`, `after/src/computer-7.html` · Before
   `computer-7-b1.png` · Screens `C050`, `C051`.
   - **A member** (anyone who is not an instance admin) sees, in **Details**: "Dani's computer" / **"Started 23:55 · running"**
     (the phase in words and when it started; from the API's member-safe fields) and nothing else: no image, no runner name or
     address, no raw phase, no start log lines with cluster text. The member-safe failure reason in words stays on the state line.
   - **Instance admins** see the same line and, under a heading **Admin only**, a `TechnicalDetails` (collapsed): **Image**
     (`aurora/agent:2026.11`), **Runner** (`runner-1 · 10.42.0.17`), **State** (`running (updated 23:56)`, the raw phase),
     the last start attempt, the start log, "Awake because", Processes. The raw phase is also visible at **Expert** for
     everyone as a secondary line under the phase word (D-412) **only if it is member-safe**; `waiting_supervisor` is a raw
     phase, so show it to instance admins and to Expert members only when the API says it is member-safe, else never.
   - The values are **current**: they come from `useSandbox`'s 5 s poll; a computer that woke no longer reads "waiting" (a vitest
     with a state change from starting to running re-renders the line).
   - Hiding is **server-driven**: the member's API view must not carry the image or runner (P5-REFINE-4, SEC-141); the web must
     not merely hide fields it received. If the API still returns them to members, report it as a blocker and hide them
     meanwhile, saying so in the completion report.

8. **Awake: "sleeps in", Sleep now, Waking spinner (computer-13). Waits for P5-REFINE-4.**
   Anchor `#f-computer-13` · After `UXR5/after/computer-13.png`, `computer-13-lt.png`, `after/src/computer-13.html` · Before
   `computer-13-b1.png`, `computer-13-b2.png` · Screens `C048`, `C049`.
   - On: "**Awake** · sleeps in 20 s when idle" (countdown from the idle data of C-1057..C-1060, `idle_stop_after_secs` and last
     activity; shown only while idle, otherwise "sleeps after 20 min of quiet"), buttons **Sleep now** (outline; sets the
     computer to sleep, no confirmation, a toast "Dani's computer is going to sleep") and **Restart** (existing confirm:
     "Restart this computer? Running processes will be stopped and the computer boots again.").
   - Waking: "**Waking** · Starting the computer. Usually under 20 seconds." with a spinner and the existing 5 s poll; no
     progress bar.
   - Before the backend merges: show "sleeps after N min of quiet" from `idle_stop_after_secs` (as today) and no Sleep now.

9. **Terminal: readable in light mode; phone keys (computer-8, computer-12).**
   Anchors `#f-computer-8`, `#f-computer-12` · After `UXR5/after/computer-8.png`, `computer-8-lt.png`, `computer-12.png`,
   `computer-12-lt.png`; markup `after/src/computer-8.html`, `computer-12.html` · Before `computer-8-b1.png`,
   `computer-12-b1.png` · Screens `C162-light-terminal.png`, `C164-m-light-terminal.png`, `C122-m-terminal.png`.
   - **Palette.** In `app/globals.css:216–241` the light ANSI tokens are mostly fine but the **bright** ones (9–14) are not on
     white: `--aurora-term-color-10` `#059669` ≈ 3.8:1, `-11` `#d97706` ≈ 3.2:1, `-14` `#0891b2` ≈ 3.7:1, and a bold green
     prompt maps to a bright colour. Write a unit test that computes the contrast of every `--aurora-term-color-0..15` against
     `--term-pane-bg` for **both** themes (read the tokens from the CSS file) and require **≥ 4.5:1**; adjust the failing light
     values (darker, same hue) until it passes; dark values pass already, keep them. Take a light-mode screenshot of a real
     colourful prompt (`PS1` with green user@host and blue path) to confirm. Programs that print their own 256-colour or true
     colour are not changed; say so in the completion report.
   - **Phone key row.** Below 640 px a row sits **above the keyboard**: **Esc · Tab · Ctrl · ↑ · ↓** (also ← → if there is room;
     44 px keys), sticky at the bottom of the pane. Ctrl is a latch (next key is sent with Ctrl, then it releases); each key
     writes through the pane's existing input path (`sendInput`, `terminal-tab.tsx:64`) so paste and chunking are unchanged. Hidden
     at 640 px and above and when the pane is not connected. "Open in new window" is hidden on phones (item 2).

## Out of scope

- The backend (P5-REFINE-4): runner re-registration fix, keep-awake from terminal/SSH, phases, Sleep now route, `aurora ssh
  <handle>` mapping, member-safe fields.
- Files in chat ("Dani saved report.md · Open · Download", rec 11 third bullet) and the timeline's computer steps: P5-REFINE-9
  and P5-REFINE-11.
- The header's computer status and the tab bar (P5-REFINE-11). Previews content (unchanged). The BYO "Connect" page.
- Command palette and shortcuts on this page (P5-REFINE-19).

## Acceptance criteria

1. **Files first:** a member with Files on opens Computer and sees the state line and the file list, with the groups **Your files**
   and **Dani's own**, matching `UXR5/after/computer-9.png` (dark) and `computer-9-lt.png` (light) at 1256 and 390 px. Terminal,
   Connect and Previews appear only with the switch on.
2. **States:** the five phases read Off, Starting, On, Stopping, Failed and nothing else in prose (vitest per phase; a grep
   guard for `waiting_supervisor`, `stopped`, `unprovisioned` outside Technical details and Expert). `computer-13.png`/`-lt.png`
   match (after P5-REFINE-4).
3. **Terminal:** after the last retry (mocked websocket, five 1006 closes) the pane shows "Couldn't connect to the computer",
   **Try again** works, typing is not sent, no cursor blink; matches `computer-2.png`/`-lt.png`. The restart line prints once on a
   reconnect that the API marks as a new shell (after P5-REFINE-4) and matches `computer-1.png`/`-lt.png`.
4. **Files failed state:** Upload and New file disabled with the line; Try again reloads. **Move to…** moves a file into a folder
   (assert the `rename` request has `to` = `/workspace/docs/fruit.csv`) and drag onto a folder does the same; matches
   `computer-3.png`/`-lt.png`.
5. **Phone files** at 390 px: stacked rows, the size and the menu do not overlap (bounding boxes), matches `computer-5.png`/`-lt.png`.
6. **Toasts** for upload, rename, delete, download, move and Save; the editor does not move when "Saved" shows (assert the
   editor's top does not change); the uploads panel collapses when done; matches `computer-10.png`/`-lt.png`.
7. **Preview:** a 3.4 MiB text file shows the first 1 MiB with the note; a CSV shows a table (cells escaped: a `<b>` cell is text);
   matches `computer-11.png`/`-lt.png`.
8. **Editor:** the dot appears on change and goes on Save; Save disabled when clean; leaving with changes asks (all three buttons
   tested); matches `computer-4.png`/`-lt.png`.
9. **SSH:** the Aurora command is first; the certificate line counts down; reload keeps Download until expiry (sessionStorage);
   the command uses `aurora-<handle>-cert.pub`; matches `computer-6.png`/`-lt.png`.
10. **Technical details:** as a member the page's DOM contains no image name, runner name, IP or raw phase (assert with the API view
    carrying them and with it not carrying them); as an instance admin the Admin-only block shows them; values update when the
    mocked sandbox changes from starting to running; matches `computer-7.png`/`-lt.png`.
11. **Palette test:** every `--aurora-term-color-*` has ≥ 4.5:1 on its pane background in both themes; the phone key row sends
    Esc, Tab, Ctrl-latch and arrows (mock the socket); "Open in new window" is not rendered at 390 px; matches
    `computer-8.png`/`-lt.png` and `computer-12.png`/`-lt.png`.
12. No horizontal scroll at 390 px on any sub-page, axe with no rule disabled, no console errors, both themes. No snake_case or
    UUID in visible text outside Technical details and Expert (the labels guard of P5-REFINE-2).
13. Screens in the completion report: each key's Before crop beside the new screenshot and the After mockup.

## Tests

Targeted only (GitHub CI is the full gate). Heavy commands through
`/home/alex/orca/workspaces/aurora/heavy.sh <lane> <command>`.

- vitest: computer page states and the sub-nav by altitude, the state line per phase, details by role, files grouping
  (`AGENT_OWN_ENTRIES`), Move dialog and the `rename` payload, the file viewer's dirty and leave dialog, CSV table, the large-file
  preview, the SSH page (countdown, sessionStorage, command text), the terminal's end state and key row, the palette contrast test.
- Playwright (mocked; extend `e2e/files.spec.ts`, `e2e/terminal.spec.ts`, `e2e/ssh.spec.ts`, `e2e/agent-tabs.spec.ts`; new
  `e2e/computer.spec.ts`): both themes at 1280 and 390 px with axe, screenshots under `/tmp/aurora-screens/p5-refine-15/`
  (outside the repo). Do not use `*.prod.spec.ts` (they need a real stack).
- Never use `localhost:4000` or the aurora-dev cluster.

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
   the **whole** web vitest suite once (`pnpm --dir web vitest run`, through `heavy.sh`) before handing over. Production
   specs (`--project production`) can't run in a lane without a real stack: update them by reading, and list them in
   the completion report so the nightly / P5-REFINE-20 runs them.
4. **New behaviour gets new tests** (the brief's Tests section); old tests are **kept and updated**, not replaced.
5. **The completion report lists** every test file changed (and why), every test removed (and the removed feature),
   the commands run with their results, and the production specs that only the nightly will run.
6. A merge that turns master's CI red because a test still expects old UI is a send-back to this lane, not the CI
   Watcher's job.

## Docs to update

- `docs/14-web-dashboard.md` §Agent page (Computer's sub-nav and altitudes), §Terminal tab (end state, restart line, phone keys,
  palette rule), §Files (groups, Move to…, toasts, preview, editor guard), §SSH (certificate state, names), a new line for
  Technical details by role (D-412).
- `docs/PROGRESS.md` row P5-REFINE-15 and `docs/FEATURES.md` rows for Computer, Terminal, Files and SSH, both current at every commit.

## Completion report

Fill in when done (P5-REFINE-20 reads it).

- Items done, with commit: 1 · 2 · 3 · 4 · 5 · 6 · 7 · 8 · 9.
- Items that waited for P5-REFINE-4 and what shipped meanwhile:
- The agent's own files list and where it came from (item 3); how a large text file is read (item 4); the instance-admin field and
  whether the member API view still carried the image/runner (item 7):
- Deviations and why:
- Screenshots compared (Before / After / mine, per finding key, both themes, 1256 and 390 px):
- Open questions:
