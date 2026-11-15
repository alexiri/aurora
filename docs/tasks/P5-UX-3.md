# P5-UX-3: "Give <Agent> <App>": the connect journey, dashboard (owner walkthrough, 2026-11-15)

## Goal

Replace "make a connection on the Credentials page, then find the agent's Access tab and give it access"
with one dialog that starts from the agent and ends with the agent able to use the app. The owner
singled this direction out after the first-run walkthrough ("I really like … `One "Connect" journey per
app`"). People choose only **which app**, **what the agent may do** (Read only / Read and change, asking
you first) and, where the app offers one, a **limit**. They then sign in with the one method that works
for them. The current controls stay under **Advanced**.

## Linked requirements

- D-337 (the journey), D-321 (plan, suggested method), D-322 (grant in the connect request), D-324(a)
  (preselection in the agent step).
- Contracts **C-914**, **C-927** (journey parts); reads C-955, C-957, C-958, C-959, C-917.
- Normative: `14 §First run and connecting` §1 (all steps, entry points, failure sentences), `14 §Visual
  design`, `14 §Key flows` (the C-842 dialog, now under Advanced; Reconnect unchanged).

## Context

- **Today's dialog:** `web/src/components/apps/connect-dialog.tsx`.
  - Routing is at ~742–801: `DeploymentStep`, `ReconnectForm`, `CredentialFreeForm`, `ConnectForm`.
  - `ConnectForm` holds the method `ChoiceCards`, fields, scopes (`ScopeGroups`, FIX-136/147), access
    (FIX-133), the OAuth client select and the FIX-149 sign-in window with `BroadcastChannel`.
  - `ConnectedStep` wraps `GiveAccessPanel` (`web/src/components/credentials/give-access.tsx`).
- **Agent Access tab:** `web/src/components/agents/access-tab.tsx` and `connection-grants.tsx`. That file
  holds the empty state "Connect an app on the Credentials page…" (~74), the FIX-154 credential-free rows
  and `AppGrantEditor`.
- **Deep link handling:** `useUrlDialog`, used by the Apps tab for `?connect=<app>` (C-842).
- **API client:** `web/src/lib/api/apps.ts` (`useApps`, `useApp`, connect mutation) and
  `credential-grants.ts`.

## Scope

1. **`ConnectJourney` component** (`web/src/components/apps/connect-journey.tsx`, with steps in their
   own files) implementing `14 §First run and connecting` §1. It works for a fixed agent (from the agent
   page) or with an **agent step** (from the Apps tab).
   - **App step:** tiles from `useApps(ws)` plus one `connect-plan` query per tile on demand (on focus or
     click; never N plans on open), grouped Ready / Already connected / Needs setup by an admin.
     - A blocked tile shows the `unlock` sentence and **Copy request** (clipboard with a fallback to
       selecting the text).
     - Families stay one tile (reuse the FIX-135 deployment step after the tile).
     - Credential-free apps go straight to FIX-154's grant path.
   - **Account step:** from `plan.existing`. "Use <label>" saves only a grant through the existing
     `PUT …/grants/connections/{conn}` with the chosen preset, expanded the same way: call the server
     expansion if P5-UX-2 exposes it on the grant route, else the C-957 path.
   - **Access step:** `ChoiceCards`. **More choices** reveals `full`, never shown for `custom_only`. Limit
     fields come from `plan.limits`. The step is skipped for read-only apps.
   - **Sign-in step:** only `plan.suggested_method`'s form:
     - `oauth2` → **Sign in to <App>** with the FIX-149 window and `BroadcastChannel`, reusing that
       code; do not fork it;
     - `fields` / `hook` → fields, with help links as button-like links ("Create a token on <App>"),
       never raw URLs.

     **Advanced** renders the existing `ConnectForm` controls (method chooser with blocked methods
     disabled and their reason, scopes, OAuth client, label). Extract them from `connect-dialog.tsx` into
     reusable pieces rather than duplicating them. A blocked suggested method shows the `unlock` sentence
     instead of a form, plus **Set it up yourself** under Advanced when `unlock.caller_can`.
   - **Submit:** `POST /v1/apps/{app}/connect` with `access`, the preset's `scopes` (oauth2), the limit
     fields and `grant { agent_ids, preset }`.
   - **Done step:** sentences per §1. A `grant_failed` result (from the response or the
     `BroadcastChannel` message) shows the connection as made but not given, and **Give access** retries
     the grant only.
   - **Failures:** C-958 sentences via `ERROR_MESSAGES` (keys added by P5-UX-1 or here, whichever lands
     first; coordinate through the shared file `web/src/lib/api/errors.ts`, append only). Typed non-secret
     values survive a failure.
2. **Entry points.**
   - Agent page › Access: an **Add an app** button in the Connections section; the "No connections in
     scope" empty state becomes that button.
   - The deep link `?tab=access&connect=<app>`, or `connect=` empty for the app step.
   - Credentials › Apps **Connect** opens the journey with the agent step, `?connect=<app>` kept.
   - Welcome and Overview guide "Connect an app" → the default agent's journey (C-927; replaces P5-UX-1's
     interim link).
   - The tour's Agents stop text (C-927, `web/src/components/onboarding/tour.tsx`).
3. **Retire `ConnectForm` as an entry.** It no longer opens from the Apps tab; its pieces live under
   Advanced. `ReconnectForm` and the reconnect deep link stay unchanged.
4. **Tests.**
   - **Vitest:**
     - the step machine (skips: read-only app, no existing connection, a single method, a given app);
     - the preset payloads;
     - blocked rendering from a plan fixture;
     - `grant_failed` → retry calls the grant route only;
     - the deep links.
   - **Playwright** (kind or the mocked e2e, whichever the repo's journey specs use):
     - from the agent page, connect a `fields` app with Read only → the agent's Access tab shows the
       connection granted;
     - an OAuth app with no client shows the unlock sentence and no form;
     - Apps-tab Connect with one agent → the agent is preselected.
   - **Axe and narrow-route** entries `ws-connect-journey` (both themes, 390 px).

## Constraints / file ownership

- Owns: `web/src/components/apps/connect-journey*.tsx` (new), `connect-dialog.tsx` (extraction only),
  `web/src/components/agents/{access-tab,connection-grants}.tsx` (entry points),
  `web/src/components/apps/apps-tab.tsx` (Connect opens the journey),
  `web/src/components/onboarding/{getting-started,tour}.tsx` (C-927 links and text only),
  `web/src/lib/api/apps.ts` (plan query, connect payload).
- Shared with P5-UX-1 and P5-UX-4: `errors.ts` (append keys) and `give-access.tsx` (P5-UX-4 owns the
  preselection; this task only passes the contextual agent). Do not run beside P5-UX-4.
- Tokens and shared primitives only (`14 §Visual design`). Any missing primitive (for example a tile grid)
  is added to the shared set, not styled inline.

## Acceptance criteria

1. From an agent's Access tab, giving that agent a token app with Read only takes: **Add an app** → the
   app → **Read only** → the token → **Connect**. That is three screens. The Done step names the agent and
   the app, and the agent holds a grant (verified via `GET /v1/agents/{agent}/grants`).
2. An OAuth app whose sign-in has no client never shows a form unless the person opens Advanced. It shows
   who can fix it, with names.
3. **Read and change, asking you first** produces a custom grant whose write tools are `ask` (verified via
   the grants route).
4. From the Apps tab with one agent in the workspace, that agent is preselected and the button reads
   **Give Daneel access**.
5. A wrong token reads "<App> did not accept this token…"; the typed label survives.
6. Every C-842 control is reachable under Advanced; Reconnect is unchanged (its existing tests pass
   untouched).
7. Light and dark, 390 px without horizontal scroll, axe-clean; `npm run typecheck`, lint, vitest,
   `npm run build` green.

## Dependencies

- **P5-UX-2** merged (plan, `grant`, C-958 details, regenerated types).
- P5-UX-4 for the agent step's preselection. If P5-UX-4 has not merged, implement the preselection rule
  locally in the journey's agent step and leave `give-access.tsx` to P5-UX-4.

## Suggested agent/model and reason

Sonnet (`nextjs-developer`). It is a multi-step UI with extraction from a large existing component and an
OAuth window round trip, so it needs judgement but no backend change.
