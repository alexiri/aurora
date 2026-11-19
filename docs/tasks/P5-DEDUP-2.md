# P5-DEDUP-2: find and remove duplicated web code (shared primitives)

**Owner, 2026-11-18.** "There are probably many other things that are duplicated and could do with
shared primitives. Write briefs for searching for and fixing those duplications: one on the rust
side and another on the web side. These tasks should probably run in parallel, on their own fresh
lanes and with some parallelization, at least to identify duplicated code. The goal is to reduce
the overall lines of code." What prompted this was FIX-173: nine hand-written secret inputs that
become one `SecretInput`. Decision: **D-376**. The Rust side is **P5-DEDUP-1**. It runs at the same
time in its own lane and shares no files with this one.

## Goal
Reduce the total hand-written lines of the dashboard (`web/src`, `web/tests`, `web/e2e`). The way
to do it is to replace duplicated components, hooks, helpers and test plumbing with shared
primitives. **Nothing may change for the person using it**, apart from a screen moving onto the
canonical primitive it should already have used (see Rules). The measure is the net change in
non-blank lines. Making the dashboard smaller but different is a failure.

## Context
- Size on 2026-11-18 (`wc -l`): `web/src` is 106 k lines, but 41 k of them are the generated
  `src/lib/api/schema.d.ts`, so about 64 k are hand-written.
  - `components/` is 52 k lines in about 45 folders.
  - `lib/` is 53 k lines, including 58 files under `lib/api/`.
  - Tests are 40 k lines (`web/tests` vitest, `web/e2e` Playwright).
- Shared homes that already exist. Use them first:
  - `web/src/components/ui/`, about 50 primitives: `Field`, `inputClass`, `Button`, `DataTable`,
    `Dialog`, `ConfirmDialog`, `FormDialog`/`form.tsx`, `Notice`, `Badge`, `Card`, `Section`,
    `Tabs`, `Menu`, `CopyValue`, `EmptyState`, `SettingField`, `Segmented`, `ChoiceCards`,
    `DropZone`, `PageHeader` and others.
  - `components/states.tsx`.
  - `lib/api/client.ts`, `queries.ts`, `paging.ts`, `errors.ts`, `unwrap.ts`.
  - `lib/utils.ts`, `lib/use-tab-param.ts`.
  - `docs/14-web-dashboard.md` §Visual design › **Components** lists the primitives. A new one is
    added there.
- The design rule (CLAUDE.md, 14 §Visual design) already says a page uses the primitives and adds
  a missing one to the shared set. This lane enforces that across the existing screens.
- Guards that must stay green, and that you may extend:
  - `web/tests/design-guards.test.ts` (tokens, no inline styles);
  - `web/tests/ui-text.test.ts` (vocabulary; its exempt lists are empty and must stay empty);
  - FIX-173's password-input guard, once it is merged.
- Not targets:
  - `schema.d.ts` and `web/openapi.json` (generated);
  - `pnpm-lock.yaml`;
  - `components.json`;
  - Next config.

## Scope

### Part A: discover duplication in parallel (read-only, no builds)
1. **Baseline.** On the starting commit, record non-blank lines per top-level folder of `web/src`
   (`components/<folder>`, `lib/<folder>`, `app`), and of `web/tests` and `web/e2e`. Leave out
   `schema.d.ts`. Use:
   `git ls-files 'web/<dir>/**' | grep -E '\.(ts|tsx)$' | grep -v schema.d.ts | xargs grep -cv '^\s*$' | awk -F: '{s+=$2} END {print s}'`
   Record the sha.
2. **Clone scan.** Run `pnpm dlx jscpd@4.0.5 --format typescript,tsx --min-tokens 50 --reporters json
   --output <scratch> web/src web/tests web/e2e --ignore "**/schema.d.ts"` from a scratch
   directory. Do not add `jscpd` to `package.json` or CI. Summarise the JSON in the report.
