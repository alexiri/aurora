# P5-REFINE-5: Agents, keys, identity and setup backend: the starter key for every agent, "can it answer?", names from the sign-in, honest key checks (owner, 2026-11-20)

## Goal

UX review 5 found that a newcomer's agent does not just work, and that Aurora does not say why. This brief is the **Rust side only**. It adds the
web `lib/api` client and types for every new field and no UI (P5-REFINE-11, -12, -13 and -18 build the screens).

- **A second agent has no AI key (priority 10, access-3).** Carol creates "Rex" and gets a red "Can't answer" dot, although the Company key says "New agents get this key
  automatically". Starter grants are applied only when a person's **personal workspace is first created** (`placement/service.rs` `ensure_personal_workspace`); a later agent, or an agent
  that exists when the key is added, gets nothing. The fix hides in a collapsed "Not given to Rex (1)". **D-414**: starter (organization) keys apply to **every new hosted agent**, and agents
  created before a key get it when it is added, with a visible "Given to Daneel and 1 other agent". With **no starter key**: Try it is not offered, setup step 3 stays "Add an AI key so agents
  can answer", and an agent's chat says "<Agent> has no AI model yet" with Add a key (admins) or Ask an admin (everyone else), never a Welcome that asks for a key that does not exist.
- **The admin cannot tell why an agent cannot answer, and members cannot see it at all.** The reason lives on `GET …/model-readiness`, which is **owners only**. The header (D-408) needs
  "Can't answer: no AI key" with one action for **whoever is looking**.
- **"Try it" never completes (setup-1).** The client guesses it from transcript events and misses.
- **Health says "did not answer from the API" right after a key on that endpoint answered (setup-4, D-383).**
- **A personal AI key ends in "Saved … Verification pending" in red, never updated (access-8).**
- **The instance admin is "alice" everywhere (priority 7).** The bootstrap admin binds by e-mail and never gets a name from the sign-in; a personal workspace is called "Your workspace", "Bob Builder", "Bob's workspace",
  "Bob's Lab" or `dev-bob` depending on the page.
- **New agent defaults to "Bring your own (BYO)" (channels-2)** although only owners can make one.
- **Turning on "Check where AI keys come from" fails with raw keys in the error (admin-1).**

Decisions that bind this brief: **D-405**, **D-407** (names from the sign-in's `name` claim; text that points to an administrator names the role), **D-408** (the header's "Can't answer"), **D-414** (starter keys),
**D-383** (health from real calls).

## Linked requirements

- **The review:** <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt>. Workers cannot open artifacts; read the local copy `/home/alex/orca/workspaces/aurora/design-ux-review-5/` (written `UXR5/` below).
  `UXR5/findings.md` is the authoritative text. `UXR5/priorities-themes.md` priorities 7 and 10.
