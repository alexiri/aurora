# P5-REFINE-3: Altitudes Use / Set up / Expert with a per-person "Show more controls" switch (owner, 2026-11-20)

## Goal

Every person meets the whole control surface at the same height: a nine-item rail, an agent with sixteen
sub-pages, a schedule that is a cron string, a Computer tab that is a sandbox report. "Simple" today means
"moved under a … menu or an Advanced fold", page by page (five different `<details>` "Advanced", three
"Technical details", and a lot not folded at all: Evals, Peers, SSH, BYO, cron, `Gate command`, raw model
ids). Role-based visibility removes whole pages (`lib/api/roles.ts visibleTopics`, C-961) but never the
depth inside a page.

This brief gives disclosure **one home** (rec 1, D-406):

- three **altitudes**, and every control belongs to exactly one;
- **role sets the floor** (extended from topics to sections inside pages);
- **one per-person "Show more controls" switch**, stored server-side next to `welcome_seen`, so it follows
  the person across browsers;
- one primitive, `<Altitude>`, and a classification table (below) that is also data in code and in `docs/14`.

Later briefs use `<Altitude>` for the controls they add. This brief applies it to the pages that **exist
today**.

Decisions that bind this brief: **D-405**, **D-406** (three altitudes, per-person switch, role is the floor).

## Linked requirements

- **The review:** <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt>. Workers cannot open artifacts; read the
  local copy `/home/alex/orca/workspaces/aurora/design-ux-review-5/` (`UXR5/` below).
  - `UXR5/layer-above.md`, **rec 1** (Three altitudes, one switch; the table and the Behaviour sketch),
    rec 16 (blind spots: Evals for every member, Previews and Connect page ids, Redact, Memory language,
    Groups and directory sync, harness names), "The first ten minutes", "Open questions" (altitude switch
    decided: per person). Anchor <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt#above>.
  - Screens that show the problem (Before): `UXR5/screens/B018-agent-home.png`, `B183-phone-menu.png`,
    `B017-agents-list.png`, `C002-agents.png`, `C047-computer.png`, `C112-ssh.png`, `C036-new-agent.png`,
    `B154-activity.png`, `C025-webhook-deliveries.png`.
  - No per-finding mockup exists for this brief. The visual of "n more controls hidden · Show" is defined
    here (Scope 4); the pages it appears on have their mockups in briefs 11–18. Where a mockup of a later
    brief shows an Expert control, that brief wraps it with `<Altitude>`.
- **Contract:** **C-1056** (the switch on `GET`/`PATCH /v1/me/onboarding`; migration
  `20261201000260`). The draft entry is in
  `docs/API_CONTRACTS.md` › “Phase 5 — P5-REFINE” (merged with the briefs, 2026-11-20);
  it is already in `docs/API_CONTRACTS.md` (the implementer corrects it there first if needed; heading exists, so ignore: if
  absent). Related: C-530/C-531 (onboarding), C-961 (visible topics).
- **Decisions:** D-405, D-406. **Security:** the switch changes only what the dashboard **shows**. It never
  authorizes anything; every route keeps its own check (no SEC change).
- **Spec:** `14` §Information architecture › "Who sees what"; §Onboarding (`welcome_seen`).

## Wave and order

- Wave 1, **lane C** (small Rust + web), starts immediately. First brief of lane C: 3 → 12 → 16.
- Disjointness: owns `lib/api/roles.ts`, the new `lib/altitude*.ts`, `components/ui/altitude.tsx`, the
  account-menu item in `app-shell.tsx`, the onboarding repo/service/dto files, and the **wrapper edits** in
  the files listed in Scope 5. Brief 1 (strings) merges first; brief 2 provides `TechnicalDetails`, which
  uses `<Altitude>` once both are on master (whichever merges second makes that one-line change).
- Briefs 11–18 wrap their new Expert controls with `<Altitude>` and add rows to the registry.

## Context

Paths under `web/src/` unless noted; line numbers are master `c0fe7d10`.