3. **Pattern sweeps, in parallel.** Run up to **five** read-only explorers (`haiku-explorer`, or
   `Explore` for the larger slices) at the same time. Each gets one slice and the pattern list
   below, and returns clusters.
   - **Slices:**
     1. `components/admin`, `settings`, `governance`, `policy`, `audit`.
     2. `components/apps`, `accounts`, `credentials`, `connectors`, `channels`, `grants`,
        `internet`, `webhooks`, `ssh`.
     3. `components/agents`, `chat`, `approvals`, `automations`, `schedules`, `memory`, `skills`,
        `usage`, `overview`, `onboarding`, `org`, `workspace`, `groups`, `identity`, `peers`,
        plus the top-level component files.
     4. `lib/**` (not `schema.d.ts`) and `app/**`.
     5. `web/tests` and `web/e2e`.
   - **Patterns to look for:**
     - **Hand-built versions of an existing primitive:** a `<table>` instead of `DataTable`, a
       dialog with its own form/submit/error state instead of `FormDialog`, an inline
       `role="alert"` error instead of `Notice`, a hand-written empty, loading or error block
       instead of `states.tsx`/`EmptyState`, a raw `<select>` or checkbox group that
       `Segmented`/`ChoiceCards` covers, copy-to-clipboard written again instead of `CopyValue`.
     - **Repeated component shapes with no primitive yet:** a "Set / Not set + Replace… + Remove"
       row, a confirm-then-mutate button, a status chip mapping a state enum to `Badge` words, a
       keyed list editor, a filter bar driven by the URL, a "load more" pager.
     - **`lib/api`:** per-resource fetch, mutation and invalidate code that follows the same
       template (a generic `useResource`/mutation helper over `client.ts`/`queries.ts` may replace
       many of them); duplicated error-to-field mapping; duplicated paging.
     - **Formatting helpers written again:** dates, relative times, durations, bytes, numbers,
       plurals, role and scope words.
     - **Tests:** repeated mock-server setups, render-with-providers wrappers, fixture objects
       built by hand in many files, Playwright login and navigation sequences, and route mocks
       that a shared fixture would cover.
     - **Dead code:** exports with no importer, and components no route renders. `knip` may help
       (via `pnpm dlx`, pinned, not added).
   - **For each cluster, an explorer returns:**
     - the locations (`file:line`);
     - the number of copies and the lines per copy;
     - the differences (text, testids, behaviour, styling);
     - the proposed primitive or helper and its props;
     - the estimated net saving;
     - the risk (low; visible change; or auth/secret-related).
4. **Catalogue.** The lane lead merges the scan and the sweeps into one ranked catalogue in this
   brief's **Completion report** (Part A), ranked by net saving × safety.
   - Keep a cluster when it has **3 or more copies**, or **2 copies with a net saving of 40 lines
     or more**.
   - List copies that differ on purpose as "not duplicates", with the reason.
   - Commit the catalogue before Part B.

### Part B: fix, in batches
- **One batch is one cluster family**, committed on its own, and its message gives the net line
  delta. When a batch adds a new primitive, the same batch adds its bullet to 14 §Visual design ›
  **Components**. When a batch makes "always use X" enforceable, extend `design-guards.test.ts`
  the way FIX-173 did for password inputs.
- **Order:**
  1. Low-risk batches with a high saving (helpers, `lib/api` templates, test plumbing).
  2. Then the batches that move screens onto existing primitives.
  3. Last, batches that touch sign-in, secrets, approvals or access-granting forms. Each of these
     gets a `sonnet-integration-reviewer` pass before its commit.
- **Before each batch**, list the files changed by every other live lane
  (`git diff --name-only master...alexiri/<lane>`), and skip clusters that touch them. Record the
  skipped clusters.
  - **FIX-173 and SecretInput:** FIX-173 owns the secret inputs and `components/ui/secret-input.tsx`
    until it merges. Do not touch them before then.
  - **FIX-171 and FIX-172** (audit and settings views) are queued in `p5-platform`. Treat their
    files as owned by that lane until they merge.
- **Within the lane:** the vitest and tsc loops are fast and do not share a build lock the way
  `cargo` does. So up to **two implementation workers** (`nextjs-developer`) may run batches on
  disjoint folders at the same time in the lane's worktree. A batch that adds or changes a
  `components/ui` primitive runs alone. The lane lead commits, one batch per commit.
- **Tell the Orchestrator after every ~5 batches** so it merges the lane early and the window for
  conflicts stays short.

### Out of scope
- Any change in behaviour: requests, routes, URLs and query params, text, accessible names, test
  ids that e2e uses, focus order, keyboard behaviour.
- New features, new visual design, dependency upgrades, a state-library change.
- The Rust side (P5-DEDUP-1).
- `schema.d.ts` generation.

## Rules
- **Moving a screen onto a primitive** may change its look only toward what 14 §Visual design
  already prescribes for that primitive. Examples: a hand-made table becoming `DataTable`, with its
  stacked layout at 390 px. Each visible change is listed in the batch's commit message and in
  the report. The text never changes. `ui-text.test.ts` must pass with no new exemptions.
- **A primitive is generic.** It must not take a prop that names one page. Keep the API small:
  if it needs more than one or two escape hatches for one caller, that caller stays as it is.
- **Server/Client boundaries:** follow the existing `"use client"` placement. A shared hook must
  not pull a Server Component into the client bundle.
- **Tests:**
  - Never delete a test case or weaken an assertion to save lines. Only setup and plumbing move
    into helpers.
  - The vitest and Playwright test counts must not drop. The one exception is tests of removed
    dead code, and each of those is listed.
  - Tests of a component that moved keep asserting the same things through the shared primitive.
- **Dead code** is removed only when `tsc`, lint and the build agree it is unused, and no route,
  dynamic import or e2e spec reaches it.
