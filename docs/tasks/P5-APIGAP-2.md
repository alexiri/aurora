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
