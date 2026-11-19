# P5-APIGAP-2: Ask an admin for a chat platform or an AI key, not only an app

## Goal
Owner, 2026-11-19 (D-399). D-386 (P5-POLISH-4, SEC-135) lets a person ask an organization owner/admin to set up an
**app** (`POST /v1/workspaces/{ws}/apps/{app}/setup-request`, joined requests, the **Needs you** card). The redesign
also shows **Ask** where a member hits something only an admin can provide, and those have no API (P5-POLISH-11
pass 2): an agent's **Channels** tab (no chat platform connected for the workspace), **Organization › AI models**
for a member (no AI key / no model they can use), and the organization's **chat platforms** page for a member.

## Change
1. **Contract first (C-1047):** generalise the setup request to a subject: `app` (today's, unchanged behaviour and
   route kept), `chat_platform` (optional connector type, e.g. "mattermost"), `ai_key` (the organization's AI key or
   access to a model). One request per (organization, subject); a second person's ask joins it, as D-386. Decide
   and document who receives each kind: `app` and `ai_key` → the organization's owners/admins; `chat_platform` →
   whoever can connect it (instance admins for a managed connector; say which). Write the route shape in `05` and
   `API_CONTRACTS` before code; prefer one new route `POST /v1/workspaces/{ws}/requests` with `{subject}` beside the
   existing app route over a second special case, and list requests the way the Needs-you card already does.
2. **Server:** storage (migration `20261201000257` if the request table needs a subject column/kind; keep existing
   rows valid), joining, notification and resolution as D-386 does for apps (resolved when the thing exists: a chat
   platform connected for the organization / an AI key the person's agents can use; or dismissed by an admin).
   Same abuse limits and audit as SEC-135's app requests; never reveal to the requester anything about other
   organizations or who else asked beyond D-386's "Bob and Carol want…".
3. **Web:** **Ask <Name>** on the three places above, reusing the app journey's ask control and wording ("Asked
   <name> · Undo"); the Needs-you card renders the new kinds with the action that fixes them (Connect a chat
   platform, Add an AI key). Replace the placeholders P5-POLISH-6/-9 left. Regenerate the API types.
4. Docs: `05`, `API_CONTRACTS` C-1047, `14`, `03` (table change, without `created_at`/`updated_at`).

## Tests
API ITs per kind: ask, a second ask joins, the right admins see it, resolved when the thing exists, dismissed,
limits; the app route unchanged (its existing ITs stay green). Vitest for the three Ask buttons and the card rows.
Targeted only (CI is the gate); `extract-schema --check`, migration guards.

## Ownership
The setup-request domain in `services/aurora-api`, the migration, the web ask control, Channels tab, org AI models
and chat platforms member views, Needs-you card. Size M. D-399, C-1047, migration `000257`.

## Completion report (2026-11-19, lane `p5-apps`)
**What changed.** C-1047 written first (API_CONTRACTS, 05, 14, 03). Migration `20261201000257` adds `app_setup_requests.subject` (`app` default, `chat_platform`, `ai_key`) and makes the open-request unique index `(org_id, subject, app_id)`; existing rows stay valid. New routes `POST/GET /v1/workspaces/{ws}/requests` (any active workspace member; `subject: app` delegates to the app ask) and `DELETE /v1/workspaces/{ws}/requests/{id}`; same `setup_request` limit class (20/h, shared with the app ask). `AppSetupRequest` gains `subject` and `connector_type`. Recipients: `app`/`ai_key` and `chat_platform` → org owners/admins; `chat_platform` also the instance admins (notice only, link to Administration › Chat platforms) when no connector of the asked type is active. Resolution: active channel integration (of the type when named) / active LLM key of the org or the asker's workspace; re-evaluated on integration create, LLM key create and the periodic sweep. Web: `AskAdmin` control ("Ask <Name>" / "Asked <Name> · Undo") on the agent Channels empty state, the member AI models view (no key or no permitted model) and a new member view of Organization › Chat platforms (members were refused before); Needs-you rows render the new kinds with **Connect a chat platform** / **Add an AI key**. API types regenerated (openapi.json, schema.d.ts, spec_schema.json).
**Files.** docs: API_CONTRACTS, 05, 14, 03, this brief. Rust: `crates/aurora-db/migrations/20261201000257_app_setup_request_subject.sql`, `crates/aurora-db/src/repo/app_setup_requests.rs`, `services/aurora-api/src/domains/app_setup_requests/{dto,routes,service}.rs`, `domains/channels/integrations.rs`, `domains/secrets/service.rs`, `http/apilimit.rs`, tests `it/app_setup_requests.rs`, `it/permission_matrix.rs`; fixtures `web/openapi.json`, `crates/aurora-db/tests/fixtures/spec_schema.json`. Web: `src/components/ask-admin.tsx` (new), `states.tsx`, `channels/agent-channels-tab.tsx`, `governance/org-governance.tsx`, `org/org-topics.tsx`, `overview/setup-request-row.tsx`, `lib/api/app-setup-requests.ts`, `lib/who-can.ts`, `lib/api/schema.d.ts`, `tests/ask-admin.test.tsx` (new).
**Tests run (all through heavy.sh).** `cargo test -p aurora-api --test it -- app_setup::requests` 14 passed (7 new: chat platform ask/join/recipients, resolve + 409, ai_key, withdraw/decline, validation and app-subject parity, limit; the 7 existing app-route ITs stay green); `permission_matrix` (routes_by_principal, coverage) and `openapi_matches_checked_in` pass after regenerating; apilimit class unit asserts added. `cargo fmt --check` ok; `cargo clippy -p aurora-api -p aurora-db --all-targets -D warnings` ok; `extract-schema.py --check` ok (after regenerating); `scripts/guards/run-all.sh` no failures; `pnpm typecheck` ok; eslint on touched web files ok; vitest `ask-admin`, `setup-request-row`, `polish-2-primitives`, `who-can`, `governance` pass (8 new tests: three Ask places, asked/undo/409, three Needs-you rows). No Playwright run (app route unchanged).
**Not done / notes.** Instance admins are told by notice but cannot list/decline chat-platform requests (the list/decline routes stay the organization's); the member chat-platforms page cannot tell whether a platform is already connected (it relies on the 409 `already_available`) and has no sidebar entry change. No CI-visible perf or Playwright work. Process slip: two small appends (dto.rs, routes.rs) were made with a shell heredoc instead of Edit.
