# P2-UI: Dashboard visual design (phase 2)

## Goal
Make the dashboard look like the approved design and brand, in light and dark themes with a theme switch. Implement `docs/14-web-dashboard.md §Visual design (phase 2, P2-UI)` exactly as written, plus decisions D-68, I-40 and I-41 in `docs/90-decisions.md`. This task changes presentation only. API calls, routes, permissions and behaviour stay the same.

## Linked requirements
- Spec: `14 §Visual design` (normative), `14 §Architecture rules` (CSP, accessibility, copy lint), `14 §Information architecture` (which pages exist and who sees them).
- Tests: `18 §Dashboard visual design (phase 2, P2-UI)`.
- Decisions: D-68 (adopt the design, dark theme and brand), I-40 (self-hosted fonts via `@fontsource-variable/*` + `next/font/local`), I-41 (theme preference in a cookie + `localStorage`, applied before first paint by a constant nonce-carrying inline script).
- Feature row: `FEATURES.md` `P2/14§visual`.

## Context
- Reference mock-ups: `docs/design/ui-refresh/*.html` (light screens, plus `*Dark.html` for dark). Open them in a browser and read the inline styles for exact values. `docs/design/ui-refresh/README.md` lists where the mock-ups differ from the spec; the spec wins.
- Brand assets: `docs/design/ui-refresh/brand/`. It holds the owner's master logos `aurora-logo-final-{light,dark}.svg` and the variants generated from them: lockups, marks, the outlined wordmark, the favicon and app icons. Its `README.md` gives the parts, colours and usage.
- Stack: Next.js 16 App Router, React 19, TypeScript strict, Tailwind CSS v4 (`@import "tailwindcss"` in `globals.css`, no `tailwind.config`), Radix (dialog, dropdown-menu, slot), `class-variance-authority`, `cn()` in `src/lib/utils.ts`, vitest + Testing Library, Playwright.
- Relevant files:
  - `src/app/globals.css`
  - `src/app/layout.tsx`
  - `src/components/app-shell.tsx`: header, `OrgNav`, `WorkspaceNav`, switchers, user menu. It currently shows "coming soon" placeholders, which must go.
  - `src/components/ui/{button,dialog,field,menu}.tsx`
  - `src/components/states.tsx`, `agent-avatar.tsx`, `agents-list.tsx`
  - `src/app/o/[org]/w/[ws]/page.tsx` (Overview)
  - `src/components/approvals/approvals-view.tsx`
  - `src/components/agents/agent-page.tsx`, `chat-tab.tsx`
  - `src/components/auth/auth-card.tsx` and the `*-view.tsx` files beside it
  - `src/components/account/account-nav.tsx`
  - `src/app/{account,admin,o/[org]}/layout.tsx`
- CSP (`src/proxy.ts`): `default-src 'self'`; `style-src 'self' 'nonce-…'` in production. **Inline `style=` attributes are blocked in production**: use Tailwind classes and CSS only. Do not change the CSP.
- Existing data only:
  - Overview activity: the existing activity/request-log query used by `approvals/activity-view.tsx`.
  - Agent-header state: the existing sandbox query used by the Computer tab, hosted agents only.
  - Approvals count: the existing pending-approvals query or stream.
  - The `Agent` schema has no runtime state, so agent cards show **no** status dot.
- Scale: about 80 files under `src/` use Tailwind palette colours (about 525 occurrences). They all move to tokens in stage 3.

## Scope
Deliver this in three stages. Each stage is its own commit series and is mergeable on its own, with all checks green.

### Stage 1: foundations and shell (merge first)
- **Tokens:** define them in `globals.css` exactly as in the `14` token table, for both themes: `:root` holds light, `:root[data-theme="dark"]` holds dark, plus `color-scheme`. Map them through `@theme`, using Tailwind v4's `@custom-variant dark` keyed on `data-theme`. Add the focus ring.
- **Theme (I-41):**
  - a `ThemeSwitch` radio group (System / Light / Dark);
  - persistence in `localStorage` + the `aurora_theme` cookie;
  - a server-rendered `data-theme` on `<html>` for explicit choices;
  - the constant inline `<head>` script with the request nonce (read `x-nonce` from the request headers, as `src/proxy.ts` sets it) that resolves System before first paint;
  - live updates on `prefers-color-scheme` changes while on System.
- **Brand:**
  - an `AuroraMark` component (inline SVG, variants auto/light/dark/mono-ink/mono-white, gradient ids from `useId`) and an `AuroraLogo` component (mark + the **outlined** wordmark, horizontal or stacked, following the theme). Copy the path data from the generated brand files; never typeset "AURORA" in a font;
  - copy `brand/aurora-favicon.svg` to `src/app/icon.svg`;
  - generate PNG app icons, including `apple-icon`, from `brand/app-icon-*.svg`. Commit the PNGs: no build-time raster dependency.
