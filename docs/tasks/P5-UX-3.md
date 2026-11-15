# P5-UX-3: "Give <Agent> <App>": the connect journey (owner, 2026-11-15)

## Goal

Replace "make a connection on one page, then find the agent's Access tab and give it access" with one
dialog that ends with the agent able to use the app. The owner singled this direction out after the
first walkthrough ("I really like … one Connect journey per app").

A person chooses **which app** (or another website), **what the agent may do** (Read only, Ask before
changes, Change without asking), and signs in with the one way that works for them. The account and the
agent's access are made in one request. The existing controls stay under **Advanced**.

## Linked requirements

- D-337, D-321, D-322, D-344, D-345, D-353; contracts **C-914, C-942 (the Copy blocks)**; reads C-917,
  C-955, C-957, C-958, C-988.
- Normative: `14 §Agents, accounts and access` §3 (the button labels, steps, Done and failure sentences),
  §2 (the access choice), `14 §Visual design`, `14 §Key flows` (the C-842 dialog, now under Advanced;
  Reconnect's behaviour unchanged).

## Wave and order

- **Wave 2.** Starts after P5-UX-2 (plan, connect with access, C-988), P5-UX-4 (`AccessControl`, the
  picker, the Accounts list), P5-UX-13 (`CopyValue`) and P5-IA-1 (routes).
- P5-UX-9 follows it in the same web lane (both edit `apps-tab.tsx`).
- Consumers: P5-UX-7 (Welcome), P5-UX-11 (organization mode), P5-UX-17 (the agent's request card).

## Context

- **Today's dialog:** `web/src/components/apps/connect-dialog.tsx`.
  - Routing ~742–801: `DeploymentStep` (families), `ReconnectForm`, `CredentialFreeForm`, `ConnectForm`.
  - `ConnectForm` holds the method `ChoiceCards`, the fields, scopes (`ScopeGroups`), the read-only
    choice (FIX-133), the OAuth client select and the FIX-149 sign-in window with `BroadcastChannel`.
- **Access tab and Accounts list:** P5-UX-4's `AccountsList`, with **Add › Connect an app or website**
  pointing at the Apps tab for now.
- **Deep links:** `useUrlDialog`, used by the Apps tab for `?connect=<app>` (C-842).
- **API client:** `web/src/lib/api/apps.ts` (`useApps`, `useApp`, the connect mutation).

## Scope

1. **`ConnectJourney`** (`web/src/components/apps/connect-journey/*`), implementing `14` §3. It runs for
   a fixed agent (from the agent's page) or with an **agent step** (from Apps & accounts).
   - **App step.** Tiles from `useApps(ws)`, grouped Ready to connect / Already connected / Needs setup
     by an admin, with **Another website or API** as the last tile.
     - A tile's plan is fetched on focus or click, never all at once.
     - A blocked tile shows the plan's `WhoCan` sentence and cannot be opened.
     - Families stay one tile. The hosted service is assumed; the FIX-135 deployment step ("Where does
       your <App> run?") is shown only when a self-hosted server is registered. Otherwise **Use a
       self-hosted <App>** under Advanced links to the app's setup page.
     - Apps with nothing to sign in to go straight to FIX-154's one-step path.
   - **Account step.** From the accounts API (`GET …/accounts?app_id=`, C-917), the accounts of this app
     the person may give: "Use <name> (connected by Bob)" (recommended) or "Connect another account".
     Using an existing account skips only the Sign-in step: the Access step follows, and the save is the
     agent's access alone (C-980).
   - **Access step.** P5-UX-4's `AccessControl` with the three choices, Read only preselected. Skipped
     for websites and apps that can only read.
   - **Sign-in step.** Only the suggested method's form:
     - sign-in methods: **Sign in to <App>**, reusing the FIX-149 window and `BroadcastChannel` code;
     - token methods: the fields and **Connect**; the link to the app's token page as a button, **Create
       a token on <App>**, followed by the permissions to tick for the chosen access (from the method's
       `scope_presets`); other help links read **Where do I find this?**; `CopyValue` blocks for the
       method's `show_context` values;
     - **Another website or API**: Name, Website, Token, with **Advanced** for the path and the other
       ways to send the token (`08 §5`), saved through C-988;
     - no suggested method: the `WhoCan` sentence instead of a form.

     **Advanced** holds the other methods (blocked ones disabled, with their reason), the scopes, the
     OAuth client select and the account's name. Extract these from `connect-dialog.tsx` into reusable
     pieces; do not duplicate them.
   - **Submit.** `POST /v1/apps/{app}/connect` with the method, the fields, the scopes of the chosen
     level (`scope_presets`) and `grant { agent_ids, level }`.
   - **Done step.** The five Done sentences of `14` §3, one per access, with **Open chat** and **Give
     <Agent> another app**. A `grant_failed` result (from the response or the `BroadcastChannel`
     message) shows the account as made but not given; **Give access** retries that part only.
   - **Failures.** The C-958 sentences through `ERROR_MESSAGES`. Typed values that are not secret survive
     a failure.
2. **Entry points**, with exactly the labels of the table in `14` §3:
   - Agent page › Access: **Give <Agent> an app**; the deep link `?tab=access&connect=<app>` (empty for
     the app step).
   - Apps & accounts › Accounts: **Add › Connect an app or website**.
   - Apps & accounts › Apps: **Connect** on an app (`?tab=apps&connect=<app>`).
3. **Retire `ConnectForm` as an entry.** Its pieces live under Advanced. `ReconnectForm` and the reconnect
   deep link keep their behaviour.
4. **The Bedrock form's Copy block (C-942).** `components/credentials/llm-form.tsx` shows the
   organization's external id ("Your organization's external id") with `CopyValue` above the
   assume-role fields, with the trust-policy snippet C-811 describes, as the journey's sign-in step
   does for `show_context`.
5. **Words.** The journey says account, access and website. It never says connection, grant, credential,
   scope (outside Advanced) or secret.
6. **Tests.**
   - Vitest:
     - the step machine and its skips (a read-only app, no existing account, an existing account, a
       given app, a website, a family with and without a self-hosted server);
     - each Done sentence; the permissions line for Read only and for Ask before changes;
     - the payload for each access choice;
     - a blocked tile and a blocked suggested method from plan fixtures;
     - `grant_failed` → retry saves only the access;
     - the deep links.
   - Playwright:
     - from an agent's Access tab, give the agent a token app with Read only; the agent's Access tab
       then lists the account with **Read only**;
     - an app whose sign-in is not set up shows who can set it up and no form;
     - from Apps & accounts in a one-agent workspace, the agent is preselected and the button reads
       **Give Daneel access**;
     - **Another website or API** ends with the agent able to use the website.
   - Axe and narrow-route entries `ws-connect-journey`, both themes, 390 px.

## Constraints / file ownership

- Owns: `components/apps/connect-journey/*` (new); `connect-dialog.tsx` (extraction only);
  `components/apps/apps-tab.tsx` (Connect opens the journey); the Add entry of the Accounts list; the
  Access tab's **Give <Agent> an app** button; `web/src/lib/api/apps.ts` (plan query, connect payload);
  the Copy block of `llm-form.tsx`.
- Uses, and does not change, `AccessControl`, the picker and `CopyValue`.
- Tokens and shared primitives only (`14 §Visual design`). A missing primitive (for example a tile grid)
  is added to the shared set.

## Acceptance criteria

1. From an agent's Access tab, giving that agent a token app with Read only takes: **Give <Agent> an
   app** → the app → the token → **Connect**. The Done step names the agent and the app, and the agent
   can use it.
2. An app whose sign-in is not set up never shows a form unless the person opens Advanced; it shows who
   can fix it.
3. **Ask before changes** results in access where every change asks (verified through the accounts list).
4. A wrong token reads "<App> did not accept this token…", and the typed name survives.
5. Every C-842 control is reachable under Advanced; Reconnect behaves as before (its tests pass; its
   texts are P5-UX-14's).
6. Typecheck, lint, vitest and `npm run build` green; axe and narrow-route suites, both themes.

## Suggested agent/model and reason

Sonnet (`nextjs-developer`). A multi-step dialog with extraction from a large existing component and an
OAuth window round trip.
