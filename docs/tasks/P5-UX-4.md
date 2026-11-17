# P5-UX-4: One list of accounts, one access choice (owner, 2026-11-15)

## Goal

After the walkthroughs, "what can Daneel use, and what may it do there?" had too many homes:
- three Credentials tabs (Secrets, LLM keys, Connections);
- three sections of the agent's Access tab (Connections, Secrets, Effective access, with rule ids);
- two access settings for one app account: the connection's read-only switch and the per-tool grant,
  which could disagree ("3 tools set to Ask" on a read-only connection);
- a "Give agents access" panel that listed the workspace's only agent unticked.

This task builds the heart of the new model (D-344, D-345):
- **one list of accounts** (apps, websites, AI keys), used by the workspace, by each agent's Access tab
  and by the organization;
- **one access choice** per agent and account: Read only, Ask before changes, Change without asking, with
  one table of every action for the rare case that needs it;
- a **limit** only on the organization's accounts.

## Linked requirements

- **Design** (D-354): the canvas “Aurora — agents, accounts and access”, https://claude.ai/artifact/3UwmUgrhh6MLCsfqdhTFmQ, boards Agent › Access, Access › Custom… and Access at 390 px; Workspace › Apps & accounts › Accounts; Read me › Patterns (account row, access switch). Read each board's note; where a board and the spec disagree, the spec wins.
- D-344, D-345, D-351, D-353; SEC-131 (the access part).
- Contracts **C-917, C-925, C-938, C-948, C-980**.
- Normative: `14 §Agents, accounts and access` §1 and §2, `14 §Information architecture` (vocabulary),
  `05` (accounts and grants rows), `07` (grants compile into rules; read-only connections, D-240), C-410
  (`custom_only`), C-504 (bulk grant).

## Wave and order

- **Wave 1.** Starts after P5-UX-2 (level expansion, the `WhoCan` shape, C-988), P5-IA-1 (the Accounts
  tabs and routes) and P5-UX-1 (the `WhoCan` component, `components/states.tsx`).
- Consumers: P5-UX-3 (uses `AccessControl` and the picker), P5-UX-9, P5-UX-10, P5-UX-11, P5-UX-14,
  P5-UX-16, P5-UX-17, P5-UX-7.
- Disjoint from P5-IA-2 and P5-UX-13, which run beside it.

## Context

- **Lists today.** Secrets and LLM keys: `services/aurora-api/src/domains/secrets`; connections:
  `domains/connections`; grants: `domains/grants/{routes,service,compile,effective}.rs`
  (`GrantMode` full|custom, `allow_tools`, `ask_tools`).
- **Connection access.** `app_connections.access` (`read`|`write`, FIX-133, D-240), changed by
  `PATCH /v1/connections/{id}` (C-740). The gateway blocks every write on a read-only connection.
- **Level expansion.** P5-UX-2 added one function that expands `read`, `ask`, `act` into a grant and one
  that derives a connection's `access` from levels.
- **Web.**
  - Hosted by P5-IA-1 under the Accounts tab: `components/credentials/credentials-view.tsx`,
    `secret-row.tsx`, `components/apps/connections-tab.tsx`.
  - Access tab: `components/agents/access-tab.tsx`, `connection-grants.tsx`, `grant-editor.tsx`
    (read-only note ~155–160), `model-readiness.tsx`.
  - Pickers: `components/credentials/give-access.tsx` (`GiveAccessPanel` ~205–341, `AccessDialog` ~371,
    `AgentPicker` ~145–171).
  - Per-tool form: `components/grants/grant-form.tsx`, `lib/grant-editor.ts` (`initialDecisions` ~64–71).
  - AI-key form: `components/credentials/llm-form.tsx`.

## Scope

1. **Accounts API (C-917).** `GET /v1/workspaces/{ws}/accounts` and `GET /v1/orgs/{org}/accounts`, with
   the `Account` and `AgentAccess` shapes of C-917.
   - One query per list, with joins over connections, secrets, grants and agents.
   - `limit` (organization accounts only, derived here from the connection's `access`: `read` → `read`,
     `write` → `act`; P5-UX-11 adds the stored three-value limit) and `levels`, per C-917. A level reads
     as it takes effect.
   - `level`, `always_allowed` and `custom` per C-917: a saved choice that is Ask before changes plus
     some changes allowed reads as `ask` with those in `always_allowed`; anything else that is not a
     level reads as `custom`.
   - `managed_by` for the organization's accounts (`WhoCan`).
   - `health` uses the connection status until P5-UX-14 adds the check fields: `sign_in_expired`,
     `disabled`, else `unknown`.
   - `model` on AI-key rows comes from the model readiness function.
   - OpenAPI, generated types, `permission_matrix` rows.
