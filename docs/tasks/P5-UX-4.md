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
  a Custom view per tool;
- a **limit** only on shared accounts.

## Linked requirements

- D-344, D-345, D-351; SEC-131 (the access part).
- Contracts **C-917, C-925, C-938, C-948, C-980**.
- Normative: `14 §Agents, accounts and access` §1 and §2, `14 §Information architecture` (vocabulary),
  `05` (accounts and grants rows), `07` (grants compile into rules; read-only connections, D-240), C-410
  (`custom_only`), C-504 (bulk grant).

## Wave and order

- **Wave 1.** Starts after P5-UX-2 (level expansion, `WhoCan`, C-988) and P5-IA-1 (the Accounts tabs and
  routes).
- Consumers: P5-UX-3 (uses `AccessControl` and the picker), P5-UX-9, P5-UX-10, P5-UX-11, P5-UX-14,
  P5-UX-16, P5-UX-7.
- Not beside P5-UX-1 on `components/states.tsx`; otherwise disjoint from the other wave-1 briefs.

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
   - `shared`, `limit` and `levels` per C-917; a level reads as it takes effect.
   - `managed_by` for shared accounts (`WhoCan`).
   - `health` uses the connection status until P5-UX-14 adds the check fields: `sign_in_expired`,
     `disabled`, else `unknown`.
   - `model` on AI-key rows comes from the model readiness function.
   - OpenAPI, generated types, `permission_matrix` rows.
2. **Access API (C-980, SEC-131).** `PUT /v1/agents/{agent}/grants/connections/{conn}` accepts `level`.
   - Not shared: the connection's `access` is set in the same transaction.
   - Shared: a level above the limit → 422 `above_account_limit` with `who_can`.
   - `mode: "custom"` with tool lists stays, with the same two rules.
3. **`AccountsList` (C-925).** One component over C-917 with the rows, kind chips, "who can use it",
   health line, "Shared by <org>", the row menu and the "No agent can use this" badge of `14` §1.
   - **Workspace Accounts tab:** replaces the lists P5-IA-1 hosted there. **Add** has two entries:
     - **Connect an app or website** opens the Apps tab in this wave; P5-UX-3 points it at the journey.
     - **Add an AI key** opens the existing form, with the agents picker ticked per the preselection
       rule, saved through C-988.
   - **Agent Access tab:** the same list with `agent_id`. Accounts the agent can use come first, with
     `AccessControl` in the row; "Not given to <Agent> (n)" follows, collapsed. The AI-key row shows the
     model line. The Connections, Secrets and Effective access sections are removed.
   - **Organization Accounts tab:** the same list, read from the organization route. P5-UX-11 adds the
     organization's actions.
4. **`AccessControl` and Custom (C-938).**
   - Three `ChoiceCards` from the account's `levels`; "Limited by the shared account" with `WhoCan` when
     choices are cut; no control for accounts with no choice.
   - **Custom…** opens the per-tool editor, starting from the current choice. Rows show the tool's name
     and description; method and host pattern sit under **Technical details**. At least one tool must be
     allowed or asked.
   - A `custom_only` app (C-410) opens Custom directly, with reads allowed.
   - One save applies the choice (C-980). Nothing else on the page needs to change to make it take
     effect.
5. **Pickers (C-925, C-948).** `GiveAccessPanel` and `AccessDialog` take the contextual agent and
   preselect it, else the only agent the person may choose, else none. The button names the result
   ("Give Daneel access"). Rows read "<workspace> · <agent>" when agents of more than one workspace are
   listed. The picker includes `AccessControl` for accounts that have a choice.
6. **Words.** Every string of these components follows the vocabulary: account, App, Website, AI key,
   access, limit. No rule id, host pattern or "secret" outside Technical details.
7. **Tests.**
   - API:
     - the list for a workspace with an app account, a website, an AI key and an organization's account;
     - `shared` and `limit` for one agent, two agents and an organization's account;
     - a saved choice above a lowered limit reads as the limit;
     - `PUT level` for a non-shared account sets the connection's `access`; with a second agent it does
       not; above the limit → 422;
     - `act` on `custom_only` → 422;
     - a non-owner → 403;
     - no N+1 (query count).
   - Vitest: the row for each kind; the Access tab's two groups; `AccessControl` with full and cut
     `levels`; Custom's defaults for each starting choice; picker preselection (context, one agent,
     several, none); row format.
   - Playwright:
     - the walkthrough's case: a read-only AWS account with writes set to ask shows **Read only**, and
       choosing **Ask before changes** is one click and one save, after which a write asks;
     - saving a website in a one-agent workspace gives that agent access without a second step;
     - a second agent given an account makes its row show the limit.
   - Axe and narrow-route suites `ws-accounts` and `agent-access`, both themes, 390 px.

## Constraints / file ownership

- Owns: the accounts routes (a new `domains/accounts` or next to `connections`); the `level` path of
  `domains/grants`; `components/accounts/*` (new: `AccountsList`, `AccessControl`, the row);
  `access-tab.tsx`, `connection-grants.tsx`, `grant-editor.tsx`, `grant-form.tsx`, `lib/grant-editor.ts`,
  `give-access.tsx`, `llm-form.tsx` and `generic-form.tsx` (picker pass-through); the Accounts tab
  content of the workspace and organization pages.
- The old list components (`credentials-view.tsx`, `connections-tab.tsx`, `secret-row.tsx`) are removed
  from the pages; delete them when nothing imports them.
- No data migration: saved grants are read through the level mapping.

## Acceptance criteria

1. A workspace has one list of what its agents can use, and an agent's Access tab is that list filtered.
2. For an account one agent uses, access is one choice and one save; no state exists in which the choice
   and the connection disagree.
3. A shared account shows its limit, and choices above it are not offered.
4. No picker opens with the obvious agent unticked; no account that nobody can use goes unmarked.
5. Rust and web gates; axe and narrow-route suites, both themes.

## Suggested agent/model and reason

Sonnet, as one lane: Rust first (two routes), then the components. The level and limit rules need a
careful reviewer against SEC-131.