- **Edit with the Edit/Write tools.** A script may do a genuinely bulk mechanical rewrite, and its
  diff is reviewed.

## Constraints / file ownership
- **Owns:** `web/src/**` (except `schema.d.ts`), `web/tests/**`, `web/e2e/**`, and 14 §Visual
  design › Components, batch by batch.
- **Does not own:** `web/package.json` and `pnpm-lock.yaml` (no new dependency), `web/openapi.json`,
  Rust, and the API contracts.
- **Other lanes:** this lane yields to them. Files in another lane's live diff are off limits
  until that lane merges.
- **Disk:** a web-only worktree needs `node_modules` (about 1 GB) and `.next`, not a Rust
  `target/`. Check `df` anyway. 51 GB was free on 2026-11-18.

## Acceptance criteria
1. **Part A catalogue** is committed in the Completion report. It has:
   - the baseline sha and per-folder counts;
   - the `jscpd` summary;
   - the ranked clusters (locations, copies, differences, proposed primitive or helper, saving,
     risk);
   - the "not duplicates" list;
   - the clusters skipped because another lane holds their files.
2. **Every batch:**
   - `pnpm typecheck`, `pnpm lint`;
   - the vitest suites of the touched folders, plus `design-guards`, `ui-text` and the FIX-173
     guard;
   - the Playwright mocked specs that cover the touched screens.
3. **Every ~5 batches and at the end of the chain:**
   - the full `pnpm test`, the full Playwright mocked project and `pnpm build`;
   - a check at 390 px in both themes of every screen whose look changed (no horizontal scroll).
   
   The test counts are no lower than at the baseline, apart from listed dead tests.
4. **Final report:**
   - per-folder line counts before and after, and the net delta (src and tests separately);
   - the delta per batch;
   - every new primitive or helper with its callers;
   - every visible change;
   - every guard added;
   - what was left and why.
5. **Nothing in Out of scope changed.** No request or route differs: the mocked e2e request
   assertions pass unchanged. `package.json`, the lockfile and `openapi.json` are untouched.

## Dependencies
- None to start Part A. Part B skips FIX-171, FIX-172 and FIX-173's files until they merge.
- Runs in parallel with P5-DEDUP-1 (no shared files).

## Suggested agent/model and reason
- **Part A:** up to five Haiku/Explore explorers in parallel, and a Sonnet lane lead for the
  catalogue.
- **Part B:** `nextjs-developer` (Sonnet), at most two at once on disjoint folders, plus
  `sonnet-integration-reviewer` for the batches that touch sign-in, secrets or access.
- Copies that differ in a way that looks like a bug (for example one form validating and its
  twin not) are reported to the owner rather than unified silently.

## Completion report
### Part A catalogue (2026-11-18, lane `p5-dedup-2`; read-only, nothing built)

**Baseline** (sha `f218cdb4`, non-blank `.ts`/`.tsx` lines, `schema.d.ts` left out):
- `web/src` components 49,375; lib 10,407; app 638; `web/tests` 27,550; `web/e2e` 9,237. Total about 97.2 k.
- `components/`: admin 8,384; agents 8,379; apps 4,589; chat 3,326; ui 3,088; accounts 1,858; account 1,776; channels 1,729; org 1,702; connectors 1,426; credentials 1,300; skills 1,230; top-level files 1,144; internet 1,051; settings 1,039; approvals 941; groups 796; governance 781; onboarding 769; policy 549; usage 526; auth 406; grants 379; freeze 290; memory 277; automations 267; schedules 272; webhooks 274; overview 190; workspace 120; theme 110; ssh 114; peers 103; audit 129; identity 61.
- `lib/`: api 7,066; chat 832; telemetry 317; onboarding 202; summary 181; apps 126; nav 89; auth 42; the rest are top-level files.

**jscpd 4.0.5** (`--min-tokens 50`, typescript+tsx; run from the scratchpad, JSON only there; the scan also picked up some `.js`): 506 clones; 5,494 duplicated lines of 148,303 scanned (3.7 %). tsx 382 clones / 3,670 lines (4.76 %); typescript 103 / 784. By location, lines in clones: tests 7,344 (3,672 test-to-test), e2e 1,664 (832 e2e-to-e2e), components/admin 848, settings 336, agents 290, apps 269, org 213, chat 166, approvals 163, lib/api 94. Biggest pairs: org↔settings 187 lines (the keys panel and delete zone), admin↔admin 376. The top test hits (`instances` vs `skill-library`) are tokenizer noise on boilerplate; the real overlap is clusters W1–W3 below. About 60 % of the tsx hits are import blocks and are dropped.

**Method:** five read-only `Explore` sweeps (slices 1–5 of the brief) merged here. Savings are estimates by reading, not measured; the first batch of each cluster re-measures them. A cluster is kept at 3+ copies or 2 copies saving 40+ lines.

