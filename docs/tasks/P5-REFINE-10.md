# P5-REFINE-10: Agent links created by people: pick an agent, choose who may hand tasks to whom (owner, 2026-11-20)

## Goal

UX review 5 (channels-1, blind spot "Linking agents (Peers)") found that **Peers is a dead end**: "No agent links … Tasks
they exchange appear here." with no button, and nothing in the agent menu creates a link. The reason is in the code: a link
is created only by an **agent**, as a side effect of its first `message_agent` call
(`services/aurora-api/src/domains/peer_tasks/service.rs:129–136`, `links::get_or_create`). The owner decided (D-411):

- **People create agent links.** An agent does not know which other agents exist or which it might want to talk to.
- The dashboard needs a **"Link an agent" picker** and a way to choose **who may hand tasks to whom** (mockup:
  "Both ways", "Daneel can hand tasks to Rex", "Rex can hand tasks to Daneel").
- That needs **create and delete routes for people**, rules for **who may link which agents**, and a link policy that says
  who may hand tasks to whom and asks first. The rules are a security decision (SEC-142).

This is the backend brief: contract, rules, migration, enforcement, tests and the typed web client. **No UI**:
P5-REFINE-12 draws Settings › Other agents.

## Linked requirements

- **Artifact (source of truth, owner's comments applied):** <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt>
  - `#f-channels-1`; rec 12 (`UXR5/layer-above.md` rec 12 "Peers moves to Settings › Other agents") and rec 3;
    blind spot "Linking agents (Peers)" (`UXR5/priorities-themes.md`); open question "Peers (rec. 12): decided, people
    create links" (`UXR5/layer-above.md`, "Open questions for the owner").
- **Local reference folder (read-only):** `/home/alex/orca/workspaces/aurora/design-ux-review-5/` (`UXR5/`).
  - **channels-1:** `UXR5/after/channels-1.png`, `UXR5/after/channels-1-lt.png`, `UXR5/after/src/channels-1.html`,
    `UXR5/before/channels-1-b1.png`, `UXR5/before/channels-1-b2.png`; screens `UXR5/screens/C035-peers.png`,
    `UXR5/screens/C039-rex-peers.png`, `UXR5/screens/C040-agent-actions-menu.png`.
    The mockup's dialog: "Link another agent" → **Agent** (a picker, "Rex"), **Who may hand tasks to whom?** with three
    options: **Both ways** ("Daneel and Rex can each give the other tasks."), **Daneel can hand tasks to Rex** ("Rex only
    works on what Daneel sends."), **Rex can hand tasks to Daneel** ("Daneel only works on what Rex sends."), buttons
    **Cancel** and **Link agents**.
- **Decisions:** D-411 (this brief), D-405 (precedence), D-407 (roles not names). Existing: FIX-110 / RFE-012 (consenting
  with `allow` is an approver's decision, `docs/07 §Agent links`), D-55 (`can_approve`).
- **Contract:** **C-1085**, drafted in
  `docs/API_CONTRACTS.md` › “Phase 5 — P5-REFINE” (merged with the briefs, 2026-11-20).
  It amends C-29 and `07 §Agent links`. It is already in `docs/API_CONTRACTS.md`; correct it there first if the code needs a different shape.
- **Security:** **SEC-142** (row drafted in the same file; add it to `docs/17-security.md`).
- **Migration:** one migration for `agent_links.direction` and `agent_links.created_by_user_id`. The number is
  **`20261201000264`** (assigned by the coordinator, 2026-11-20).
- **Normative docs:** `07 §Agent links and peer tasks` (line ~904), `03 §agent_links`, `05` (route rows), `13 §` where the
  agent-link approval card is described (`channels/cards/build.rs:247`).

## Wave and order

- **Wave 2, lane F, after P5-REFINE-8** (it keeps lane F's `app.rs` and `permission_matrix.rs` edits serial).
- Lane F: 5 → 6 → 8 → **10**.
- Unblocks: P5-REFINE-12's item "Settings › Other agents with Link an agent" (channels-1).
- `web/openapi.json` and `web/src/lib/api/schema.d.ts` are regenerated here.

## Context

- **Agent links domain:** `services/aurora-api/src/domains/agent_links/`:
  - `routes.rs` (`router` :14: `list_links` `GET /agents/{agent}/links` :19, `set_policy` `PUT /agent-links/{id}/policy`
    :33);
  - `service.rs` (`view` :14 builds `AgentLink` from a row as seen from one agent, `list` :46, `set_policy` :56: loads the
    agent with `load_agent` (membership), `Allow` needs `require_approve`, locks the row, fences on `revision`, audits
    `agent_link.policy` critical);
  - `dto.rs` (`LinkPolicy`, `LinkSelf`, `LinkPeer` :31 with `agent_id, handle, name, workspace_id, policy`, `AgentLink` :41,
    `SetPolicyBody` :62);
  - `handler.rs` (the `message_agent` action handler: approving a side flips it to `allow` and, when no side asks, opens
    the peer task).
- **Repo:** `crates/aurora-db/src/repo/agent_links.rs` (`AgentLinkRow` :10, `ordered` :46, `get_or_create` :80 (starts
  both sides at `initial`), `list_for_agent` :115, `set_policy` :128, `find_addressable` :150).
- **Table:** `agent_links` (`migrations/20261105000004_policy.sql:178`; `agent_a_id < agent_b_id` CHECK, unique pair,
  `revision`). Triggers bump the agents' home sync on insert, delete and policy change
  (`migrations/20261107000002_home_sync_bumps.sql:63`).
- **Where an agent creates a link today and applies the policy:** `domains/peer_tasks/service.rs`: `send_message` :93
  (`find_addressable` :104, frozen check :124, `initial` `allow` for the same workspace and `ask` otherwise :129,
  `get_or_create` :136, blocked check, `asks` list, approvals created :200–236), `open_task` :245, `cancel` :689,
  `close_with_promotion` :599. Tool: `domains/tools/agents.rs` (`list_agents`, `message_agent`).
- **Approvals of a link side:** `action_approvals` (`message_agent` key), `crates/aurora-db/src/repo/action_approvals.rs`
  (`expire` :213, `pending_message_agent_exists` :242); chat card `domains/channels/cards/build.rs:247` and its action
  `channels/actions.rs:500`.
- **Authorization helpers:** `crates/aurora-db/src/authz.rs` (`workspace_access`, `can_use`, `can_approve` :430, WorkspaceAuth
  `can_own` :97, `can_approve` :102), `services/aurora-api/src/http/auth.rs` (`Caller::workspace` :83, `require_approve`
  :216, `require_own`), `domains/conversations/service.rs:40` `load_agent`. Frozen: `crates/aurora-db/src/repo/freeze.rs:402`
  `is_agent_frozen`.
- **Web today:** `web/src/components/peers/peers-tab.tsx` (`LinkRow` :16 with the policy select, `Links`, `Tasks`,
  `PeersTab` :98), `web/src/lib/api/peers.ts` (`useAgentLinks`, `usePeerTasks`, `useSetLinkPolicy`).
- **Tests that exist:** `services/aurora-api/tests/it/peer_tasks.rs` and `tests/it/peers_rig/`;
  `permission_matrix.rs`; the FIX-110 cases.

## Scope

1. **Contract first.** Land C-1085 and the SEC-142 row in the docs. Where the drafted shapes must change after you read the
   code, change the draft in the doc and say so in the completion report.
2. **Migration** (number to be assigned; `docs/03-data-model.md` § `agent_links` updated in the same change):
   - `agent_links.direction text NOT NULL DEFAULT 'both' CHECK (direction IN ('both','a_to_b','b_to_a'))` (a = the lower id,
     as the table orders the pair); existing rows stay `both`;
   - `agent_links.created_by_user_id uuid NULL REFERENCES users (id) ON DELETE SET NULL` (null = an agent created it or the
     row predates the column);
   - the gateway role has no grant on `agent_links` (check; none needed). `scripts/extract-schema.py --check`.
3. **Who may link which agents (the rules the owner asked to define; SEC-142).** Implement exactly this, and state it in
   `07 §Agent links`:
   - **Creating** needs `can_use` on **both** agents' workspaces. A person can link the agents of two workspaces only
     when they could already use both (nothing about an agent the caller cannot use is revealed: 404).
   - **Same organization only.** An agent of another organization is a 404.
   - **Hosted and not frozen only.** A BYO agent is 409 `not_hosted`; a frozen agent (either) is 423 `frozen`.
   - **Consent follows approval rights (FIX-110).** Each side of the new link starts `allow` when the creator
     `can_approve` that side's workspace, else `ask`; the creator never sets `blocked`. So a plain member linking an agent
     of a workspace they can only use leaves that side at `ask` and the first task still needs that side's approval.
   - **Direction** (`both`, `to_peer`, `from_peer`, from the asked agent's side; stored `both | a_to_b | b_to_a`).
     Widening a direction needs `can_approve` on **both** sides (it gives the other agent more reach); narrowing needs
     `can_use` on either side. Enforced where a task is **opened** (`send_message`): an opening against the direction is
     refused with "<Peer> only works on tasks that <Self> sends it."; messages inside an open task, the task report and
     cancellation are not directional.
   - **Deleting** needs `can_use` on **either** side (it only restricts).
   - The **agent path stays**: `message_agent` to an unlinked agent still creates a link as today (starting policies,
     `direction: both`, `created_by: agent`). Retiring it is an owner question, out of scope; report it as an open question.
4. **Routes (C-1085):**
   - `GET /v1/agents/{agent}/linkable`: candidates (same-organization agents of workspaces the caller can use, every
     workspace for an organization owner or admin; not the agent itself; frozen listed with `frozen: true`; BYO listed
     with `hosted: false`; `linked` flag; name, handle, workspace id and workspace name only; at most 200; sorted by
     workspace name then agent name).
   - `POST /v1/agents/{agent}/links` body `{ peer_agent_id, direction }` → 201 `AgentLink`; 200 with the existing link when
     the pair is linked already with the same direction; 409 `link_exists` (with the id) when the direction differs.
     Errors: 400, 404, 409 `link_self`, 409 `not_hosted`, 423 `frozen`. Audit `agent_link.create` (critical, in the
     transaction; metadata `agent_a_id`, `agent_b_id`, `direction`, `policy_a`, `policy_b`, `via: "person"`).
   - `PUT /v1/agent-links/{id}/direction` body `{ direction, revision }` → 200 `AgentLink`; 409 `stale_revision`. Audit
     `agent_link.direction` (critical).
   - `DELETE /v1/agent-links/{id}` → 204. One transaction: cancel every open and queued peer task of the pair with the
     platform's cancellation line (`peer_tasks::close_with_promotion`, `Close::Cancelled`), expire the pair's pending
     `message_agent` action approvals (`action_approvals::expire`; there is no `abandoned` status there), delete the row.
     Audit `agent_link.delete` (critical; `cancelled_tasks: n`).
   - `AgentLink` gains `direction` (from the asked agent's side: `both | outgoing | incoming`), `created_by`
     (`{ kind: "person" | "agent", display_name: string | null } | null`), and `LinkPeer` gains `workspace_name`. Existing
     fields and `PUT /v1/agent-links/{id}/policy` are unchanged.
5. **Enforcement in `peer_tasks::send_message`:** read `link.direction` and the sender's position; refuse an opening
   against it before any approval is created (no approval for a task that could never run). Add the refusal text to the
   tool's coded refusals. `list_agents` (the tool) is unchanged.
6. **Typed web client (no UI)** in `web/src/lib/api/peers.ts`: `useLinkableAgents(agentId, enabled)`,
   `useCreateLink(agentId)`, `useSetLinkDirection(agentId)`, `useDeleteLink(agentId)` (each invalidates
   `peerKeys.links`), the `LinkDirection` type and a helper `directionSentence(selfName, peerName, direction)` returning
   the three mockup sentences ("Daneel and Rex can each give the other tasks.", "Rex only works on what Daneel sends.",
   "Daneel only works on what Rex sends."). `pnpm gen:api` regenerates `openapi.json` and `schema.d.ts`; the existing
   Peers tab keeps compiling and behaving the same.
7. **Permission matrix, coverage and docs** (below).

## Out of scope

- All UI (Settings › Other agents, the dialog, the empty state): P5-REFINE-12 (and P5-REFINE-14 moves Peers out of the
  Automations list).
- Retiring agent-created links, or deciding whether an agent may discover other agents.
- Links across organizations, and a person linking agents of workspaces they cannot use (they ask a person who can).
- Changing the peer task budget, TTL, queue or report rules.

## Constraints / file ownership

- **Owns:** `domains/agent_links/**`, `crates/aurora-db/src/repo/agent_links.rs`, the migration, the direction check in
  `domains/peer_tasks/service.rs` (`send_message` only; nothing else in that file), `web/src/lib/api/peers.ts`,
  `web/openapi.json`, `web/src/lib/api/schema.d.ts`.
- Shared files: `app.rs` (the router is already merged at `app.rs:684`; the new routes join the existing router), append
  to `permission_matrix.rs` and `docs/tests/security-coverage.md`.
- Do not edit `web/src/components/**`.

## Acceptance criteria

1. A member of two workspaces creates a link between an agent in each with `direction: both`; the response is an
   `AgentLink` with `created_by.kind = "person"`; the audit row exists; both sides' policies follow the creator's
   `can_approve` (`allow` for a side they approve, `ask` for the other).
2. The refusals of scope item 3: a caller with `can_use` on one side only, an agent of another organization, a BYO agent
   and a frozen agent each get the coded answer and **no row is written**; `linkable` returns no data about any agent the
   caller cannot use.
3. `direction: to_peer` stops the peer from opening a task against the sender (refusal text, no approval created); the
   sender's tasks work; the reverse direction works with `from_peer`; `both` works both ways.
4. `PUT …/direction`: widening needs `can_approve` on both sides, narrowing `can_use` on either; a stale `revision` is 409.
5. `DELETE`: the pair's open and queued tasks end with the cancellation line in their origin conversations, pending
   `message_agent` approvals are `expired`, the link is gone, and a later `message_agent` from an agent creates it again
   with `created_by.kind = "agent"`.
6. Existing behaviour is unchanged: the agent-created link, `PUT …/policy` (FIX-110) and the peer task tests pass as they
   are.
7. Gates: `cargo fmt --check`; clippy `-D warnings` for aurora-api and aurora-db; targeted `cargo test` (below);
   `scripts/guards/run-all.sh`; `python3 scripts/extract-schema.py --check`; OpenAPI check and `pnpm gen:api` with no
   diff; web `pnpm typecheck`. Report the counts. GitHub CI is the full gate.

## Tests

Targeted only. Heavy commands through `/home/alex/orca/workspaces/aurora/heavy.sh <lane> <command>`.

- **API integration** (extend `services/aurora-api/tests/it/peer_tasks.rs` and its rig, or add `agent_links_people.rs`):
  - create: both sides' policies by role (an approver on both, an approver on one, a plain member on both), response
    shape, audit row, idempotent repeat (200), different direction (409 `link_exists`);
  - SEC-142 refusals (one test each, plus a no-row assertion): one side only, no side, another organization (404), BYO
    (409), frozen (423), self (409), unknown direction (400);
  - `linkable`: scope by workspace access, org owner sees all workspaces, no other agent's data in the JSON (scan the
    fields), `linked` and `frozen` and `hosted` flags;
  - direction enforcement in `send_message` for the three values, before any approval is created;
  - `PUT …/direction` widen and narrow rules, stale revision;
  - delete: cancellation lines, expired approvals, cascade, re-creation by an agent;
  - `permission_matrix.rs` rows for the four routes.
- **DB:** migration up on a copy with existing links keeps `direction = both` and `created_by_user_id` null.
- **Unit:** the direction mapping between the asked agent's side and the stored `a_to_b/b_to_a` (both orders), the
  sentence helper in the web (`peers.test.ts`).

## Tests move with the UI (owner, 2026-11-20 — binding)

The owner: "Make sure the briefs not only change the website, but also the tests that test it! I don't want a ton of
broken tests because a button was renamed and the tests look for the old label." (P5-REFINE-1 merged with old labels
in tests, and master's CI broke until the CI Watcher fixed it in `ee7bfa0d`.) So, in **every commit** of this brief:

1. **Find every test that sees what you change.** Before committing a change to visible text, a label, an
   `aria-label`, a role, a heading, a route or `?tab=` id, a page's structure, an API field, an error code or message,
   search all test trees for the **old** strings, selectors and paths, and for the components you touched:
   - `web/tests/**` (vitest + Testing Library);
   - `web/e2e/**` (Playwright: the mocked default project **and** the `production` project that `scripts/e2e/*.sh`
     run against a real stack — `first-run.sh`, `kind.sh`, `files.sh`, `ssh.sh`, `channels.sh`, `approvals-park.sh`…);
   - `scripts/e2e/*.sh` themselves (they grep pages and call routes);
   - Rust tests that assert a message, an error code or a response shape (`services/*/tests/**`, `crates/*/src/**`
     `#[cfg(test)]`, `services/aurora-api/tests/it/**`);
   - fixtures and mocks (`web/e2e/fixtures/**`, `web/tests/**/mocks*`, MSW handlers, `web/openapi.json` snapshots).
   A plain `rg -n "<old text>|<old selector>" web/tests web/e2e scripts/e2e services crates` per change is the minimum.
2. **Update them in the same commit as the change.** New expectations use the new words exactly as the mockup shows
   them. Prefer `getByRole(…, { name })` or a `data-testid` over raw text where the text is not the point of the test;
   where the words are the point (they are the fix), assert the new words. Never loosen an assertion so that both the
   old and the new label pass, never `.skip`/`test.fixme`/delete a test to get green; a test is removed only when the
   feature it covers is removed, and the completion report names it and why.
3. **Run what you touched, plus everything that references it.** Through `heavy.sh`: every vitest file you changed
   **and** every vitest file that renders a component you changed; every Playwright spec (mocked project) that visits a
   page you changed. **Exception to the targeted-tests rule for this set (owner, 2026-11-20):** a commit that renames
   or restructures shared UI (a primitive, the app shell, the rail, the agent page, a label used on several pages) runs
   the **whole** web vitest suite once (`pnpm --dir web vitest run`, through `heavy.sh`) before handing over. Production
   specs (`--project production`) can't run in a lane without a real stack: update them by reading, and list them in
   the completion report so the nightly / P5-REFINE-20 runs them.
4. **New behaviour gets new tests** (the brief's Tests section); old tests are **kept and updated**, not replaced.
5. **The completion report lists** every test file changed (and why), every test removed (and the removed feature),
   the commands run with their results, and the production specs that only the nightly will run.
6. A merge that turns master's CI red because a test still expects old UI is a send-back to this lane, not the CI
   Watcher's job.

## Docs to update

- `docs/API_CONTRACTS.md` › "Phase 5 — P5-REFINE": C-1085 (and mark C-29 as amended).
- `docs/07-policy-approvals.md` § "Agent links and peer tasks": who creates, the rules of scope item 3, the direction.
- `docs/05-control-plane-api.md`: rows for the four routes; `docs/03-data-model.md` § `agent_links`.
- `docs/17-security.md` SEC-142 row, `docs/tests/security-coverage.md`.
- `docs/PROGRESS.md` and `docs/FEATURES.md` rows for P5-REFINE-10.

## Suggested agent/model and reason

Sonnet (`rust-developer`). A security rule (who consents for whom), a migration and an enforcement change in the peer
task path. Not Haiku.

## Completion report

(Fill in when done; P5-REFINE-20 reads this.)

- **Items done** (scope items 1–7, with commit ids):
- **Migration number used:**
- **Contract text changed from the draft, and why:**
- **Deviations from this brief, and why:**
- **Starting-policy rule** as implemented (a table of creator rights → side policies):
- **Screenshots compared:** none (backend brief); attach the JSON of `linkable` and of a created link for the mockup's
  "Daneel and Rex" example:
- **Gate counts:**
- **Open questions for the owner:** (including whether agent-created links should be retired)