- **Fonts:** add the three `@fontsource-variable/*` packages at exact versions. Load them with `next/font/local` in `layout.tsx` and expose them as `--font-sans`, `--font-display` and `--font-mono`.
- **UI primitives** in `components/ui/`:
  - update Button (variants `primary`, `outline`, `approve`, `ghost`, `danger`; sizes `sm`, `default`, `lg`), Field, Dialog and Menu;
  - add Badge, StatusDot, Card, Tabs (segmented + underline, with correct `tablist`/`tab`/`aria-selected`/`aria-controls`) and Avatar (extend `agent-avatar.tsx` rather than duplicating it);
  - restyle EmptyState, ErrorState and Loading.
- **Shell:** rebuild `app-shell.tsx` as the sidebar shell. It needs:
  - the horizontal `AuroraLogo` (about 38 px high, accessible name "Aurora");
  - the `ThemeSwitch` in the sidebar footer, above the account menu, and on Account › Profile;
  - one context switcher for org + workspace;
  - icon nav for each context (workspace, org, account, admin);
  - the pending-count pill on Approvals (only where `14` / RFE-012 allow);
  - the account menu at the bottom;
  - a narrow-screen top bar with a nav dialog;
  - a `PageHeader` component (breadcrumb, title, description, actions).
- Remove the "coming soon" nav placeholders. Keep every existing link, its visibility rule (admin-only items) and `aria-current`.
- Icons: add inline stroke SVG icons in one file, `components/ui/icons.tsx`. Add no icon library.

**Scope (owner, 2026-11-08):** the design is the dashboard's visual identity. **Every** existing screen and every future screen follows it (`14 §Visual design`, "Scope: the whole dashboard"). The mock-up screens are the references the shared patterns are taken from, not the limit of the work.

### Stage 2: reference screens and the shared page patterns
- Restyle these screens to the mock-ups and `14 §Visual design §Screens`, in both themes:
  - Workspace Overview, including the approvals callout, agents grid, checklist with progress and recent activity;
  - Approvals (gateway and action approval cards, batch header, conflict notice, segmented tabs, Live indicator);
  - the agent page header and tab strip, and the Chat tab (bubbles, tool rows, inline approval panel, composer);
  - the auth layout (`auth-card.tsx` becomes the split layout with the dark brand panel: the dark horizontal logo and the tagline; every `/auth/*`, `/join`, `/link/channel` and `/setup-error` view uses it).
- Use `PageHeader` on every shell page.
- **Extract the page patterns** these screens establish into reusable components under `web/src/components/ui` (or `web/src/components/patterns`), so every other screen composes them instead of re-styling by hand: page sections and cards, lists and data tables (incl. row actions and narrow-screen behaviour), forms and field groups (labels, help, errors, submit rows), empty / loading / error states, confirmation and form dialogs, status badges and dots, tab strips that scroll within themselves at 390 px. Document each pattern briefly in `14 §Visual design` (what it is for, which component) so later lanes and future screens use them.
- **Auth forms:** change layout and styling only. Do not change form logic or which fields appear. P1-005 (RFE-004, OIDC-only) owns the form contents. If P1-005 is in flight, rebase onto it. Never reintroduce a password form that it removed.

### Stage 3: every other screen adopts the design (parallel lanes by area)
Not a colour swap: each screen is rebuilt from the stage-1 primitives and the stage-2 page patterns, matches the reference screens' look (spacing, typography, cards, tables, forms, states), works in both themes and at 390 px with no horizontal page scroll. No behaviour, API or field changes. Lanes (disjoint directories, run in parallel after stage 2 merges):
- **3a — org, account and admin:** org overview, members, invitations, workspaces, governance, storage, API keys, account pages, every `/admin/*` page.
- **3b — workspace:** credentials and connections, apps, policy (editor, simulate, warnings, effective view), activity, channels, workspace settings and access, BYO controls.
- **3c — agent pages:** every agent tab other than Chat (instructions, memory, skills, schedules, webhooks, peers, channels, access, computer/sandbox), conversation actions (archive, delete, redact, export), and the agent list/create flows.
Each lane lists every route it restyled in its report, with a screenshot of each in light and dark at 1280 px and 390 px saved outside the repo (`/tmp/aurora-screens/p2-ui-s3<x>/`, never committed).

### Stage 4: enforcement
- Move anything stage 3 left on palette colours or hex literals to tokens (there should be little left).
- Add the token-guard vitest and the Playwright accessibility (both themes), theme and narrow-shell tests from `18 §Dashboard visual design`. Add `@axe-core/playwright` as a dev dependency at an exact version. Extend the narrow-shell test to every route (no horizontal page scroll at 390 px).
- **Inline styles**: the production CSP (`style-src 'self' 'nonce-…'`) blocks `style=` attributes; only dev allows them, so they silently break in production. Known cases: `components/org/storage-view.tsx` and `components/memory/size-meter.tsx` (meter widths). Add a shared `Meter` primitive that needs no inline style (e.g. an SVG `<rect width>` or stepped width classes), move both meters to it, and add a guard (vitest or `scripts/guards/`) that fails on any `style=` prop in `web/src`.
- The guard and these tests are what keep future screens on the design (`14 §Visual design`, "Scope: the whole dashboard").

