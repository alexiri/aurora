# P5-LAYOUT-5: Bring-your-own agents back in Settings, and only when there is a choice (owner, 2026-11-21)

## Goal

The owner, on the workspace Agents page: "the things that are set for me and I can't do anything about aren't useful
here, hide them. Why isn't the BYO configuration in settings?"

Today the top of the Agents page carries a scope-ladder block: a caps heading "Set for you by an Aurora administrator", a
card "Bring-your-own agents · [lock] Set for you by an Aurora administrator · Nobody can create BYO agents", and a
**Restrict further…** button. In the owner's workspace the instance value is already the strictest ("Nobody can create
BYO agents"), so Restrict further… opens a select with only "Same as the organization": the block offers nothing to do.
It is there because P5-IA-4 (C-967, D-340 "settings live in their topics") moved the workspace's BYO setting from
Settings › General to the Agents list header.

After this brief:

- The Agents page has **no BYO block**: the page header, then the agents.
- The workspace's BYO choice is a field in **Settings › General**, saved with the rest of the form, shown **only to
  workspace owners** and **only when they can choose something**: when a stricter value than the level above exists,
  or when the workspace has its own value that can be cleared. Otherwise Settings shows nothing about BYO.
- The limit from above is said in the field's hint, in one sentence, not as a locked ladder row.

## Linked requirements

- **Decision:** D-442 (BYO in Settings › General; a workspace page does not show a limit from above that the viewer can
  neither change nor tighten). Amends C-967's workspace part and D-340 for this one setting; `14 §Scope ladder` keeps the
  ladder for Internet access, AI keys and rules, Data › Retention and app availability.
- **Contract:** C-967 (workspace) amended in place: "the workspace BYO setting is a field of Settings › General, shown to
  owners when there is a choice (D-442)". No API change: `PATCH /v1/workspaces/{ws}` with `byo_posture` (`null` =
  inherit) as today; the server already refuses a looser value (`domains/tenancy/service.rs` `set_byo_posture`, 422
  `governance_below_instance_minimum`).
- **Altitude:** `docs/14` §Altitudes table row `agent.byo`: "the BYO row on the Agents list (Set up)" becomes "the BYO
  field in Settings › General (Set up)".
- **Feature row:** `P5/14§layout-quiet` in `docs/FEATURES.md`.

## Wave and order

- **Lane B**, first: **5** → 4. Lane A runs P5-LAYOUT-1 → 2 → 3 beside it.
- P5-LAYOUT-3 changes how `agents-view.tsx` renders the list (props to `AgentsList`); this brief removes the BYO block
  from the same file. Whichever merges second rebases.
- P5-LAYOUT-1 may change the General tab's wrapper class; same rule.

## Context

All paths under `web/src/`, master 55b1bdb8.

- `components/agents/agents-view.tsx` :50–55: `<Altitude id="agent.byo" level="setup"><WorkspaceByoRow …/></Altitude>`.
- `components/agents/workspace-byo.tsx`: `WorkspaceByoRow` (owners: `OwnerByoRow` reads the organization's value with
  `useEffectiveGovernance(org_id)`, falling back to `workspace.effective_byo_posture`); `ByoRow` renders a `ScopeLadder`
  (`components/ui/scope-ladder.tsx`, `collapsed`, "Restrict further…") with a `Field "In this workspace"` select
  (`"" = Same as the organization (<value>)` plus `byoOptionsFrom(above.value)` minus the above value), its own
  `SaveBar`, and "This workspace is stricter: …". `BYO_WORDS` holds the three sentences.
- `lib/api/governance.ts:140` `byoOptionsFrom(floor)`: the postures at or stricter than `floor`.
  Order: `enabled` < `owners_only` < `disabled`.
- `components/settings/general-tab.tsx`: `GeneralTab` — one `Form` (`max-w-md`), a `fieldset` disabled for non-owners
  with the notice "Only workspace owners can change these settings.", fields Name, Time zone, Memory language (Expert,
  `Altitude id="ws.memlang"`), one `SaveBar` that sends `usePatchWorkspace` with the changed fields; `base` tracks what is
  saved; a workspace changed elsewhere resets the form from `updated_at`.
