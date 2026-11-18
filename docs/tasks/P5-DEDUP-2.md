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
(Part A catalogue first, then the batches.)
