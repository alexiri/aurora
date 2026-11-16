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
