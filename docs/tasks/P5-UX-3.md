# P5-UX-3: "Give <Agent> <App>": the connect journey (owner, 2026-11-15)

## Goal

Replace "make a connection on one page, then find the agent's Access tab and give it access" with one
dialog that ends with the agent able to use the app. The owner singled this direction out after the
first walkthrough ("I really like … one Connect journey per app").

A person chooses **which app** (or another website), **what the agent may do** (Read only, Ask before
changes, Change without asking), and signs in with the one way that works for them. The account and the
agent's access are made in one request. The existing controls stay under **Advanced**.

## Linked requirements

- **Design** (D-354): the canvas “Aurora — agents, accounts and access”, https://claude.ai/artifact/3UwmUgrhh6MLCsfqdhTFmQ, boards Agent › Give Daneel an app (the journey); Workspace › Apps & accounts › Apps and › Accounts (the Add menu); Read me › Patterns (Done sentences). Read each board's note; where a board and the spec disagree, the spec wins.
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

## Completion report (partial, lane p5-apps, 2026-11-17)

**Done (commit 6de40829, web only; `pnpm typecheck` clean, `eslint src tests` 0 errors, full `vitest run` before the last fixes: 2063 pass, the 4 failures it showed were fixed and re-run green by file — a final full run was not repeated):**
- `components/apps/connect-journey/`: `connect-journey.tsx` (dialog, step state, existing-account save, grant_failed retry, Done), `app-step.tsx` (tiles in three groups, plan read on focus/click only, blocked tile with `WhoCan`, family one tile and `DeploymentStep` only when a self-hosted server is registered), `sign-in-step.tsx` (suggested method only; **Sign in to <App>**; **Create a token on <App>** + "tick" line from `scope_presets`; `CopyValue` for `show_context`; blocked method → `WhoCan`; **Advanced** with the other methods (blocked ones disabled with the reason), scopes, OAuth client, instance, **Use a self-hosted <App>**, setup guide; `WebsiteStep` for **Another website or API**, saved through C-988 with `grant`), `logic.ts` (pure: steps and skips, levels, Done sentences, tick list, payload, C-958 sentences, host), `advanced-fields.tsx` and `use-sign-in-window.ts` (extracted from `connect-dialog.tsx`; Reconnect's `ConnectForm` uses them, no duplicate).
- Entry points: Accounts › Add › **Connect an app or website**, agent Access › **Give <Agent> an app**, empty states, the Apps tab's **Connect**, and the deep links `?tab=access&connect=<app>`, `?tab=apps&connect=<app>`, `?connect=` (empty = App step). `appsHref` removed. Only owners see the buttons.
- API client: `useConnectPlan`, `ConnectChannelMessage.connected` gains `grantedAgentIds`/`grantFailed`, `buildGenericCreate` takes `agentIds`; `ChoiceCards` option `disabled` (shared primitive).
- C-942: the Bedrock assume-role form shows the organization's external id as `CopyValue` above its fields (the trust-policy snippet stays below).
- Vitest: `tests/connect-journey.test.tsx` (22: steps/skips, Done sentences, payload per access, blocked tile, blocked method, existing account saves only access, grant_failed retry, website one request, one-agent preselection); `connect-dialog.test.tsx` rewritten for the journey (token, OAuth window, channel, Reconnect unchanged); `accounts`, `dashboard-links`, `policy`, `apps`, `ui-text` tests follow the new buttons.

**Closed in the second session (2026-11-17, commits 12e4baaf, 7d3066f5 and the gate fix):**
1. **Playwright.** `e2e/support/mock-api.ts` serves `connect-plan` (Sentry's sign-in is not set up → the blocked case), the methods' `audience`/`scope_presets`/`show_context`, and `extraAccounts` (accounts a spec's saves made); `e2e/support/journey.ts` records the connect request, the website's C-988 request and the access PUTs. New `e2e/connect-journey.spec.ts` (5 cases × 2 themes × 2 widths = 20): agent Access tab → GitHub → Read only → token → Done → the Access tab lists the account with Read only; Ask before changes (level `ask`, write permissions); an app with no sign-in shows who can set it up and no form; one-agent Apps & accounts (Daneel preselected, **Give Daneel access**); **Another website or API** in one request. Axe + overflow at 1280 and 390 in both themes on every step. `connect-dialog`, `connect-access`, `connect-scopes`, `grant-form`, `ia-navigation` and `who-can` follow the journey; `stage4-axe` has `ws-connect-journey`, `ws-connect-journey-apps`, `ws-connect-journey-agent`.
2. **Two real defects found by the specs and fixed:** an app tile's `aria-label` differed from its visible text (axe `label-content-name-mismatch`; the tile now takes its name from its content), and the Done step's Access dialog no longer got the new account's scopes and method (FIX-136's "Not available with this connection's token" list) — `SignInResult`/`DoneState` carry `scopes` and `methodId` again.
3. **Website host/path/recipe editing:** `components/accounts/edit-website.tsx`, row menu **Change website** (website accounts): Website, and under Advanced the path and the way the token is sent; PATCH of the website's own record (read from the secrets list when the dialog opens); the name stays Rename's. Vitest in `tests/accounts.test.tsx`.
4. **Batch gate** (web only in this task; no Rust or API change since the last gate): clippy workspace `-D warnings` 0; `guards/run-all.sh` exit 0; `extract-schema.py --check` up to date; `openapi_matches_checked_in` ok, `gen:api` no diff; web typecheck clean, lint 0 errors (4 old warnings), vitest 123 files / 2068 tests, `pnpm build` ok; Playwright mocked 1021 tests: 1017 passed, 4 failed (`who-can` owner empty state looked for a *link* to Connect, now a button; fixed, 12/12 rerun), production project 8/8. `hosted.sh` not run: it does not exercise the dashboard.

**Still open:**
4. Design boards were read (Journey board: step bar, three-column tiles) but not compared pixel by pixel. **Custom…** (the actions table) is not offered inside the journey's Access step (needs an existing account; it stays on the agent's row afterwards). The Advanced "account name" is not offered: `ConnectRequest` has no name field.
5. `ConnectDialog` still routes new connects (DeploymentStep, CredentialFreeForm) for non-reconnect use; nothing opens it that way any more (dead path, kept so the old pieces stay tested; trim in P5-UX-14).
6. Brief's fail-before check was not run for the new tests (the new journey has no pre-existing behaviour to fail against).
