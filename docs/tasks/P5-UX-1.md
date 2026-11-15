# P5-UX-1: Role-aware navigation, refusals and empty states; first-run fixes (owner walkthrough, 2026-11-15)

## Goal

The first-run walkthrough found pages that lie to people about why they cannot proceed:

- An org member saw **Credentials** and **Apps** in the org sidebar. Both pages, and the Welcome page's
  "Invite a teammate" (the org Members page), said **"Could not reach the server. Check your connection
  and try again."** The API had answered 403.
- The Welcome page's **"Connect an app"** was a 404.
- A workspace **owner** was told "Ask a workspace owner to add one." right under the Add button.

This task makes the dashboard show each person only what they can open, turns refusals into words, makes
empty states ask only people who cannot act, and fixes the small first-run defects found on the way.

## Linked requirements

- D-323; contracts **C-915, C-916**, the interim link of **C-927**; renders C-912's `required_role`.
- Normative: `14 §First run and connecting` §2, `14 §Organization` (nav note), `14 §Onboarding` (empty
  states teach; role-aware), `14 §Visual design`.

## Context (facts found 2026-11-15)

- **Nav.** `web/src/components/app-shell.tsx` ~299–323 `OrgNav` shows Workspaces, Credentials, Apps and
  Governance to every role. `ORG_LINKS` (~284–297) are already admin-gated. `WorkspaceNav` (~337–368).
- **The cause of the network sentence.**
  - `web/src/components/org/org-gate.tsx:24` returns `<ErrorState error={{ code: "forbidden" }} />`, a
    plain object.
  - `errorMessage` (`web/src/lib/api/errors.ts` ~238–245) looks codes up only for `instanceof ApiError`
    and otherwise returns the network sentence. `ERROR_MESSAGES.forbidden` already exists (~16).
  - The same plain-object pattern is in `components/channels/shared.tsx:96`,
    `components/credentials/workspace-credentials.tsx:18,20` and
    `components/policy/workspace-policy.tsx:41`.
- **Empty states.**
  - `components/states.tsx` ~32–58 `EmptyState` renders `ask` whenever there is no `action`.
  - `credentials-view.tsx:58–72` always passes `ask`; `schedules-tab.tsx:260` does the same.
  - The correct conditional use is in `connections-tab.tsx:238` and `policy-view.tsx:240`.
  - Name helpers: `ownerAsk` and `useOwnerNames` in `components/onboarding/getting-started.tsx` ~56–66.
- **Welcome.** `getting-started.tsx` ~351–371 `OptionalNext`:
  - "Connect an app" links to the non-existent `${base}/apps`;
  - "Invite a teammate" links to the admin-only `/o/<org>/members` and shows when the *workspace* role is
    owner.
- **Role data.** `useOrg(org).data.my_role`, `useWorkspace(ws).data.my_role`, `useCanApprove`, and
  `/v1/me` `memberships[].workspaces[].my_role` (`lib/api/queries.ts`). There is no shared role hook.

## Scope

1. **Role hook.** Add `useViewerRoles(org, ws?)` → `{ orgRole, wsRole, isOrgAdmin, canOwn, canApprove }`
   in `web/src/lib/api/roles.ts`, from the existing queries. Replace the ad-hoc `my_role === "owner"`
   checks this task touches; do not sweep the rest of the app.
2. **Nav (C-915).** `OrgNav` lists Credentials and Apps only for org owners and admins. Members keep
   Workspaces and Governance. Update the `14` org-section note (already in the spec) only if the code
   differs from it.