- **Server (Rust):**
  - `crates/aurora-db/migrations/20261129000001_user_onboarding.sql` (the table); latest migration is
    `20261201000259_person_timezone_link_handle.sql`; this brief adds `20261201000260_user_more_controls.sql`.
  - `crates/aurora-db/src/repo/onboarding.rs`: `OnboardingRow` :11–18, `get` :20–33, `Patch` :84–92,
    `apply` :95 (an upsert with `COALESCE($n, column)`).
  - `services/aurora-api/src/domains/onboarding/dto.rs`: `MeOnboarding` :9–21, `PatchOnboarding` :23–33
    (`deny_unknown_fields`); `service.rs`: `view` :17–35 (a person with no row), `get_me` :37, `patch_me`
    :42.
  - Tests: `services/aurora-api/tests/it/onboarding.rs`. OpenAPI: `web/openapi.json` (checked-in; test
    `openapi_matches_checked_in` in `services/aurora-api/tests/it/foundation.rs` :541–551), types
    `web/src/lib/api/schema.d.ts` (`pnpm gen:api`).
  - Docs: `docs/03-data-model.md` `user_onboarding` (:175), `docs/05-control-plane-api.md` :524.
- **Web client:** `lib/api/onboarding.ts` (`useMeOnboarding`, `usePatchOnboarding`, `onboardingKeys.me`);
  `lib/onboarding/use-guide.ts` (the pattern for server-side per-user flags).
- **Roles:** `lib/api/roles.ts` (`ViewerRoles`, `visibleTopics` :17–40; org member sees
  `workspaces, ai-models, internet-access`; workspace role gives overview, agents, skills, apps,
  internet-access, approvals (owner/approver), activity, people+settings (owner)). Used by
  `components/app-shell.tsx` (`OrgNav` :213–227, `WorkspaceNav`, admin nav) and `lib/nav/*`.
- **Account menu:** `components/app-shell.tsx` :120–175 (items: Your account, Administration, Show me
  around, What moved [removed by brief 1], Sign out).
- **Existing folds and gates** the wrapper replaces or uses:
  - `components/agents/sections.ts` `isPageAvailable` :91–100 and `agent-page.tsx` :95–99 (availability of
    Terminal, Files, Previews, SSH, Connect, Evals, Peers; `byo`);
  - `components/agents/instructions-tab.tsx` :216 (`<details>` Advanced: model roles, effort, harness);
    `components/agents/model-roles-fields.tsx`;
  - `components/agents/workspace-byo.tsx` (the BYO row, used by `agents/agents-view.tsx` :39);
    `components/ui/scope-ladder.tsx` :36–70 (`collapsed` puts "Tighten…" behind a button);
    `components/workspace/workspace-topics.tsx` :72;
  - `components/agents/create-agent-dialog.tsx` :33 (kind defaults to `"byo"`; brief 12 changes the
    default, this brief only makes the kind select Expert);
  - `components/schedules/schedules-tab.tsx` :118–125 (cron field, Gate command), `:82` anchored,
    `components/automations/owner-controls.tsx`;
  - `components/webhooks/webhooks-tab.tsx` (deliveries rows, delivery ids);
  - `components/approvals/activity-view.tsx` (the nine-column request log);
  - `components/admin/settings/settings-index-view.tsx` :113 (YAML export/import);
  - `components/chat/message-group.tsx` :126 (Redact), `components/create-workspace-form.tsx` :92 and
    `components/settings/general-tab.tsx` :57 (Memory language);
  - `components/org/org-topics.tsx` :171–177 (Groups, Directory sync tabs);
  - `components/accounts/account-row.tsx` :283 and others ("Technical details");
  - `components/agents/status-rail/parts.tsx` :299 and the agents list (Default badge).
- **Words test:** `web/tests/ui-text.test.ts`.

## The classification table (rec 1, resolved)