2. **Access API (C-980, SEC-131).** `PUT /v1/agents/{agent}/grants/connections/{conn}` accepts `level`.
   - Choosing a level replaces what was saved, always-allowed changes included.
   - A workspace's account: the connection's `access` follows its agents in the same transaction (`read`
     when every agent's access is `read`, else `write`), also when access is removed.
   - An organization's account: a choice above the limit → 422 `above_account_limit` with `who_can`.
   - `mode: "custom"` with tool lists stays, with the same rules.
3. **`AccountsList` (C-925).** One component over C-917 with everything `14` §1 lists for a row: kind
   chip, "who can use it" (each agent's entry opens the access choice and **Remove access**), the health
   line, "Shared by <org>", the "No agent can use this" badge, the menu (Give access, Rename, Reconnect
   or Replace, Test, Turn off or Turn on, Remove), and the empty states. Leave a slot above the list for
   the requests banner (P5-UX-17 fills it). People who cannot change an account see its values as text
   and no controls (`14` "What a person sees of something they cannot do").
   - **Workspace Accounts tab:** replaces the lists P5-IA-1 hosted there. **Add** has two entries:
     - **Connect an app or website** opens the Apps tab in this wave; P5-UX-3 points it at the journey.
     - **Add an AI key** opens the existing form, with the agents picker ticked per the preselection
       rule, saved through C-988.
   - **Agent Access tab:** the same list with `agent_id`. Accounts the agent can use come first, with
     `AccessControl` in the row; "Not given to <Agent> (n)" follows, collapsed. The AI-key row shows the
     model line. The button is **Give <Agent> an app** (the Apps tab in this wave; the journey after
     P5-UX-3). The Connections, Secrets and Effective access sections are removed.
   - **Organization Accounts tab:** the same list, read from the organization route, with **Add › Add an
     AI key** (the existing organization key form). P5-UX-11 adds connecting for the organization and
     the organization's actions.
4. **`AccessControl` and the actions table (C-938).**
   - Three `ChoiceCards` from the account's `levels`, in the fixed order and with the sentences of `14`
     §2; "Limited by <org>" with `WhoCan` when choices are cut; no control for accounts with no choice.
   - Under Ask before changes, the list "Always allowed: <action>, <action>", each with a remove button
     that makes the agent ask again (a save that moves that tool from allowed back to asked).
   - "Custom: <n> actions differ from <level>" when the saved choice is custom.
   - **The actions table**: each tool's name and description with its saved choice (Allowed, Asks you,
     Blocked), editable by people who may change the access; method and host pattern under **Technical
     details**; at least one tool allowed or asked. It opens from **Custom…** under the choices. Setting
     a change to Allowed under Ask before changes saves an always-allowed change, not Custom.
   - A `custom_only` app (C-410) opens the table directly, with reads allowed.
   - One save applies the choice (C-980). Nothing else on the page needs to change to make it take
     effect.
5. **Pickers (C-925, C-948).** `GiveAccessPanel` and `AccessDialog` take the contextual agent and
   preselect it, else the only agent the person may choose, else none. The button names the result
   ("Give Daneel access"). Rows read "<workspace> · <agent>" when agents of more than one workspace are
   listed. The picker includes `AccessControl` for accounts that have a choice.
6. **Words.** Every string of these components follows the vocabulary: account, App, Website, AI key,
   access, limit, Turn off. No rule id, host pattern or "secret" outside Technical details.
7. **Tests.**
   - API:
     - the list for a workspace with an app account, a website, an AI key and an organization's account;
     - `limit` only on the organization's account; none on a workspace account two agents use;
     - a saved choice above the limit reads as the limit;
     - a saved choice that is `ask` plus one allowed change reads as `ask` with `always_allowed`; one
       with a blocked read reads as `custom`;
     - `PUT level` for a workspace's account sets the connection's `access` from all its agents (two
       agents, one `read` and one `ask` → `write`; removing the second → `read`); for an organization's
       account above the limit → 422;
     - `act` on `custom_only` → 422;
     - a non-owner → 403;
     - no N+1 (query count).
   - Vitest: the row for each kind, for an owner and for a member (text, no controls); the empty states;
     the Access tab's two groups; `AccessControl` with full and cut `levels`, with always-allowed
     changes and with custom; the table's defaults for each starting choice; picker preselection
     (context, one agent, several, none); row format.
   - Playwright:
     - the walkthrough's case: a read-only AWS account with writes set to ask shows **Read only**, and
       choosing **Ask before changes** is one click and one save, after which a write asks;
     - saving a website in a one-agent workspace gives that agent access without a second step;
     - two agents use one workspace account with different access, and no limit is shown.
   - Axe and narrow-route suites `ws-accounts` and `agent-access`, both themes, 390 px.

## Constraints / file ownership

- Owns: the accounts routes (a new `domains/accounts` or next to `connections`); the `level` path of
  `domains/grants`; `components/accounts/*` (new: `AccountsList`, `AccessControl`, the row);
  `access-tab.tsx`, `connection-grants.tsx`, `grant-editor.tsx`, `grant-form.tsx`, `lib/grant-editor.ts`,
  `give-access.tsx`, `llm-form.tsx` and `generic-form.tsx` (picker pass-through); the Accounts tab
  content of the workspace and organization pages.
- In `components/accounts/`, keep these in separate files, because later briefs each edit one of them:
  the list (with the banner slot, P5-UX-17), the row, the row's menu (P5-UX-11, P5-UX-14), the health
  line (P5-UX-14), `AccessControl`, and the actions table (P5-UX-10).
- P5-UX-13 runs beside this task and does not touch these files; the Bedrock form's Copy block is
  P5-UX-3's, later.
- The old list components (`credentials-view.tsx`, `connections-tab.tsx`, `secret-row.tsx`) are removed
  from the pages; delete them when nothing imports them.
- No data migration: saved grants are read through the level mapping.

## Acceptance criteria

1. A workspace has one list of what its agents can use, and an agent's Access tab is that list filtered.
2. For a workspace's account, access is one choice per agent and one save; no state exists in which the
   choice and the connection disagree, with one agent or several.
3. An organization's account shows its limit, and choices above it are not offered.
4. No picker opens with the obvious agent unticked; no account that nobody can use goes unmarked.
5. Rust and web gates; axe and narrow-route suites, both themes.

## Suggested agent/model and reason

Sonnet, as one lane: Rust first (two routes), then the components. The level and limit rules need a
careful reviewer against SEC-131.

## Completion report (lane p5-apps, worker 23) — PARTIAL: parts 1 and 2 committed, part 3 and the gates open

**Split.** (1) API: accounts lists + level PUT + limit + access sync (scope 1, 2, API tests). (2) Web components and wiring: `AccountsList`, row, menu, health line, `AccessControl`, actions table, Access tab, workspace/organization Accounts tab, picker preselection (scope 3–5, parts of 6). (3) Web tests, words pass, Playwright, old-component removal, batch gate.

**Part 1 — done (ee238481 + the schema rename in part 2's commit).**
- `GET /v1/workspaces/{ws}/accounts`, `GET /v1/orgs/{org}/accounts` (`domains/accounts`): a constant number of queries (connections and secrets of both scopes, one grants+agents+workspaces query, snapshot); `accounts_queries.rs` counts `sqlx::query` events (own test binary, like `db_spans.rs`) and asserts 4 accounts and 40 accounts cost the same.
- `PUT …/grants/connections/{conn}` takes `{ level }` or `{ mode, tools }` (`PutConnectionAccess`); the level is expanded in the transaction from the snapshot. The connection's `access` follows its agents in the same transaction (`sync_connection_access`, on every grant write and on removal). An organization's account refuses a grant above its limit (`above_account_limit`, `who_can` stub + `limit`) on every path that writes a grant (the check sits in `write_connection_grant`; the system's starter grants are exempt). `act` on `custom_only` → `full_grant_forbidden`.
- Added to the contract (written into C-917): `app.custom_only` on `Account`; `model` only on a list read with `agent_id`, only for the agent's own entry and for callers who can own the workspace. The `AccessLevel` schema name was taken, so the per-agent level enum is `AgentAccessLevel`.
- Tests: `it/accounts.rs` (8: all kinds + org account, read-only app and `custom_only` levels, two agents with different access and no limit + access follows + removal recomputes, ask+always_allowed / custom reading, level expansion and bad bodies, `custom_only` 422 + member 403 + org list 403, limit cuts/422 for level, custom, full and the bulk route + org list), `accounts_queries.rs` (1), levels unit tests, permission matrix rows. New tests could not fail "before" in the usual sense (the routes did not exist); `the access follows` test fails without `sync_connection_access`.
- **Changed an existing test:** `simulate_apps::simulate_connection_custom_grant_matches_gateway` wrote writes-asked on a read-limited organization connection, which SEC-131 now refuses; it grants first and lowers the connection's access afterwards (the state "limit lowered later").
- Gates run for part 1: `cargo fmt --check`; `clippy -p aurora-api -p aurora-db --all-targets --all-features -D warnings` clean; `cargo test -p aurora-api --lib` 126 passed; `--test it -- accounts:: grants_ connect permission_matrix openapi simulate_apps` all green (161 + 9); `--test accounts_queries` 1 passed; `scripts/guards/run-all.sh` exit 0; OpenAPI regenerated + `gen:api`.

**Part 2 — committed, typecheck clean, vitest NOT green.**
- New: `lib/api/accounts.ts` (hooks), `lib/accounts.ts` (labels, sentences, preselection rule), `components/accounts/{access-choice,access-control,actions-table,account-health,account-menu,account-row,accounts-list}.tsx` (separate files as the brief asks). `access-tab.tsx` is now the list read for the agent (`AccessTab` takes `agentName`); the workspace Accounts tab shows the list to members too (text, no controls); the organization Accounts tab hosts the same list; `give-access.tsx` takes `contextAgentId`, preselects (context, else the only choosable agent of a workspace's list), names the button ("Give Daneel access") and prefixes rows "<workspace> · <agent>" when several workspaces are listed.
- Gates: `pnpm typecheck` clean, `pnpm lint` 0 errors. vitest: `policy.test.tsx` (AccessTab block replaced by two tests of the new tab; one of them still fails — the member case), and these pre-existing tests fail and need rewriting for the new Access tab / picker button: `grant-editor.test.tsx` (2: read-only connection list, save refreshes readiness on the Access tab), `credential-access.test.tsx` (model readiness on the Access tab ×2, custom_only form, "adds agents with the same picker"), `grant-form.test.tsx` / `onboarding.test.tsx` (button now "Give <Agent> access" instead of "Give access" when one agent is ticked: 3).

**Left open (part 3 and the gates).**
- Vitest for the new components (rows for each kind for owner/member, empty states, Access tab groups, `AccessControl` with full/cut levels, always-allowed, custom, table defaults per starting choice, picker preselection, row format) and the fixes of the failing tests above.
- Playwright: the walkthrough cases, one-agent website save, two agents / no limit; `ws-accounts` and `agent-access` axe + narrow-route suites in both themes; the old `connections-tab.tsx`, `secret-row.tsx` (and `CredentialsView`) are still imported (by `credentials-view.tsx`, `connector-detail.tsx`, tests) and were not deleted.
- **Add an AI key** still uses the existing form's second step (picker with the preselection rule), not the single C-988 request. The words pass (no "secret", rule id, host pattern outside Technical details) was not run over the new strings; the row menu has no **Replace** for websites/AI keys and no separate Test for apps; the AI-key model line is only built for hosted agents of owners. Design boards were not opened.
- The batch gate (full suites, workspace clippy, schema check, web build/lint/vitest, Playwright incl. stage4-axe/routes, ia-navigation, who-can; gateway e2e is not needed: the gateway decision path is unchanged) did not run.
- Process slips to know about: one heredoc append to `crates/aurora-db/src/repo/grants.rs` and one throwaway python block removal in `tests/it/accounts.rs` (both my own new code) in part 1.