- Tests: `web/tests/byo.test.tsx` `describe("workspace BYO setting (P5-IA-4, C-967)")` :164–216 ("moved out of Settings ›
  General", "is a ladder row in the agents list header", "a workspace member reads the value as text"); `web/e2e/ia-4.spec.ts`
  :79–90 (Agents page ladder, "Restrict further…", "In this workspace", Settings without the setting);
  `web/e2e/ia-3.spec.ts` and `internet-access.spec.ts` use the same ladder for other settings (not changed here).

## Scope

1. **Agents page:** remove the BYO block (the `Altitude id="agent.byo"` wrapper and `WorkspaceByoRow`). Nothing else on
   the page changes.
2. **Settings › General, owners only:** a field **Bring-your-own agents** after Time zone, inside the same form and saved
   by the same Save (one PATCH with the other changed fields, `byo_posture: value || null`), wrapped in
   `Altitude id="agent.byo" level="setup"`.
   - Options: "Same as your organization (<sentence of the value above>)" (`""`), then each stricter posture as its
     sentence (`BYO_WORDS`).
   - Hint: "Your organization allows: <sentence>. A workspace can only be stricter." When the limit comes from the
     instance, "Aurora's administrators allow: …" (the role, never a person, D-407).
   - The value above comes from the organization's effective governance as today (`useEffectiveGovernance`), falling
     back to `effective_byo_posture`. A server error on the field shows under it (`fieldErrors(...).byo_posture`).
3. **When it is shown:** only if the viewer is a workspace owner **and** (`byoOptionsFrom(above).length > 1` **or** the
   workspace has its own `byo_posture`). In the owner's screenshot (above = "Nobody can create BYO agents", no own value)
   Settings shows no BYO field at all. Non-owners never see it.
4. **The rule behind it (D-442), for this brief's pages:** a workspace page does not draw a limit from above that the
   viewer can neither change nor tighten. Apply it to the BYO setting only; the other ladders are out of scope.
5. **Clean up:** `workspace-byo.tsx` becomes the Settings field (rename to `components/settings/byo-field.tsx` or keep the
   file, your choice; no dead export left). Keep `BYO_WORDS` in one place.
6. **Docs:** `docs/14` §Workspace › Settings (the General bullet: BYO is back, tighten-only, owners, only when there is a
   choice); §Altitudes table `agent.byo`; `docs/API_CONTRACTS.md` C-967 workspace line amended in place with "(D-442)".

## Out of scope

- The organization's and Administration's BYO pages (ladders there stay).
- Hiding other "Set for you by …" rows (Internet access, AI keys, Retention, app availability): FIX-206 (D-444, the
  owner's answer: hidden for members and approvers).
- The New agent dialog's BYO kind (unchanged; it already follows the effective posture).

## Acceptance criteria

- The Agents page shows no "Set for you by an Aurora administrator", no "Bring-your-own agents" and no "Restrict
  further…", for every role.
- In Settings › General, an owner whose organization allows "Members can create BYO agents" sees the field with three
  options' worth of choice (Same as…, Only workspace owners…, Nobody…) and the hint; saving "Nobody can create BYO agents"
  sends one PATCH with `byo_posture: "disabled"`, shows "Saved", and the field keeps the value after a reload.
- An owner whose limit above is "Nobody can create BYO agents" and who has no own value sees no BYO field. With an own
  value left from before, the field shows with "Same as your organization (…)" to clear it.
- A member or approver sees no BYO field.
- Name, Time zone and Memory language save as before; one Save for all.

## Tests

- vitest (`byo.test.tsx`, replacing the P5-IA-4 describe's expectations, not deleting its cases): Agents page has no BYO
  block; General shows the field to an owner with a choice, hides it with no choice, shows it with an own value, hides it
  from a member; saving sends `byo_posture` with the other changed fields; a 422 on `/byo_posture` shows under the field.
- Playwright (`ia-4.spec.ts` :79–90): the Agents page has no ladder; Settings › General has the field for the owner,
  saves it, and reads it back.

## Tests move with the UI (owner, 2026-11-20 — binding)

The owner: "I don't want a ton of broken tests because a button was renamed and the tests look for the old label." In
**every commit** of this brief:

1. **Find every test that sees what you change**: search `web/tests/**`, `web/e2e/**` (mocked **and** `production`
   projects), `scripts/e2e/*.sh`, Rust tests that assert messages or shapes, and fixtures/mocks for the old strings
   ("workspace-byo", "Restrict further…" on the Agents page, "In this workspace", "Bring-your-own agents",
   "This workspace is stricter") and the components you touched
   (`rg -n "<old>" web/tests web/e2e scripts/e2e services crates`).
2. **Update them in the same commit.** Never loosen an assertion so old and new both pass; never `.skip`/`test.fixme`/delete
   a test to get green. A test is removed only with the feature it covers, and the report says so.
3. **Run what you touched and what references it** through `heavy.sh`: every vitest file rendering the Agents page or
   Settings › General, and the mocked Playwright specs visiting them (`ia-4`, `workspace-pages`, `m1`, `first-run`).
   Production specs can't run in a lane: update them by reading and list them in the report.
4. New behaviour gets new tests; old tests are kept and updated.
5. The completion report lists every test file changed and why, the commands run with their results, and the
   production specs left to the nightly.
6. A merge that turns master red because a test still expects the old UI is a send-back to this lane.

## Docs to update

- `docs/14-web-dashboard.md` §Workspace (Settings) and §Altitudes (`agent.byo`); `docs/API_CONTRACTS.md` C-967.
- Not PROGRESS or FEATURES (the Orchestrator updates them at merge).

## Suggested agent/model and reason

`nextjs-developer` (Sonnet): a field moves between two pages and joins an existing form with one save; small, but the
show/hide rule and the shared save need care.

## Completion report

Files changed; screenshots of the Agents page (owner, member) and Settings › General (owner with a choice, owner without,
member) in both themes; test files changed; commands and results; production specs not run.

### Done (lane B, 5debd221)

- Files: `agents-view.tsx` (BYO block removed), `workspace-byo.tsx` → `settings/byo-field.tsx` (`BYO_WORDS`, `useByoChoice`, `ByoField`), `general-tab.tsx` (field after Time zone, one PATCH), docs/14 (General bullet, `agent.byo` row), API_CONTRACTS C-967.
- Tests: `tests/byo.test.tsx` (P5-IA-4 describe replaced: owner with a choice, instance wording, one PATCH with the other fields, clear to null, no choice, own value left, member, Agents page without a block, 422 under the field); `e2e/ia-4.spec.ts` (Agents page has no ladder; Settings field saves and reads back).
- Run: whole web vitest 247 files / 3419 tests passed; mocked Playwright `ia-4`, `refine-16`, `workspace-pages`, `approvals-question` 88 passed (light and dark, 1280 and 390); tsc, eslint on touched files, `scripts/guards/run-all.sh` clean. Production specs: none reference the old ladder.
- Not captured: a member's Settings screenshot (covered by the vitest case).