Three altitudes. **Use**: everyone who may use the thing. **Set up**: the default for owners and admins, and
for anyone who turns on More controls. **Expert**: only with More controls on (and instance admins inside
Administration, where Expert is on by default). The role decides whether the person may see the control at
all; the switch can only widen up to that role (a member with the switch on does **not** get owner-only
controls).

"Now" = this brief wraps it on the page that exists today. "Later: brief N" = the control is created or
rebuilt by brief N, which wraps it using the registry id given.

| Registry id | Control | Altitude | Where today | Now / Later |
|---|---|---|---|---|
| `chat` | Chat, Approvals, Overview, Give an app, an account's access choice, Files, the agent's summary and timeline | **Use** | everywhere | no wrapper |
| `rail.skills`, `rail.apps`, `rail.internet`, `rail.activity`, `rail.people`, `rail.settings` | rail topics other than Overview, Agents, Approvals | **Set up** | `lib/api/roles.ts` | Now (Scope 2) |
| `rail.platform`… | Administration topics | **Expert default for instance admins** | admin nav | Now (no hiding; marks the rule) |
| `agent.instructions` | Name, Instructions, Model (the C3 card) | **Use** (the card is the Behaviour sketch's Use column) | `instructions-tab.tsx` | no wrapper |
| `agent.memory`, `agent.skills` | Memory, Skills pages | **Set up** | `sections.ts` | Now (Scope 3) |
| `agent.tests` | Tests (Evals) page | **Expert** | `sections.ts`, `evals-tab.tsx` | Now |
| `agent.otheragents` | Other agents (Peers) page | **Expert** | `sections.ts`, `peers-tab.tsx` | Now |
| `agent.terminal`, `agent.ssh`, `agent.previews` | Terminal, SSH, Previews pages | **Expert** | `sections.ts` | Now (Files stays Use) |
| `agent.connect` | Computer: Connect (BYO only) | **Expert** | `sections.ts` | Now |
| `agent.byo` | BYO as a kind of agent: the kind select in New agent, the BYO row on the Agents list | **Expert** (the row: **Set up**) | `create-agent-dialog.tsx`, `workspace-byo.tsx` | Now |
| `agent.roles`, `agent.effort`, `agent.harness`, `agent.program` | Model roles, Subagents use, Effort, Harness / agent program (the "Advanced" fold) | **Expert** | `instructions-tab.tsx` :216 | Now |
| `agent.proactive` | "<Agent> may message you first" | **Set up** | `instructions-tab.tsx` | Now |
| `agent.handle` | the agent's @handle line | **Expert** | agent header | Now |
| `policy.restrict` | "Restrict further…" (was Tighten…) and "Set for you by" ladder detail | **Set up** | `scope-ladder.tsx`, `workspace-topics.tsx`, `internet-level.tsx` | Now |
| `policy.try` | "Try a request" | **Set up** | `internet-level.tsx` | Now |
| `policy.presets` | Open / Balanced / Locked down | **Set up** | — | Later: brief 18 |
| `auto.cron` | the raw cron field | **Expert** | `schedules-tab.tsx` :118 | **Later: brief 14** (presets exist then; wrapping it now would leave no way to schedule) |
| `auto.gate`, `auto.anchor` | "Only run when…", "Keep running if I leave" | **Expert** | `schedules-tab.tsx`, `owner-controls.tsx` | Now |
| `auto.curl`, `auto.signature` | webhook "How to call it" detail, Copy as curl, signature recipe | **Expert** (the panel itself is Set up) | — | Later: brief 14 / 19 |
| `auto.deliveryids` | delivery ids, raw payload sizes | **Expert** | `webhooks-tab.tsx` | Now |
| `auto.verification` | the verification choice cards (Signed / Password / Anyone) | **Set up** (default Signed) | — | Later: brief 14 |
| `log.requests` | the request-log table (nine columns, `default_denied`…) | **Expert** | `activity-view.tsx` | Now (the timeline replaces it later: brief 9/16) |
| `tech.details` | every "Technical details" fold | **Set up** and above; never for plain members | account row, actions table, app review, Computer | Now (via brief 2's `TechnicalDetails`) |
| `export.yaml` | YAML/JSON export and Import of settings | **Expert** (Administration: visible) | `settings-index-view.tsx` | Now |
| `export.agent` | Export / Import an agent | **Expert** | — | Later: brief 19 |
| `ws.memlang` | Memory language ("Language <Agent> remembers in") | **Expert** | `create-workspace-form.tsx`, `general-tab.tsx` | Now |
| `chat.redact` | "Remove this message's content…" | **Expert** | `message-group.tsx` :126 | Now |
| `org.groups`, `org.directory` | Groups, Directory sync tabs under People | **Expert** unless a provider is configured (then Set up) | `org-topics.tsx` :171–177 | Now |
| `apikeys` | Account › API keys explained | **Set up** | `settings/keys-tab.tsx` | Now |
| `palette.verbs`, `kbd`, `bulk` | command-palette verbs, keyboard shortcuts, bulk actions | **Expert** (palette nouns: **Use**) | — | Later: brief 19 |
| `fold.cli` | "From your terminal" CLI/API snippets | **Expert** | `ssh-page.tsx` :87 and others | Now (SSH snippet) / Later: 14, 15, 19 |

If a control is not in this table, the implementer asks (it is a gap in the classification), not guess. The
same table is the first rows of `lib/altitude-registry.ts` (Scope 1) and goes into `docs/14`.

## Scope

1. **Server: the switch (C-1056).**
   - Copy the C-1056 entry from the contracts draft into `docs/API_CONTRACTS.md` (and the `03` and `05`
     rows) **first**.
   - Migration `crates/aurora-db/migrations/20261201000260_user_more_controls.sql`:
     `ALTER TABLE user_onboarding ADD COLUMN more_controls boolean NOT NULL DEFAULT false;`.
   - `repo/onboarding.rs`: `OnboardingRow.more_controls`, `get`/`apply`/RETURNING list, `Patch.more_controls:
     Option<bool>`; in the upsert, the INSERT branch uses `COALESCE($n, false)`, the UPDATE branch
     `COALESCE($n, user_onboarding.more_controls)`.
   - `dto.rs`: `MeOnboarding.more_controls: bool` (documented), `PatchOnboarding.more_controls:
     Option<bool>`. `service.rs`: `view` maps it; `None` row → `false`; `patch_me` passes it.
   - Tests in `services/aurora-api/tests/it/onboarding.rs`: default `false` for a person with no row; PATCH
     `true` persists and is returned by GET; PATCH with only `welcome_seen` leaves it; unknown field → 400;
     one person's value never changes another's.
   - Regenerate `web/openapi.json` (`UPDATE_OPENAPI=1 cargo test -p aurora-api --test it
     openapi_matches_checked_in`) and `web/src/lib/api/schema.d.ts` (`pnpm gen:api`). Run
     `python3 scripts/extract-schema.py --check` and `scripts/guards/run-all.sh`.
2. **Web: the altitude model.**
   - `lib/api/roles.ts`: add `type Altitude = "use" | "setup" | "expert"`, `viewerFloor(roles): Altitude`
     and `altitudeAllows(item: Altitude, floor: Altitude, more: boolean): boolean`.
     - **Floor:** instance admin (anywhere) → `setup` (in Administration → `expert`); organization owner or
       admin → `setup`; workspace owner of a **team** workspace → `setup`; workspace owner of the
       **personal** workspace, approver, member, org member → `use`. *(Flagged decision: rec 1's table says
       "workspace owners" are Set up, but its "first ten minutes" shows Bob, owner of his personal
       workspace, with a rail of Overview, Agents, Approvals only. This brief follows the walk-through:
       a person's own personal workspace starts at Use. The Orchestrator confirms with the owner; the
       change is one line in `viewerFloor`.)*
     - **With `more` on:** `expert` for everyone, **capped by role**: an item whose caller-passed
       `allowed` is false stays hidden regardless.
   - `visibleTopics` becomes altitude-aware: new optional parameter `{ floor, more }`. At `use` the
     workspace rail lists Overview, Agents, Approvals (if the role may approve). Skills, Apps & accounts,
     Internet access, Activity, People and Settings need `setup`. Organization member: Workspaces only at
     `use`; AI models and Internet access at `setup`. Administration topics are all visible to instance
     admins (their floor is expert there). Direct URLs to a hidden topic **still work** (altitude hides
     links, it never blocks a route); the page shows its normal content.
   - `lib/altitude-registry.ts`: the table above as data, `Record<id, { altitude, area, later?: string }>`.
   - `lib/api/use-more-controls.ts`: `useMoreControls()` returns `{ more, setMore, pending }` over
     `useMeOnboarding`/`usePatchOnboarding`. Optimistic update; on error revert and show the standard error.
     While the first load is pending `more` is `false` (no flash of Expert controls).
   - `components/ui/altitude.tsx`: `<Altitude id level="setup"|"expert" allowed?=true label? count?>` renders
     children when `altitudeAllows(...)`; otherwise it **registers one hidden control** with the page's
     `AltitudeScope` and renders the in-place collapsed line when `label` is set (below). Props:
     `promote?: boolean` (**contextual promotion**: when true the children render at any altitude, for the
     case "a chat error about the AI key shows the key fix; a failed webhook test shows the receiver's
     status"; the caller passes the condition). Unknown `id` → throws in tests, ignored in production
     (the registry test makes it a build-time error).
   - `<AltitudeScope>` (page-level context) counts hidden controls; `<MoreControlsLine />` renders
     **"3 more controls hidden · Show"** (the page's footer; "1 more control hidden · Show"; nothing when
     zero). **Show** turns the per-person switch on (the same `setMore(true)`), it does not open only this
     page's folds. When the switch is on, the same place reads **"Showing more controls · Hide"**.
3. **Apply on the pages that exist today** (every row marked "Now"; no page is redesigned):
   - **Agent page** (`sections.ts`, `agent-page.tsx`): `isPageAvailable` takes the viewer's `altitude`;
     Memory and Skills need `setup`; Tests (`evals`), Other agents (`peers`), Terminal, SSH, Previews,
     Connect need `expert`. Hidden pages are not in the tab/menu lists, and a direct `?tab=` for one still
     opens it (the page renders; only navigation is hidden). The agent page footer shows
     `<MoreControlsLine />`. Do not restructure the tab bar (brief 11).
   - **Instructions** (`instructions-tab.tsx`): the "Advanced" `<details>` becomes
     `<Altitude level="expert" id="agent.roles" label="Model roles, effort and agent program">` (the
     collapsed line replaces the fold at Use/Set up); "may message you first" is `setup`.
   - **Agents list and New agent:** the BYO row (`workspace-byo.tsx`) is `setup`; the kind select in
     `create-agent-dialog.tsx` is `expert` (the dialog always creates a hosted agent when the select is
     hidden; the default change is brief 12's).
   - **Internet access** (`scope-ladder.tsx`, `workspace-topics.tsx`, `internet-level.tsx`): "Restrict
     further…" and "Try a request" are `setup` (a plain member does not see them; they read the sentence
     "Set for you by …" only).
   - **Schedules and webhooks:** Gate and Keep-running controls, delivery ids: `expert`. The raw cron
     field **stays visible** until brief 14.
   - **Activity:** the request-log table is `expert`; a person at lower altitudes sees the collapsed line.
   - **Technical details:** brief 2's `TechnicalDetails` is wrapped as `setup`; if brief 2 has not merged,
     wrap the five existing folds directly.
   - **Settings:** Memory language `expert`; Redact `expert`; YAML export `expert`; Groups and Directory sync
     tabs `expert` unless a provider is configured; the SSH CLI snippet `expert`.
   - Each wrapper uses the registry id from the table. Rows marked "Later" are **not** wrapped here.
4. **The switch in the UI.**
   - **Account menu** (`app-shell.tsx`): a menu item **"Show more controls"** with a switch on its right
     (`MenuCheckboxItem` or a `Switch`), above "Show me around". It toggles `setMore` and does not close
     the menu. Label while on: "Show more controls" with the switch on.
   - **Account › Profile** (`account/profile-view.tsx`): a row "Show more controls" with a `Switch` and the
     sentence "Shows Terminal, Tests, model settings and other expert controls. You only see what your
     role allows."
   - **Bottom of pages that hide something:** `<MoreControlsLine />` as above; it sits at the end of the
     page content in muted text, with **Show** as a link-styled button. It is also the only place that says
     the count.
   - **One-time hint** (rec 1 risk): after the person has opened three of the in-place collapsed lines (a
     counter in `localStorage` `aurora.altitude.opened`, try/catch), show once a small notice "Want
     everything? Turn on Show more controls." with a **Turn on** button and a dismiss; store the dismissal
     in `localStorage` too.
   - Phone: the switch is in the account menu sheet; the footer line wraps; no horizontal scroll at 390 px.
5. **The altitude lint.** `web/tests/altitude-registry.test.ts`: (a) scans `src/components` for
   `<Altitude` and fails when an `id` is missing from the registry or the wrapper's `level` differs from the
   registry; (b) fails when a registry row marked "Now" (no `later`) has no `<Altitude` usage; (c) renders the
   pages touched here with `more=false` at the `use` floor and asserts the **Expert strings are absent**
   ("Model roles", "Tests", "Other agents", "Terminal", "Gate", "Language", "Remove this message's
   content", the request-log caption) (rec 1's grounding: "assert that Expert strings never render at Use").
6. **Docs.** `docs/14` new section **Altitudes** (the three definitions, the floor rule including the
   personal-workspace line, the switch, the `<Altitude>` primitive, the classification table, the contextual
   promotion rule, the lint), and a line in §Information architecture › "Who sees what" and §Onboarding
   (`more_controls`).

## Out of scope

- Restructuring the agent page, the Settings button, Automations list, Computer pages (briefs 11, 12, 14, 15).
- The raw cron wrapper, presets, verification choices, Export/Import, palette, shortcuts, bulk actions
  (briefs 14, 19).
- Making any route unreachable. Altitude hides navigation and depth, never authorization.
- A server rule that uses `more_controls`.

## Acceptance criteria

1. `GET /v1/me/onboarding` returns `more_controls: false` for a new person; `PATCH { more_controls: true }`
   persists it and the next `GET`, from another session, returns `true`; the Rust integration tests above
   pass; `openapi_matches_checked_in` passes; `extract-schema.py --check` and the guards pass.
2. As a member (or a person in their personal workspace) with the switch **off**: the workspace rail lists
   Overview, Agents, Approvals only; an agent's pages show no Terminal, SSH, Previews, Tests, Other agents,
   Memory, Skills; Instructions shows Name, Instructions, Model and one collapsed line "Model roles, effort
   and agent program"; Internet access shows the sentence without "Restrict further…" or "Try a request";
   Activity shows no request-log table; the page footer reads "n more controls hidden · Show".
3. Pressing **Show** (or the account-menu switch) reveals every Expert control **in place**, the footer reads
   "Showing more controls · Hide", the state survives a reload and a sign-in from another browser.
4. A team-workspace owner, an organization admin and an instance admin see Set up controls by default;
   an instance admin sees Expert controls in Administration by default.
5. A member with the switch **on** does not see an owner-only control (e.g. the workspace Settings danger
   zone), checked in a test.
6. A direct link to `?tab=evals` still opens the Tests page for a person at Use; it is just not in the
   navigation.
7. `<Altitude promote>` shows its children at Use (test with a failing-key fixture).
8. The altitude lint (Scope 5) passes and fails when a classified control is rendered at Use.
9. No horizontal scroll at 390 px on the touched pages, in both themes; axe passes.
10. The classification table in this brief, `lib/altitude-registry.ts` and `docs/14` §Altitudes list the same
    ids and levels.

## Tests

Targeted only; heavy commands through `/home/alex/orca/workspaces/aurora/heavy.sh <lane> <command>`; GitHub
CI is the full gate.

- Rust: `cargo test -p aurora-api --test it onboarding` and `openapi_matches_checked_in`;
  `cargo fmt --check`; clippy on `aurora-db` and `aurora-api` only.
- Vitest: `roles` (floor, `visibleTopics` per altitude and role), `altitude` (primitive, scope count,
  promote, optimistic switch, error revert), `account-menu` (switch), `agent-page`/`sections` (availability
  per altitude), `instructions-tab`, `scope-ladder`, `activity-view`, `altitude-registry`.
- Playwright (mocked): one spec, as a member then as an owner, switch off then on, in both themes at 1256 and
  390 px, axe, no console errors.

## Docs to update

- `docs/API_CONTRACTS.md` C-1056 (+ P5-REFINE heading), `docs/03-data-model.md`, `docs/05-control-plane-api.md`.
- `docs/14-web-dashboard.md` §Altitudes (new), §Information architecture, §Onboarding.
- `docs/PROGRESS.md`, `docs/FEATURES.md` rows.

## Suggested agent/model and reason

Sonnet (`rust-developer` for Scope 1, then `nextjs-developer`; or one Sonnet lane). A small migration and a
cross-cutting UI primitive; the classification must follow the table exactly.

## Completion report

- **Items done** (Scope 1–6): Scope 1 (server) in `e4b82614`: migration `20261201000260`, repo/dto/service, IT
  `more_controls_defaults_off_persists_and_is_per_person`, `web/openapi.json`, `schema.d.ts`,
  `spec_schema.json`, docs `03`/`05`. Scopes 2–6 (web) in the lane's second commit: `lib/api/roles.ts`
  (`Altitude`, `viewerFloor`, `altitudeAllows`, altitude-aware `visibleTopics`), `lib/altitude-registry.ts`,
  `lib/altitude.tsx` (`useAltitude`, `AltitudeScope`, counters/hint), `lib/api/use-more-controls.ts`,
  `components/ui/altitude.tsx` (`<Altitude>`, `<MoreControlsLine>`), `MenuSwitchItem` in `ui/menu.tsx`, the account
  menu row, Account › Profile › Controls, the wrappers below, the lint, `docs/14` §Altitudes.
- **Contract C-1056 entry**: it was already in `docs/API_CONTRACTS.md` (merged with the briefs, 1e1fef73); no
  correction was needed. The `03` row and the `05` text are in `e4b82614`.
- **Classification table check**: "Now" ids → files: `agent.memory/skills/tests/otheragents/terminal/ssh/previews/
  connect` → `agents/sections.ts` (`PAGE_ALTITUDE`) + `agent-page.tsx`; `agent.roles` (and `effort/harness/
  program`, one group) and `agent.proactive` → `instructions-tab.tsx`; `agent.handle` → `agent-page.tsx`;
  `agent.byo` → `create-agent-dialog.tsx` (Expert, kind select) and `agents-view.tsx` (Set up, BYO row; the
  registry row carries `also: "setup"`); `policy.restrict` → `ui/scope-ladder.tsx`; `policy.try` →
  `internet/internet-level.tsx`; `auto.gate` → `schedules/schedules-tab.tsx`; `auto.anchor` →
  `automations/owner-controls.tsx` (both tabs) and the schedule form; `auto.deliveryids` →
  `webhooks/webhooks-tab.tsx`; `log.requests` → `approvals/activity-view.tsx`; `tech.details` → `account-row`,
  `actions-table`, `app-setup/sections`, `computer-tab` (the four folds that exist; brief 2's `TechnicalDetails`
  is not on this branch); `export.yaml` → `admin/settings/settings-index-view.tsx`; `ws.memlang` →
  `create-workspace-form.tsx`, `settings/general-tab.tsx`; `chat.redact` → `chat/message-group.tsx`;
  `org.groups`/`org.directory` → `org/org-topics.tsx` (tab list, via the hook); `apikeys` →
  `account/my-keys-view.tsx` (the explanatory sentence); `fold.cli` → `agents/ssh-page.tsx`. `rail.*` →
  `visibleTopics` (+ `app-shell.tsx`); `rail.platform` marks the rule. "Later": `policy.presets` → P5-REFINE-18;
  `auto.cron`, `auto.curl`, `auto.verification` → P5-REFINE-14; `auto.signature`, `export.agent`,
  `palette.verbs`, `kbd`, `bulk` → P5-REFINE-19 (the registry's `later` names them; the doc table says
  "14 / 19" for `auto.curl`).
- **Deviations and why**:
  - Personal-workspace floor as the brief flags: the owner of a personal workspace starts at Use (one line in
    `viewerFloor`). Personal is read from `personal_of` on the workspace or, until the org's workspace list
    answers, assumed (so nothing deeper flashes in).
  - The brief's lint (c) asserts "Model roles" is absent, but the collapsed line's label is "Model roles, effort
    and agent program" (acceptance 2); the tests assert the controls (Harness, Effort, the role selects) instead.
  - `<Altitude id="agent.byo">` is wrapped at two levels (kind select Expert, BYO row Set up): the registry row has
    `also: "setup"` and the lint accepts either.
  - Registry rows with `mark: true` (rail topics, `agent.effort/harness/program`) have no wrapper of their own;
    the lint's "used in code" check skips them. `org.groups`/`org.directory` are used through the hook (tabs are
    data), so the ids are literals in `org-topics.tsx`.
  - A schedule that already has a gate command, and a field the server rejects, is promoted (shown) so a hidden
    setting never silently keeps working or fails. `useAltitude`/`useMoreControls` are inert (Use / off) without a
    QueryClient, so a control rendered alone in a unit test does not need one.
  - The Playwright mock now answers `more_controls: true` by default so the existing specs see the whole
    surface; `e2e/altitude.spec.ts` overrides it (not run here, by the owner's rule).
  - "What moved" is still in the account menu: brief 1 (strings) removes it; the switch row sits above Show me
    around as specified.
  - Directory tabs: a hidden tab list also hides a direct `?tab=groups` request on People (the brief says routes
    stay open; Groups falls back to Members). Pages and topics follow the brief.
- **Screenshots compared**: none taken (no running dashboard on this box for this lane; Playwright not run by
  rule). The footer line and the account-menu row are covered by vitest (`altitude.test.tsx`,
  `context-switcher.test.tsx`) and the unrun `e2e/altitude.spec.ts` (both themes, 1256 and 390 px, axe).
- **Tests run**: `cargo test -p aurora-api --test it onboarding` 5 passed (incl. the new one);
  `openapi_matches_checked_in` passed; `cargo fmt --check` clean; `cargo clippy -p aurora-db -p aurora-api
  --all-targets -D warnings` clean; `extract-schema.py --check` and `scripts/guards/run-all.sh` pass; `pnpm typecheck`
  clean; eslint on touched files: 0 errors (1 existing warning in `account-row.tsx`); vitest: 44 files, 411 tests
  passed (roles, altitude, altitude-pages, altitude-registry, agent-sections, scope-ladder, model-roles-instructions,
  agents, context-switcher, internet-pages, ui-text and every touched component's test files). Test support:
  `tests/support/render.tsx` `wrap` seeds the switch on by default (`moreControls: false` tests Use).
- **Open questions**: (1) personal-workspace floor (the owner confirms; one line). (2) Waiting: `TechnicalDetails`
  wrapper after P5-REFINE-2 merges (whichever merges second makes the one-line change); the "Later" rows above.
  (3) Playwright `altitude.spec.ts` needs a first run in CI; other specs that use their own onboarding fixture
  (not the shared mock) may need `more_controls: true`.