3. **Refusals (C-915).**
   - Add `forbiddenError(requiredRole)` returning an `ApiError`, and use it in every gate listed above.
     Any other bare `{ code }` error object found by `grep` gets the same treatment.
   - `errorMessage` maps any object with a string `code` through `ERROR_MESSAGES`, and renders
     `forbidden` with `details.required_role` as "Only <phrase> can open this page." The phrases:
     - `org_admin` → "organization owners and admins"
     - `org_owner` → "organization owners"
     - `workspace_owner` → "workspace owners"
     - `workspace_approver` → "workspace owners and approvers"
   - `ErrorState` accepts `askNames` and appends "Ask <names>." when given.
   - The network sentence stays only for transport failures (`TypeError` from fetch, `AbortError`, a
     non-JSON 5xx).
   - Add `ERROR_MESSAGES` keys for C-912's codes: `connect_probe_failed` per cause and
     `oauth_client_required`. Coordinate with P5-UX-3 on `errors.ts` (append only).
4. **Empty states (C-916).**
   - `EmptyState`'s `ask` becomes `ask?: { text: string; canAct: boolean }`, or an equivalent the type
     system enforces, and renders only when `!canAct`.
   - Fix every caller (credentials, schedules, any other `ask` user).
   - Where a role can act, pass the primary action instead.
   - The "ask" text names owners with `useOwnerNames`.
5. **Welcome links (C-927, interim).**
   - "Connect an app" → `…/credentials?tab=apps` until P5-UX-3 replaces it with the journey.
   - "Invite a teammate" is hidden until P5-UX-5 merges; P5-UX-5 adds it back pointing at Add people.
   - Each remaining link gets a test that its href resolves to an existing route (a route-table test over
     `src/app`).
6. **First-run fixes (C-916, `14 §First run and connecting` §2):**
   1. Chat empty state: "Say hello to <Agent>" and the handle, not the brief (the chat component's empty
      view).
   2. The message editor toolbar wraps at any width, with no clipped buttons at about 1250 px or at
      390 px.
   3. The tab bars of Credentials and Settings show no stray scrollbar widget (the tab primitive's
      overflow styles; fix it in the primitive).
   4. Connect-form labels never print "(optional) (optional)" (the label builder).
   5. Help links render "Where do I find this?" without the URL in the text (the field renderer).
   6. The agent header names the computer state in words: Off, Starting, On, Stopping, Failed, with
      the reason on hover or tap.
   7. The admin landing never flashes "You are not in an organization yet" before redirecting to
      `/admin/setup` (gate the root page's empty-org view on the setup-status query).
7. **Tests.** Vitest for:
   - the hook;
   - `errorMessage` over `ApiError`, plain objects and transport errors;
   - `EmptyState` typing (a `// @ts-expect-error` case for `ask` without `canAct`);
   - each fix.

   Playwright: a member sees no Credentials or Apps in the org nav; a member on `/o/<org>/members`
   (direct URL) reads "Only organization owners and admins can open this page."; an owner's empty
   Secrets shows **Add a secret** and no "Ask" line.

## Constraints / file ownership

- Owns: `app-shell.tsx`, `org-gate.tsx`, the listed gate callers, `states.tsx`, `errors.ts` (shared
  with P5-UX-3: append), `lib/api/roles.ts` (new), the empty-state callers, `getting-started.tsx`
  (`OptionalNext` only), and the files of the seven fixes.
- Do not touch `connect-dialog.tsx` beyond the label and help-link renderers it shares with other forms
  (P5-UX-3 owns the dialog).
- `14 §Visual design` for every changed screen.

## Acceptance criteria

1. No sidebar item and no Welcome link leads a member to a 403 or a 404 (Playwright over a member and an
   owner).
2. A 403 never renders the network sentence (unit plus Playwright); a real network failure still does.
3. No `EmptyState` renders an "Ask…" line to a viewer who can act (unit over every caller).
4. Each of the seven fixes has a test or a screenshot check in the narrow-route suite.
5. Typecheck, lint, vitest and `npm run build` green; axe and narrow-route suites green, both themes.

## Dependencies

None. It can run first; P5-UX-3 and P5-UX-5 later replace the interim Welcome links.

## Suggested agent/model and reason

Sonnet (`nextjs-developer`). Many small, well-located changes across shared components; the risk is in
the shared primitives (`EmptyState`, tabs, errors).