#### Ranked clusters (net saving × safety). Part B order is: A (helpers, plumbing, dead code) → B (move onto existing primitives, new small primitives) → C (auth, secrets, approvals, access; each with a `sonnet-integration-reviewer` pass).

**Tier A — low risk, high saving, no visible change**
1. **W1 Test `afterEach(cleanup + vi.unstubAllGlobals)` into `tests/setup.ts`** (about -400). 113 files repeat 4 lines. Leave the 8 `afterEach(cleanup)`-only files, 2 without `unstubAllGlobals` and `files-viewer.test.tsx` (`beforeAll`) alone; files with extra steps keep a local `afterEach` for the extras only. Risk: low. Do first.
2. **W2 `wrap()` QueryClient render helper → `tests/support/render.tsx`** (about -400). About 85 of 107 files define the same 5-line `wrap`; 8 keep `qc`, 7 use `renderHook` (`queryWrapper`). The ~11 copies with `new QueryClient()` without `retry:false` stay separate or take `{retry}`. Risk: low.
3. **W3 Fetch mocks `json`/`err`/`Handler`/`mockFetch` → `tests/support/api.ts`** (about -450; `json` in 97 files, mockFetch in ~68, six variants). Variant (a) (12 identical copies, ~25 files) first; keep the string-input, `decodeURIComponent` and recording variants as options with their own call-record shapes; grep for tests that assert the 404 message before swapping. Risk: medium (a helper could hide a per-test difference).
4. **L1 `useInvalidatingMutation(fn, ...keys)`** in `lib/api/` (about -150 to -200; ~71 hooks over 20 files, 5–7 lines each). `onSuccess` must return the promise. `void`/`onSettled`/`(_r, vars)`/`invalidateAgentGrant` variants stay as they are. Skip `webhooks.ts` until WEBHOOK-4 merges. Risk: low.
5. **D1 Dead code** (about -330 in all, no behaviour):
   - `credentials/generic-form.tsx` `GenericSecretForm` (-115, plus its imports; keep `RecipeFieldsInput`, `buildGenericCreate`, `GenericFormState`) and `referencesAvailable` in `lib/api/secrets.ts:238`;
   - `apps/app-name.tsx` (-24);
   - `chat/turn-usage.tsx` (-62);
   - unused hooks and helpers in `lib/`: `useViewerRoles` (roles.ts), `usePutSettings`/`usePreviewSettings` (settings.ts), `useToggleGrant`/`useEffective` (policy.ts), `useRemoveAgentConnectionGrant`, `useVerifySecret`, `useEvalFingerprint`, `clearDraft`, `currentSubscription`, `appInstanceName`, `reauthErrorMessage`, `agentSummaryText`, `tracingEnabled`, `sectionSlug`, `purgedReason`, `isStatus`, `welcomePath`, `UPLOAD_MAX_BYTES`, `DELETE_MAX`, `MAX_RANGE_DAYS`, `MAX_QUERY`, `ACCESS_*_CODES`, `LOG_MODE_NOTICE`, `PopoverClose`, `PlugsIcon`, ~45 unused type aliases, `AdminOrgFreeze`.
   - Remove only when `tsc`, lint and the build agree. **Owner glance:** `usePutSettings`/`usePreviewSettings`/`useViewerRoles` look like unfinished wiring, not just leftovers. Further, `ConnectDialog`'s non-reconnect paths (below, B6) are dead in production.
6. **E1 Playwright `serious(page)` axe filter → `e2e/support/axe.ts`** (about -170; 36 specs copy a 5-line function that `support/files.ts:190` already exports). Risk: very low. Plus **E2** `check()`/`SHOTS` (-40, 4 identical copies).
7. **L2 `retryUnlessClientError`** (14 identical local `retry` predicates in `lib/api`; -40). Risk: low.
8. **L3 `fmtDate(iso, empty="—")` and `plural(n, one, many)` in `lib/utils.ts`; `ago` helper** (about -50 together; `fmtDate` is copied in ~12 places, `when` ×4, inline in ~10; `plural`/`count` ×3 plus ~35 inline ternaries; `ago`/`updatedAgo` identical ×2). Keep the placeholders ("Never", "never", "") as the argument; keep `key-table`'s `fmtDate` as a re-export. Risk: low. Do **not** merge `formatBytes`/`humanSize`, the countdown or `relativeTime` helpers (different output; see "not duplicates").
9. **L4 `errorDetail(err, key)` and `detailFields(err)`** in `errors.ts` (about -35; ~10 copies of the `typeof d === "object"` cast, 2 of the `fields` parse). Risk: low.
10. **E3 e2e `openThemed`/`trackPageErrors` in `e2e/support/open.ts`** (about -125; 25 specs define their own `open`). Start with the identical copies (`send-as-me`, `ssh`); keep per-spec waits. `support/mock-api.ts` is held by p5-platform: add nothing there. Risk: medium (timing).