### Out of scope
- API, OpenAPI or schema changes, including an agent runtime-status field. If you think one is needed, report it as a follow-up.
- A command palette or search ("Search or jump to… ⌘K" in the mock-ups).
- A per-user, server-stored theme preference (I-41 keeps it in the browser).
- New pages or behaviour.
- Nothing on this list exempts a screen from the design: phase-2 RFE screens added by other lanes are restyled by the stage-3 lane for their area if they don't already follow it.

## Constraints / file ownership
- You own:
  - `web/src/app/globals.css` and `web/src/app/layout.tsx` (font wiring, `data-theme` and the theme script only);
  - `web/src/app/icon.svg`, `apple-icon.png` and the app-icon files under `web/public/`;
  - `web/src/components/ui/**`, `app-shell.tsx`, `states.tsx`, `agent-avatar.tsx`, `agents-list.tsx`;
  - the styling (className changes) of every other file under `web/src/components` and `web/src/app`;
  - the new tests;
  - `web/package.json` and `pnpm-lock.yaml`, for the font and axe packages only.
- Do **not** change logic, hooks, queries, API calls, `src/lib/**`, `src/proxy.ts` or `openapi.json`.
- Other phase-2 lanes may edit the same component files to add RFE features. Stage 1 must merge **before** they start their dashboard work, if at all possible. In stages 2 and 3, rebase often and resolve conflicts by keeping their behaviour and applying your styling. Never drop their changes.
- Keep untrusted-text rendering as it is: React text nodes or the safe markdown renderer, never `dangerouslySetInnerHTML`. The one exception is the theme script (I-41). It must be a module-level string constant with no interpolation, carry the nonce, and have a comment pointing to I-41.
- Copy lint stays green: no platform or app names in core UI copy.
- Every interactive element is a real `<button>`, `<a href>` or form control. Icon-only buttons have an `aria-label`. Primary actions are at least 44 px high and others at least 36 px.

## Acceptance criteria
- [ ] Stage 1:
  - tokens, fonts and primitives exist as specified;
  - every shell page renders in the sidebar shell, with no "coming soon" items;
  - the narrow-screen nav dialog works (keyboard, Esc, focus return);
  - the production CSP string is unchanged, and fonts and icons are served from the dashboard's origin;
  - the theme switch works: System follows the OS live, Light and Dark persist across reloads, and a reload never flashes the wrong theme;
  - the favicon and app icons are in place, and the sidebar shows the horizontal logo in both themes.
- [ ] Stage 2: Overview, Approvals, the agent page + Chat, and the auth pages match the light and dark mock-ups, within the spec's corrections, at 1280 px and 390 px. There is no horizontal page scroll at 390 px. The shared page patterns exist as components and are documented in `14 §Visual design`.
- [ ] Stage 3 (per lane): every route of the lane's area is built from the primitives and page patterns, looks like the reference screens in light and dark, and has no horizontal page scroll at 390 px; the report lists every route with its screenshots.
- [ ] Stage 4: the token-guard test passes, so no palette colours or hex literals remain in `src/` outside `globals.css` (the one exception is the brand colours inside `AuroraMark` and `AuroraLogo`), and the narrow-shell test covers every route.
- [ ] The axe checks report 0 serious/critical violations on the pages listed in `18`, in both themes, and the theme tests pass.
- [ ] `pnpm typecheck`, `pnpm lint`, `pnpm test` and `pnpm build` pass.
- [ ] The existing vitest suites and `e2e/` Playwright specs pass. Selectors are updated only where markup changed by design, and no assertion is removed.
- [ ] `bash scripts/e2e/m1.sh` passes.
- [ ] Before/after screenshots of Overview, Approvals, agent Chat and sign-in at both widths, in light and dark, are attached to the completion report.

## Verification
- Commands (in `web/`): `pnpm install --frozen-lockfile && pnpm typecheck && pnpm lint && pnpm test && pnpm build`, the new Playwright specs, and `bash scripts/e2e/m1.sh` from the repo root.
- Expected results: everything green. Run `curl -sI` against a production build (`pnpm start`) and confirm the `Content-Security-Policy` header is identical to master's.

## Dependencies
- Blocked by: nothing (phase 0 is on `master`).
- Coordinate with:
  - P1-005 (auth form contents);
  - P2-006, P2-010, P2-011A, P2-012, P2-021 and P2-025, which add dashboard UI and should build on the stage-1 primitives;
  - P2-017, which adds a retry toast: style it with the tokens.
- Unblocks: consistent, theme-aware styling for all later dashboard work.

## Suggested agent/model and reason
`nextjs-developer` (Sonnet), in its own worktree. The work is multi-file Next.js styling with accessibility and CSP constraints. Stage 3 is mechanical and may go to `haiku-worker` in batches of about 10 files, once stage 1's primitives and guard pattern exist.

## Completion report
- Files changed:
- Tests/checks:
- Screenshots:
- Remaining risks / follow-ups (e.g. agent runtime status in the API, command palette):