- **Findings** (artifact anchor `https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt#f-<key>`; After `UXR5/after/<key>.png` and `-lt.png`; markup `UXR5/after/src/<key>.html`; Before `UXR5/before/<key>-b1.png`; screens `UXR5/screens/<id>.png`):
  - `setup-2` (the starter key reaches existing agents; "Given to Daneel and 1 other agent") · After `setup-2.png`, `setup-2-lt.png` · Before `setup-2-b1.png`, `setup-2-b2.png` · Screens `A015-add-key-filled.png`, `A021-try-it-chat.png`, `A022-welcome-access-given.png`.
  - `access-3` (a second hosted agent; "Give Rex the Company key") · After `access-3.png`, `access-3-lt.png` · Before `access-3-b1.png`, `access-3-b2.png` · Screens `C041-rex-chat-cant-answer.png`, `C044-rex-agent-access.png`, `C045-rex-access-expanded.png`, `A111-org-apps.png`.
  - `setup-1` (Try it completes on the first finished turn) · After `setup-1.png`, `setup-1-lt.png` · Before `setup-1-b1.png` · Screens `A025-chat-answer.png`, `A026-checklist-after-hello.png`, `A190-admin-overview-requests.png`.
  - `setup-4` (Health: real calls, display names) · After `setup-4.png`, `setup-4-lt.png` · Before `setup-4-b1.png` · Screens `A030-health.png`.
  - `access-8` (a personal key says Checking… then Works; the typed key stays after an error: the second half is the web's) · After `access-8.png`, `access-8-lt.png` · Before `access-8-b1.png`, `access-8-b2.png` · Screens `B140-ai-key-result.png`, `B142-ai-key-litellm-result.png`, `B143-ai-key-final.png`.
  - `firstday-3` (Welcome ticks the AI key only after it answered) · After `firstday-3.png`, `firstday-3-lt.png` · Before `firstday-3-b1.png` · Screens `B182-phone-welcome.png`.
  - `channels-2` (New agent defaults to "Runs on Aurora"; the API half is the optional `kind`) · After `channels-2.png`, `channels-2-lt.png` · Before `channels-2-b1.png`, `channels-2-b2.png` · Screens `C036-new-agent.png`, `C002-agents.png`.
  - `admin-1` (a failed save is not kept; which endpoints need a key origin, set in Helm) · After `admin-1.png`, `admin-1-lt.png` · Before `admin-1-b1.png` · Screens `A055-ai-rules.png`, `A056-ai-rules-saved.png`, `A093-admin-settings.png`.
  - Priority 7 (names by the sign-in's name; the personal workspace's name): `UXR5/priorities-themes.md` priority 7; screens `A061-admin-people.png`, `B015-switcher.png`.
  - The words of the empty-key state ("<Agent> has no AI model yet"): `setup-2` fix text and `access-3` After.
- **Layer-above:** rec 15 (the stop ladder and "name the role"), rec 10 (first-success cards, the newcomer's agent that works): `UXR5/layer-above.md`, anchor `…#above`.
- **Decisions:** D-405, D-407, D-408, D-414, D-383. **Contracts:** **C-1061..C-1066** (drafts in
  `docs/API_CONTRACTS.md` › “Phase 5 — P5-REFINE” (merged with the briefs, 2026-11-20), section P5-REFINE-5). **The six entries are already in `docs/API_CONTRACTS.md`; if the code needs a different shape, correct the entry there first**
  (heading "Phase 5 — P5-REFINE" if absent). **Migration `20261201000261`**.
- **Security:** no new SEC number. The starter grant widens agents' access: audited per agent and grant (`actor_label = placement`), the same function and checks as the sign-in path
  (`grant_*_as_system`; a refused credential is skipped and audited). The new route is member-visible: it returns the **reason** and a **fix for the caller**; key **names** only to people who can own the workspace
  (as `model-readiness`).
- **Normative docs:** `04 §New-user placement` and `§Identity`, `05` (placement rows, `agents`, `me`, setup), `08` (LLM keys, verification), `10 §The answer rule`, `docs/tests/security-coverage.md`.

## Wave and order

- Wave 1, **lane F (Rust)**: **5** → 6 → 8 → 10. Starts immediately, in parallel with 1–4.
- Disjointness (lane F): `services/aurora-api/src/domains/{placement,agents,onboarding,me,oidc,secrets,accounts,settings,scim}/**`, `domains/hosted/{routes,model}.rs` (only the new route and a shared function; brief 4 owns
  `hosted/routes.rs`' sandbox routes: **coordinate** — add your route in a new file `hosted/answer.rs` and one `routes!` line), `crates/aurora-db/src/repo/{placement,users,account}.rs`, migration `…261`,
  `crates/aurora-db/migrations/`, the web `lib/api/{placement,accounts,agents,onboarding,account}.ts`. Brief 4 (lane E) edits `hosted/routes.rs`; rebase on it rather than editing the same hunks.
- `web/openapi.json` and `web/src/lib/api/schema.d.ts` regenerated in this change. Brief 18 (Administration UI) and 11/12 (agent pages) wait for these fields.

## Context

Paths from the repository root; line numbers are master `c0fe7d10`.

### Starter grants (Scope 1)

- `services/aurora-api/src/domains/placement/service.rs`:
  - `ensure_personal_workspace` :214 — creates the personal workspace and its default agent, then loops the saved placement's `starter_grants` :268–333 (one `SAVEPOINT starter_grant` each,
    `grants::grant_secret_as_system` / `grant_connection_as_system`, audit with `starter: true`, a refused credential skipped and audited `Denied`). **This loop is the only place starter grants are applied.**
  - `put` :474 (the whole-document PUT); `view` :361 (builds `PlacementView`, resolves `starter_grants` to names :438–470); `starter_ids` :671;
    `backfill_starter_grants` :703 (**personal default agents only**, `repo::personal_default_agents` `crates/aurora-db/src/repo/placement.rs` :240); `personal_without_starter` :685 / `count_personal_without_starter` :256.
  - DTO: `placement/dto.rs` `PlacementView` :196 (`starter_grants: Vec<StarterGrantView>`), `BackfillStarterGrants`, `BackfillResult`.
- `services/aurora-api/src/domains/grants/service.rs`: `grant_secret_as_system` :112, `grant_connection_as_system`; both take the transaction, the agent and the credential id and return the audit event.
- `services/aurora-api/src/domains/agents/service.rs`: `create` :276 (inserts the agent in a transaction; a BYO agent mints its token); route `agents/routes.rs` `create_agent` :140; `CreateAgentRequest` (`dto.rs` :129, `kind: String` **required**).
  The default agent of a new workspace/organization: `tenancy/service.rs` `create_workspace_named_in_tx` :197 and `create_organization` :100.
- `services/aurora-api/src/domains/accounts/service.rs` `starter_keys` :270 (C-1049): the `starter` flag only when the placement is `shared_org`, personal workspaces are on and the organization is the default organization.
- Web: `web/src/lib/api/placement.ts` (`useBackfillStarterGrants` :40), `web/src/components/admin/setup-view.tsx` (`putWithStarters` :108, `BackfillOffer` :122, the key-added toast :505).

### "Can it answer?" (Scope 2)

- `services/aurora-api/src/domains/hosted/routes.rs` `model_readiness` :586 (`can_own` only) → `hosted/model.rs` `readiness` :1093 (the dispatcher's own function: per granted key `reason`, the default model, roles).
  `KeyReadiness` and `ModelReadiness` in `hosted/dto.rs`.
- Web: `web/src/components/agents/agent-state.tsx` — `AgentState` reads `useModelReadiness(agentId, canOwn && hosted)` (so a member never sees "Can't answer") and computes `cantAnswer = !keys.some(usable)`.
- `services/aurora-api/src/http/who_can.rs` `WhoCan` (:42, C-981): the roles that can fix something, no names.

### Try it and Health (Scope 3)

- `services/aurora-api/src/domains/onboarding/service.rs` `setup` :151 builds `SetupStatus` (`onboarding/dto.rs` :44; its comment says "Try it is the client's"); the LLM endpoint check :259–282
  (`reachable` :144 does an unauthenticated `GET base_url`; "did not answer from the API" :278). Web `web/src/components/admin/setup-view.tsx` `useTryIt` :91 uses `useAgentReplied`
  (`web/src/lib/onboarding/use-guide.ts` :30), which reads the three newest conversations for a `text` event: it misses an answer in another agent or another conversation.
- `secrets.last_ok_at` / `last_error_at` (C-1033, migration `…254`), written by the gateway (`services/aurora-gateway/src/account_health.rs`); `accounts/health.rs` `health` :60.
- Turns: `turns.status = 'done'`, `speaker_user_id`, `role = 'turn'` (`crates/aurora-db/migrations/20261105000006_conversations.sql`).

### Names (Scope 4)

- `services/aurora-api/src/domains/oidc/service.rs`: `Verified` :299 (`name: Option<String>` from the ID token's `name`, :417), `display_name(v)` :442 (cleaning), `callback` :469; the existing-identity path
  `identities::find_user` :641; the e-mail bind of a bootstrap/invited/SCIM user :644–690 (**this path never sets a name**); `create_user` :785 (`display_name(v)` at :832; `"{short}'s org"` :879).
  Bootstrap admin and SCIM-provisioned accounts "bind by e-mail" (comment :655) and keep whatever name they were created with ("alice").
- `services/aurora-api/src/domains/me/service.rs` `patch_me` :91 (`display_name` 1–80 characters) → `crates/aurora-db/src/repo/account.rs` `update_display_name` :63; `me/dto.rs` `Me` :55, `PatchMeRequest` :87.
- `services/aurora-api/src/domains/scim/service.rs` (stored names, `display_name` ≤ 80 for `users.display_name` :45).
- `services/aurora-api/src/domains/placement/template.rs` `person_name` :242 (given name, else name, else e-mail local part; C-923) used at `placement/service.rs` :240.
- `crates/aurora-db/migrations/20261105000001_identity.sql` `users.display_name` (≤ 80).

### Keys (Scope 5)

- `services/aurora-api/src/domains/secrets/llm.rs` `check_key` :84 (a key whose endpoint did not answer and with no origin rule returns `OriginStatus::Pending` :121); `secrets/service.rs` `create` :359 and `verify` :822 (re-checks, stores the origin, notifies the gateway);
  `crates/aurora-db/src/repo/secrets.rs` `set_origin` :207. Web: `web/src/components/credentials/origin-view.tsx` ("Verification pending").
- `accounts/test.rs` `test_secret` :113 calls `secrets::verify`; `accounts/dto.rs` `Account` :160.

### Agent default and the governance save (Scope 6)

- `CreateAgentRequest.kind` required (`agents/dto.rs` :129). Web `web/src/components/agents/create-agent-dialog.tsx` (:62 picks the kind).
- `settings/service.rs` `apply` :1120 → `apply_inner` (one transaction: plans, `h.evaluate`, `problems_error` :110 → `invalid(...)` with a field per problem). The validator's message for the missing
  origin is in `crates/aurora-config/src/validate.rs` :2759 (`required because governance.llm.require_key_origin = true`, key `llm.endpoints/<id>/key_origin`). The transaction is atomic, so a refused save
  writes nothing today; the web showing "Dashboard" and counting a change is a client state bug (P5-REFINE-18) — but the API must keep proving it.

## Scope

1. **The starter key reaches every hosted agent (C-1061; setup-2, access-3).** Anchors `…#f-setup-2`, `…#f-access-3`.
   - Factor the starter loop of `ensure_personal_workspace` (:268–333) into one function `apply_starters(tx, state, agent, starters, via)` in `placement/service.rs` (savepoint per grant, skip-and-audit on refusal, audit
     `starter: true, via`). The sign-in path calls it with `via: "signin"`.
   - **New hosted agent:** call it from `agents::service::create` (after the insert, inside the transaction, hosted agents only) and from the default-agent creation of `create_workspace_named_in_tx`, when the saved
     placement is `shared_org`, personal workspaces are on and the agent's organization is the default organization (`starter_ids(&row)` is non-empty). `via: "new_agent"`.
   - **Existing agents:** `put` (:474) computes the starter ids **added** by this save (new set minus the previously saved set) and applies them to every hosted agent of the default organization (a new repository
     query `hosted_agents_of_org(org, limit)`; at most 2,000 per save; the rest is `starter_pending`), one transaction and one audit row per agent and grant, `via: "placement_save"`, `backfill: true`; skip agents that already have it.
     `PlacementView` gains `starter_applied` and `starter_pending`.
   - `backfill_starter_grants` :703 iterates **every hosted agent** of the default organization (not `personal_default_agents`); `SetupStatus` gains `agents_without_starter` (count of hosted agents of the default organization
     lacking at least one starter credential; keep `personal_workspaces_without_starter`).
   - Web client: `lib/api/placement.ts` types for `starter_applied`, `starter_pending`; `lib/api/agents.ts` `kind` optional.
2. **"Can this agent answer, and what fixes it" (C-1062; access-3, setup-2, priority 10).** Anchors `…#f-access-3`, `…#f-setup-2`.
   - New route `GET /v1/agents/{agent}/answer-readiness` in a new file `domains/hosted/answer.rs` (one `routes!` line in `hosted/routes.rs`; operation `getAgentAnswerReadiness`). Reuse `model::readiness` (do not duplicate the key logic). Caller rule: `can_use`
     (`load_agent`). Decide `reason`: `no_ai_key` when no key is granted; `key_not_usable` when keys are granted and none is usable; `no_model` when a usable key serves no allowed model (D-372, `default_not_served`/`no_default_model`).
     `fix` for the caller: `give_key` when an organization/workspace key the caller may give (usable, not granted to this agent, the starter key first) exists and the caller `can_own` the workspace (use the same grant authority
     check the grant route uses; **never** return a key the caller could not grant); `add_key` when no key exists and the caller is an org owner/admin or an instance admin; otherwise `ask_admin` with the `WhoCan` role projection of
     `http/who_can.rs` (roles, never names; D-407).
   - BYO agents: `can_answer: true`. Register the route in `tests/it/permission_matrix.rs` (a member of the workspace gets 200, a non-member 404).
   - Web client: `lib/api/credential-grants.ts` (or a new `lib/api/answer-readiness.ts`) `useAnswerReadiness(agentId)`; **do not change `agent-state.tsx`** (P5-REFINE-11 does, from this field).
3. **Try it completes; Health from real calls (C-1063; setup-1, setup-4).** Anchors `…#f-setup-1`, `…#f-setup-4`.
   - `SetupStatus.try_it`: one query for the caller (`done`) and the personal/default agent; `available` from `answer-readiness` of that agent. Keep the old client behaviour working (the field is additive).
   - The `endpoint:<id>` checks (:259–282) gain `evidence`, `last_ok_at`, `last_error_at` from the endpoint's secrets (one query grouped by `llm_endpoint_id`, newest `last_ok_at`/`last_error_at` over `status = 'active'` keys); precedence and wording as C-1063;
     the endpoint name is the **display name** (never the provider id, never "(litellm)"); the message says "answers" / "did not answer", not "from the API".
   - Dev display names: `deploy/kind/values-llm-mock.yaml` :18 and :34 carry "LiteLLM (VidaiMock, dev only)" and "AIGW shape (no default model, dev only)" as **display names**. These are config, not code; do not rename them here, but report them in the
     completion report as the source of the review's "dev labels" so the Orchestrator can decide (they are right for a dev cluster).
4. **People are named by their sign-in (C-1064; priority 7).** Anchor: priority 7 in `UXR5/priorities-themes.md`.
   - Migration `20261201000261_display_name_edited.sql`: `ALTER TABLE users ADD COLUMN display_name_edited_at timestamptz;` then the backfill of C-1064 (`display_name_edited_at = created_at WHERE display_name <> split_part(email::text, '@', 1)`; leave NULL otherwise;
     case-insensitive compare). Update `docs/03-data-model.md` (live section) and run `python3 scripts/extract-schema.py --check`.
   - `oidc::service::callback`: after the user is resolved (new, found by identity, or bound by e-mail) and **before** the transaction commits, `if user.display_name_edited_at.is_none() && let Some(n) = clean(v.name) && n != user.display_name` →
     `users::set_display_name_from_signin`. A missing `name` never writes. `create_user` keeps its e-mail-local-part fallback for a **new** person with no `name`.
   - `patch_me` sets `display_name_edited_at = now()` only when the new name differs from the current; the SCIM write of `displayName` sets it (so a directory's name is never overwritten by the sign-in). `Me` gains `display_name_source`.
   - Personal workspace name: new `template::personal_workspace_name(&dyn Lookup) -> Option<String>` returning `"<First>'s workspace"` (given name, else first word of `name`, else e-mail local part, cleaned; whole name ≤ the workspace-name cap); use it at
     `placement/service.rs` :240 instead of `person_name` for the **name** (the slug is unchanged). Keep `person_name` for the org name if still used. Update the C-923 text in `docs/04` and `API_CONTRACTS`.
   - Unit tests: claims with `given_name`, only `name`, only e-mail, bidi characters stripped, 80-character cap.
5. **An AI key says whether it works (C-1065; access-8, firstday-3).** Anchors `…#f-access-8`, `…#f-firstday-3`.
   - `Account.verification` for `ai_key` rows in `accounts/service.rs` (where the AI-key row is built, near `model_line`), derived as C-1065 from the stored origin, `health`, the last Test and "a re-check is queued" (an in-memory set of secret ids on the replica
     that is retrying; `checking` while present). The state is **advisory**: Test is always authoritative.
   - Background re-check: in `secrets::service::create` and the rotate path, when the stored origin is `Pending`, `tokio::spawn` a task that calls `verify(state, row)` at +5 s, +30 s and +2 min and stops at the first pass or when the row is gone/disabled; on a pass the existing
     `verify` stores `verified` and notifies the gateway. No new table. Bounded: one task per secret.
   - Confirm and test that a **rejected** key at save (`KeyFail::InvalidKey` → `422 llm_key_invalid`) stores nothing (no secret row, no sealed value left: `discard`).
   - Web client: `lib/api/accounts.ts` type `verification`.
6. **New agents default to hosted; a refused key-origin save explains itself (C-1066; channels-2, admin-1).** Anchors `…#f-channels-2`, `…#f-admin-1`.
   - `CreateAgentRequest.kind: Option<String>`; absent → `hosted` when `state.ops().hosted.enabled`, else `byo`. Update OpenAPI and the generated type; existing callers send `kind` and are unaffected.
   - In `settings/service.rs` `problems_error` (or a function it calls), recognise the validator's problem for key `llm.endpoints/<id>/key_origin` with the `require_key_origin` message and answer **one** error `key_origin_required` with
     `details { endpoints: [{ id, display_name, source }] }` (all missing endpoints in one error) and the sentence of C-1066. `source` from the merged view's key source (`eff.keys[...].source`, `aurora_config::Source`: Helm/deployment → `helm`).
   - Add the integration test that is the whole of "a failed save is not kept": `PUT /v1/admin/settings/governance` refused → `GET` unchanged, `generation` unchanged, no `instance_settings` row written, no audit event for it.
   - Web client: `lib/api/settings.ts` knows the new code (`ApiError.code === "key_origin_required"`, `details.endpoints`); `lib/api/errors.ts` gets the sentence.

## Out of scope

- All UI: the header's "Can't answer" and its button, the Access tab, the New agent form, Try it, the setup checklist, the key dialog, Welcome, the failed-save banner (P5-REFINE-11, -12, -13, -18).
- Org-level "Ask an admin" requests (P5-REFINE-8). The `ask_admin` fix here only names the roles (`WhoCan`).
- Changing placement into per-organization starters (the starter notion stays the instance's saved placement, default organization).
- Renaming existing personal workspaces (nothing existing is renamed; people rename their own).
- The dev display names (reported, not changed).

## Constraints / file ownership

- Owns the files listed under "Wave and order". The migration is `20261201000261` only.
- Every starter grant uses `grant_*_as_system` (no second grant path); never log or return key material; the new route returns key names only to `can_own`.
- Never touch `localhost:4000`, the aurora-dev cluster or its database. Throwaway Postgres for tests. Heavy commands only through `/home/alex/orca/workspaces/aurora/heavy.sh <lane> <command>`.
- Fast targeted tests only; GitHub CI is the full gate.

## Acceptance criteria

1. **Starter key, new agent.** With a saved `shared_org` placement whose starter is the Company key: a second hosted agent created by a member of the default organization has the key granted (`GET …/model-readiness` usable) and one audit row
   `grant.create` (`starter: true`, `via: "new_agent"`); an agent in another organization and a BYO agent get none; a refused credential is skipped and audited, and the creation still succeeds.
2. **Starter key, existing agents.** `PUT /v1/admin/placement` adding the key grants it to every hosted agent of the default organization (three agents in two workspaces in the test), answers `starter_applied { agents_granted: 3, agent_names: ≤ 3 }`, audits per agent,
   and a second identical PUT applies nothing; removing a key applies nothing; the backfill route reaches agents the personal-workspace version missed; `agents_without_starter` counts them.
3. **Can it answer.** Member, owner and org admin each get the right `fix` for: a usable key not granted (starter) → `give_key` for the owner, `ask_admin` for the member; no key at all → `add_key` for the org admin, `ask_admin` (roles only) for the member; a BYO agent → `can_answer: true`.
   No key name reaches a member.
4. **Try it.** After one finished turn by the admin in their personal agent, `try_it.done` is true on the next `GET /v1/admin/setup`; a turn in another conversation or another agent the admin owns also counts; before any key, `available` is false.
5. **Health.** An endpoint whose key had a successful real call shows `evidence: "call"`, status ok and the display name, even when the unauthenticated probe fails; a failed real call after the last success shows warning; no message contains "from the API", a provider id or "(litellm)".
6. **Names.** (a) A bootstrap admin created with display name "alice" and a token `name: "Alice Admin"` becomes "Alice Admin" at the next sign-in; (b) a person who edited their name keeps it across sign-ins; (c) a SCIM-set name is kept; (d) the migration backfill sets `display_name_edited_at` for "Bob Builder" and leaves it NULL for "alice" (`alice@…`); (e) a new person with given name "Dana" gets the personal workspace "Dana's workspace" (slug unchanged); (f) no `name` on a new person falls back to the e-mail's local part.
7. **Key verification.** A key saved while its endpoint is down shows `checking` then, once the endpoint answers, `works` within 2 minutes (test with injected clock and a mock endpoint); a rejected key at save is `422 llm_key_invalid` with **no** secret row left; Test on a `not_checked` key resolves the state.
8. **New agent default and the governance save.** `POST /v1/workspaces/{ws}/agents` without `kind` creates a hosted agent (and a BYO agent when hosted is disabled); the refused governance save returns `key_origin_required` with the endpoint list and `source`, and `GET` shows the old value, same generation, no audit row.
9. **Contracts, schema, docs.** C-1061..C-1066 in `docs/API_CONTRACTS.md` **before** the code commits; `docs/03` (users column), `docs/04`, `docs/05` rows, `docs/tests/security-coverage.md` row for the new route; `scripts/extract-schema.py --check` passes; `web/openapi.json` and `schema.d.ts` regenerated; web types added.
10. **Gates (targeted):** `cargo fmt --check`; `cargo clippy -p aurora-api -p aurora-db -p aurora-config --all-targets -- -D warnings`; `cargo test -p aurora-api --test it -- placement onboarding oidc me secrets accounts settings agents scim permission_matrix` (names as they exist);
    `cargo test -p aurora-db`; `scripts/guards/run-all.sh`; `python3 scripts/extract-schema.py --check`; OpenAPI check and `pnpm gen:api` with no diff; web `tsc`. Report counts.

## Tests

- **Placement (`tests/it/placement.rs`, `onboarding.rs`):** the starter application for a new agent, for the save, for the backfill (criteria 1–2).
- **New `tests/it/answer_readiness.rs`:** the matrix of criterion 3 + the permission matrix row.
- **`onboarding.rs`:** `try_it` and the endpoint evidence (criteria 4–5), with `account_health` rows written directly.
- **OIDC (`tests/it/oidc*.rs`) and `me`:** name refresh, edited names, SCIM, the migration backfill on a seeded database, the personal workspace name (criterion 6).
- **Secrets/accounts:** pending origin → background re-check → `works`; rejected key leaves nothing (criterion 7) — use `tokio::time::pause` and a mock endpoint (`wiremock`) as the existing key tests do.
- **Settings (`tests/it/settings_governance.rs`):** the refused save (criterion 8).
- **Unit:** `template::personal_workspace_name`, the `fix` chooser, the `reason` decision.

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
   the **whole** web vitest suite once (`cd web && /home/alex/orca/workspaces/aurora/heavy.sh <lane> pnpm exec vitest run` (not `pnpm --dir web …`, which fails here)) before handing over. Production
   specs (`--project production`) can't run in a lane without a real stack: update them by reading, and list them in
   the completion report so the nightly / P5-REFINE-20 runs them.
4. **New behaviour gets new tests** (the brief's Tests section); old tests are **kept and updated**, not replaced.
5. **The completion report lists** every test file changed (and why), every test removed (and the removed feature),
   the commands run with their results, and the production specs that only the nightly will run.
6. A merge that turns master's CI red because a test still expects old UI is a send-back to this lane, not the CI
   Watcher's job.

## Docs to update

- `docs/API_CONTRACTS.md` (C-1061..C-1066, first), `docs/03-data-model.md` (live: `users.display_name_edited_at`), `docs/04-*` (§New-user placement: starters reach every new hosted agent and are applied when added; §Identity: the name comes from the sign-in until edited; C-923 name),
  `docs/05-control-plane-api.md` (`GET /v1/agents/{agent}/answer-readiness`, placement answer, `Me.display_name_source`, setup `try_it`), `docs/08-*` (key verification state), `docs/tests/security-coverage.md`.
- `docs/PROGRESS.md` and `docs/FEATURES.md` rows for P5-REFINE-5; `docs/90-decisions.md` is the Orchestrator's (D-414 is already numbered).

## Suggested agent/model and reason

Sonnet (`rust-developer`). Six independent slices across five domains and one migration; the starter application and the sign-in name refresh touch security-relevant paths (grants, identity). Not Haiku.

## Completion report

(Fill in; P5-REFINE-20 reads it.)

- **Items done** (Scope 1–6, each with its commit):
- **Contracts C-1061..C-1066** (the commit that added them to `docs/API_CONTRACTS.md`) and **migration `20261201000261`**:
- **Starter key:** the three paths (new agent, save, backfill) and their tests; the 2,000-agent bound and what `starter_pending` showed in the test:
- **answer-readiness:** the role matrix result; what `give_key` checks for authority:
- **Try it / Health:** the queries; an example payload with `evidence`:
- **Names:** the sign-in paths covered (new, identity, e-mail bind), the backfill result on the seeded rows, the SCIM rule:
- **Key verification:** the retry schedule as implemented; what happens if the replica dies; the rejected-key proof:
- **Agent default / governance save:** the error body of the refused save; the proof nothing was written:
- **The dev display names** ("LiteLLM (VidaiMock, dev only)", "AIGW shape (no default model, dev only)"): reported for the Orchestrator's decision:
- **Deviations and why:**
- **Screenshots compared:** none (no UI); the UI briefs compare `setup-2`, `access-3`, `access-8`, `setup-4`.
- **Tests run** (commands, counts, anything not run and why):
- **Open questions:**