**Tier B — move onto existing primitives and add small shared ones**
11. **P1 `ErrorNotice({error, format?, className?})`** in `ui/notice.tsx` (about -75 in admin/settings, -35 in apps/accounts/credentials/connectors/internet/webhooks, -190 repo-wide; 167 copies of `<Notice tone="danger" role="alert">{errorMessage(e)}</Notice>`). Fold `connectors/shared.tsx` `ConnectorError` and `channels/shared.tsx` `MutationError` into it with `format`. Leave the copies with extra children (`plugins-view:75`, `placement-view:362`) and the field-level `<p role="alert">` ones. Optional companion: `ConfirmDialog` `error?` prop (-12; a spacing-only change). Risk: low.
12. **P2 `Select` primitive** (about -45 admin/settings/policy, -45 apps/credentials/channels/internet, 111 raw `<select>` repo-wide). `{value, onChange(value), options, placeholder?, id?}` plus `children` for the grouped copies; the compact `h-9` select in `accounts/actions-table.tsx:251` stays. Risk: low.
13. **P3 `LoadMore({query, label?})`** in `ui/` (about -35 to -60; 12+ copies, 3–5 lines each). Risk: low. Pair with the `HistoryList` shell in `approvals-view` (`HistoryTab`/`ActionHistory`, about -25; keep the decision code out).
14. **P4 `ConfirmButton`** (Delete → Confirm delete / Cancel; about -30 in slice, -50 repo-wide, 5+4 copies: connector-card, channels-view, webhooks-tab ×2 (no Cancel, per-instance state), oauth-clients-panel, memory-tab, schedules-tab, skills-view, library). Risk: low; the webhooks and library copies differ (keep them out).
15. **P5 `useCopy`/`CopyButton`** (11 hand-written clipboard state machines; about -60). `HashValue` keeps its own look. **Visible change** for the copies with no failure handling (`states.tsx`, `connect-tab`, `oauth-clients-panel`, `directory-view` ×2, `previews-tab`: a failed copy is now shown). `one-time-key-dialog` is secret-related (tier C).
16. **P6 Settings-save feedback and error-state helpers**: `SaveFeedback`/`SavedNotice` (13 files; `org/settings-view` + `settings/general-tab` are an identical 13-line block; about -30), `TypeToConfirmDelete` (`org/settings-view.tsx:215-283` + `settings/danger-tab.tsx`, about -45; keep `size="lg"` as a prop) and `useAsyncAction` (3rd copy `account/delete-view`, about -25). **Bug-looking:** the org form guards `confirm !== slug` in `submit`, the workspace form does not (benign; the button is disabled). Report; the shared version keeps the guard.
17. **P7 Admin Settings list editors**: `ProvidersControl` (`admin/settings/sign-in-settings-view.tsx:36-157`) and `EndpointsControl` (`llm-controls.tsx:195-336`) share ~120 lines → `useEntryListEditor` plus `EntryActions`/`EntryErrorList` (about -65). Dialog twins `llm-endpoint-dialog`/`sign-in-provider-dialog`: `str`/`lines`/`split` helpers (-9), `idProblem` (-8), `TestRow` (-15), `HelmEntryDialog` (-25; the provider one hides `client_secret`: confirm endpoints cannot hold secrets). Not 14 §Visual design primitives, so these stay in `admin/settings/`. Risk: low–medium. **FIX-172 touches settings views: re-check.**
18. **P8 Schedules/webhooks tab shell**: `AgentItemRow` + `AgentItemList` (`schedules/schedules-tab.tsx` ↔ `webhooks/webhooks-tab.tsx`, ~70 shared lines per file, about -60; `confirm` is a boolean vs `"delete"|"rotate"|null`). Plus the `CronTimezoneFields` pair in `agents/evals/schedules-section.tsx`. Risk: low.
19. **P9 Role select and `AddBindingForm`** (`settings/access-tab` ↔ `group-access`, 4 identical `member/approver/owner` selects plus the two add-forms; about -40). Keep lowercase labels where they exist. This is access-granting UI, so it is **tier C**.
20. **P10 Hand-built dialogs onto `FormDialog`**: `accounts/account-menu` Rename/Replace (-20; needs an optional `formLabel` and keeps `space-y-3` or accepts `space-y-4`), `addon-sources-view` `ScheduleDialog` (-12), `policy/rule-editor` `RuleEditorDialog` (-8), `plugin-detail-view` Reject (-8; **adds a Cancel button: visible**). `account/push-devices` ↔ `ssh-keys-view` rename dialogs (-25, 2 copies). Risk: visible change (listed per batch).
21. **P11 Small shared pieces**: `useAppDetails(apps)` in `lib/api/apps.ts` (-10), `toggleSet` and `useAgentNames` (-15), `RuleActionBadge` (-8), `sandboxTone` (-4), `buildJourneyConnect` tail (-8), `setup-view` `STATE[...]` reuse (-5), `AuditTable`/`FeedStates` for `settings/audit-tab` ↔ `audit/audit-panel` (-30; **FIX-171 owns audit views: wait**), `useCursorPager` (-16, labels kept), `TextField` (`Field`+`input`; -2/-3 per copy, only after P1/P2). Risk: low.
22. **T1 Terminal tab tests** `RecordingSocket`/`installSocket` → `tests/support/terminal.ts` (-70; only the 2 identical `RecordingSocket` copies move; the other stubs differ). **T2 fixtures** `ORG`, `T` (-40; only exact copies). **T3** `stubClipboard()` (-12). **T4** e2e `expectThemedAccessible` (`addon-sources`/`skill-libraries`; -20). Risk: low–medium.

**Tier C — auth, secrets, approvals, access (last; each with a reviewer pass before its commit)**
23. **S1 API-key panels**: `org/keys-view.tsx` and `settings/keys-tab.tsx` are one file twice (about 95 identical lines; both slices found it) → `ApiKeyPanel({idPrefix, keys, create, revoke, header?})`, each caller ~12 lines (about -75 to -80). Keep every id and testid via `idPrefix` (`okey-*`/`key-*`, `okeys`/`wkeys`), the one-time dialog and the 1–80 / future-expiry validation exactly. Related 8-line block in `account/my-keys-view.tsx`.
24. **S2 Grant editors** `agents/grant-editor.tsx` ↔ `agents/app-grant-editor.tsx`: ~85 shared lines (`change`, `submit`/`mapGrantError`/`customOnly` fallback, footer, Dialog/Loader shell) → `GrantEditorForm` + `GrantDialogShell` (about -70). Also compute `splitByScopes(toolsForMethod())` once in `EditorLoader`. Keep the error mapping exactly.
25. **S3 `ConnectDialog`** (`apps/connect-dialog.tsx`, ~320 lines) runs a second connect flow beside `connect-journey/sign-in-step.tsx` (**off limits: p5-platform's live diff**). In production it is mounted only from `accounts-list.tsx:220` with `reconnect`, so `CredentialFreeForm` (~120 lines) and the `!reconnect`/instance-picker branches are unreachable, but 3 test files (`credential-free-apps`, `app-families`, `instances`) still exercise them. **Needs the owner's decision**: delete the unreachable branches and rewrite those tests (about -150; tests removed would have to be listed), or fold reconnect into `ConnectJourney` (about -300, behaviour-risky). Not started without an answer.
26. **S4** copy behaviour of `account/one-time-key-dialog.tsx` via `useCopy` (part of P5), approvals pending-tone `<p role="alert">` → `<Notice tone="pending">` ×3 (`approval-question`, `approvals-view`, `own-send-approvals`; -6; `<p>` becomes `<div>`), `AgentChecklist`/`WorkspaceRows` row (-15; markup changes slightly), `credentials/give-access.tsx` selection state.

**Estimated total** if everything in tiers A and B is taken: about -2,900 lines (tests about -1,600: W1–W3, T1–T4, E1–E3; src about -1,300: D1, L1–L4, P1–P11), and tier C about -200 more, S3 aside. These are sums of the sweeps' estimates; the first batch of each cluster re-measures. The estimate is about 3 % of the 97 k hand-written lines.

**Not duplicates (differ on purpose, leave alone)**
- `HashValue` (truncates, copy only for a hash) vs `CopyValue`; `connector-detail`'s secret Set/Replace/Clear/Undo row (single use, staged state); `Fingerprint` in `addon-sources-view` (inline layout; `CopyValue` has no label override).
- `grant-editor` read-only/`inert`/`unavailable` parts vs `app-grant-editor`; `OrgFreeze` vs `AdminOrgFreeze` (different caller rank).
- `AuditPanel` (Critical badge, org column, critical-only filter) vs `AuditTab`: do not merge them; only the table and feed states are shared.
- `ActionHistory` vs `HistoryTab` (columns, status sets, tone maps); `StatusChip` in channels vs `ConnectorStatusChip` (the connector labels are sentences); `channelError` vs `connectorError` (different server code tables).
- `formatBytes` (`attachments.ts`, one decimal, to MiB) vs `humanSize` (`files.ts`, to TiB): they print the same size differently ("12.0 KiB" vs "12 KiB"); an unintended inconsistency, not unified because it changes text. The countdown helpers (`approvals.ts`, `approval-card.tsx`, `usage-format.ts duration`, `chat/layout.ts formatDuration`), `expiryText`, `lastActivityText`, `relativeTime`, `ago`/`updatedAgo` (different phrasing); fixed `en-GB` (chat) and `en-US`/UTC (usage) formatters vs browser-locale `toLocaleString()`. The two `ROLE_LABEL` constants (member roles vs model sizes). `fieldErrors` vs `governanceIssues`/`policyIssues` (different rules; only the `fields` parse is shared, L4).
- `accounts/actions-table.tsx:251` compact `h-9` select; `availability-view` raw radios (`ChoiceCards` would restyle them, -10, skipped); `rule-editor` checkboxes (style drift only fixable with a visible change); `StringListEditor`/`HostListEditor` vs the egress allow-list (disabled/readOnly/shape differ); `useNow` ×2 (one takes `active`); `useFreeze*` ×4 pairs (need casts over typed openapi paths; about -25, not worth the type risk); `Dialog open onOpenChange` shell ×9 (1 line each); the three `LegacyGate` pages under `app/` (Next needs one route file each); `fetchAllPages` ×35 (typed paths; not generalisable).
- Tests: `terminal-tab-*` stubs (url assertion, mutable `sandboxState`, Fake clipboard events differ); `instances.test`/`skill-library.test` jscpd hits; `directory.test` self-clone (per-test route stubs differ); `e2e` themed `forEach` wrappers (`themeMatrix` would only re-indent everything; skip); `e2e` `fulfillJson` ×18 (net 0 per copy; only with a `mock-api.ts` change by another lane).

**Looks like a bug (reported, not unified silently)**
1. `org/settings-view.tsx` `OrgDangerZone.submit` guards `confirm !== slug`; `settings/danger-tab.tsx` does not (button disabled in both, so benign).
2. Four clipboard copies (`connect-tab.tsx:36`, `one-time-key-dialog.tsx:57`, `oauth-clients-panel.tsx:93`, `states.tsx:31`) call `navigator.clipboard?.writeText(x).then(...)` with no `.catch`: a rejected write is an unhandled rejection, and with no clipboard `?.` gives `undefined` and `.then` throws. `directory-view.tsx:47,190` show "Copied" whatever the result.
3. `connectors/add-connector.tsx:143` validates required settings, `connector-detail` `SettingsForm` does not (probably deliberate: secrets stay "set"). `channels/integration-dialog.tsx:47` exempts credential fields when `existing`; `agent-channels-tab.tsx:219` (re-attach) does not. `GenericSecretForm` had checks that `sign-in-step.tsx` (another lane) may lack. Flag only.
4. `freeze-panel.tsx:24` `date()` has no null check (`when`/`fmtDate` elsewhere do).

**Skipped because of other lanes** (checked with `git diff --name-only master...alexiri/<lane>` at `f218cdb4`):
- `alexiri/p5-platform` (28 files): `web/src/components/webhook/**`, `lib/webhook/logic.ts`, `lib/api/webhook.ts` (P5-WEBHOOK-4); `components/accounts/account-row.tsx`, `admin/admin-topics.tsx`, `org/org-topics.tsx`, `apps/connect-journey/sign-in-step.tsx`, `ui/code-field.tsx`; `e2e/support/mock-api.ts`, `e2e/support/webhook.ts`, `e2e/webhook*.spec.ts`; `tests/webhook-*.test.ts(x)`. This blocks cluster S3's fold option and parts of E2/E5; FIX-171/172/173 files (audit and settings views, `ui/secret-input.tsx`, the secret inputs and the password-input guard) are treated as owned too: P11 `AuditTable`, P7 `admin/settings/**` and the `keys`/`secret` clusters wait for them.
- `alexiri/p5-apps` (77 files), `alexiri/fix-167` (24), `alexiri/p5-agent` and `alexiri/p5-dedup-1` (0): no `web/` files at this time. Queued briefs not yet visible in a diff: P5-GUIDE-7 (`domains/tools/`), FIX-170, FIX-169 terminal wrappers, P5-WEBHOOK-1/2. Re-run the diffs before each batch.

**Suggested Part B batches** (one cluster family per commit; two disjoint workers at most): 1 W1 + W2 (test plumbing); 2 W3; 3 E1 + E2; 4 L1 + L2; 5 L3 + L4; 6 D1; 7 P1; 8 P2; 9 P3 + P4; 10 P5; then P6–P11, T1–T4, E3; then tier C with reviews (S1, S2, S4; S3 on the owner's word).

### Part B progress (lane `p5-dedup-2`, 2026-11-18)
Owner answers 2026-11-18, folded into the plan: delete `usePutSettings`, `usePreviewSettings`, `useViewerRoles`, `chat/turn-usage.tsx` (with D1); `ConnectDialog` loses its unreachable non-reconnect branches and tests are rewritten (a commit `P5-DEDUP-2: S3 …`, only `connect-dialog.tsx` and its tests; `sign-in-step.tsx` stays off limits).

| Batch | Commit | Net lines | Notes |
|---|---|---|---|
| 1 W1+W2 | `425408a1` | -732 | `afterEach(cleanup, unstubAllGlobals)` in `tests/setup.ts` (113 files), `wrap()` in `tests/support/render.tsx` (64 files). vitest 145 files / 2341 tests, unchanged. |
| 2 W3 | `ce0d4a42` | -292 | `json` (96 files), `err` (5), `Handler`+`mockFetch` (12 identical copies) in `tests/support/api.ts`. `json` is the null-aware form; files whose `json` sent `"null"` for a literal null body keep their own. |
| 3 E1 | `d686ba3b` | -114 | `serious(page)` in `e2e/support/axe.ts` (17 identical specs; `files.ts` re-exports). E2 not done: the `SHOTS` directories differ per spec. Mocked Playwright on the 17 specs: 275 passed, 5 timeouts at load ~45 that passed alone. |
| 4 L1+L2 | `c44ccd40` | -205 | `useInvalidatingMutation` (`lib/api/mutations.ts`, 50 hooks in 19 files), `retryUnlessClientError` (`errors.ts`, 15 files). Skipped `webhook(s).ts`. |

Total so far (`git diff --shortstat master...HEAD -- web/`): 161 files, +646 −1,989 = **-1,343 lines** (tests -1,138, src -205). No visible change, no test case removed or changed. Bulk mechanical rewrites were done by reviewed node scripts (stated in each commit).

Gate notes: the host load was 45–90 (six lanes). Full vitest runs then lost tests to 20 s timeouts and "failed to start forks worker"; every file that failed passed when run alone (byo, chat-v2, evals-page, files-viewer, composer-p5, context-switcher, grant-form, markdown, addon-sources, agent-next-steps, agent-page-header, auth-views, secret-stores). `scripts/guards/run-all.sh` blocked in `dep-graph.sh` (`cargo tree` at 0 % CPU, waiting on a cargo lock held elsewhere), so it was not completed for batch 4; the guards that do not need cargo pass. **Conflict to expect:** `alexiri/p5-platform` now changes `web/src/lib/api/account.ts`, which batch 4 also touched (the `retry` predicate and about 6 mutation hooks).

**Batches 5–6 (2026-11-19, after batches 1–4 merged; not merged yet, `git diff --shortstat master...HEAD -- web/`: 34 files, +140 −865 = −725 lines):**

| Batch | Commit | Net lines | Notes |
|---|---|---|---|
| 5 L3+L4 | `e1e9c34f` | -11 | `fmtDate`/`plural`/`ago` in `lib/utils.ts` (key-table keeps `fmtDate` as a re-export), `errorDetail`/`detailFields` in `lib/api/errors.ts`. The helpers cost what the call sites save, so the net is small. Skipped (p5-platform's live diff): send-as-me, secret-stores-view, admin/settings views, instructions-tab, webhook logic. typecheck, lint, build, vitest 96 files / 1232 tests, guards/run-all.sh exit 0. |
| 6a S3 | `4434c151` | -383 | `ConnectDialog` is reconnect-only (`reconnect` required): `CredentialFreeForm`, the instance picker, the read/write choice and the family step are gone; `DeploymentStep` stays exported for the journey. **9 tests of the deleted branches removed**: instances 4 → 2 (now `InstanceField` direct), connect-methods −2 (non-reconnect read/write) and the chooser tests run as reconnects (button "Reconnect"), credential-free-apps −3; app-families keeps its 3 tests on `DeploymentStep` direct. |
| 6b D1 (part) | `285f16ae` | -331 | Owner's deletions: `usePutSettings`, `usePreviewSettings`, `useViewerRoles`, `chat/turn-usage.tsx`; plus `GenericSecretForm` (keeps `RecipeFieldsInput`) and `referencesAvailable`. **5 tests of dead code removed** (model-usage `TurnUsage` 2, credentials generic form 3). typecheck, eslint, build, 4 vitest files green; `run-all.sh` not re-run. |

D1 left for later: `apps/app-name.tsx` (a test asserts it, so deleting it removes a test), and the unused constants, types and small helpers in the catalogue (`useToggleGrant`, `useEffective`, `useRemoveAgentConnectionGrant`, `useVerifySecret`, `useEvalFingerprint`, `clearDraft`, `currentSubscription`, `appInstanceName`, `reauthErrorMessage`, `agentSummaryText`, `tracingEnabled`, `sectionSlug`, `purgedReason`, `isStatus`, `welcomePath`, `UPLOAD_MAX_BYTES`, `DELETE_MAX`, `MAX_RANGE_DAYS`, `MAX_QUERY`, `LOG_MODE_NOTICE`, `PopoverClose`, `PlugsIcon`, `AdminOrgFreeze`: all still have no importer outside their own file). The full web gate (vitest full, Playwright mocked full, 390 px / both themes, production) has not run for batches 5–6; only touched-file vitest, typecheck, lint and build did. Paused by the Orchestrator for P5-POLISH.

Next: finish D1 (the list above) → 7 P1 → 8 P2 → 9 P3+P4 → 10 P5 (with the clipboard fix and its vitest) → P6–P11, T1–T4, E3; S3 as its own commit; tier C last.
