# P5-ZOOM: built-in `zoom` package — meetings, recordings and transcripts on Zoom (phase 5, D-430, SEC-145)

Read `docs/tasks/P5-CATALOG.md` first (common rules, acceptance, order). This brief owns
`plugins/builtin/zoom/**` (including `hooks/`), its `REFUSED` entry and its `ICONS.md` row.

## Goal

"What meetings do I have tomorrow?", "schedule a 30-minute Zoom with Ana on Thursday", "summarize the
recording of yesterday's standup", "who attended the review?". An agent lists and schedules meetings,
reads past meetings and participants, and reads cloud recordings and their transcripts, as the person
(personal connection) or for an account (shared connection an admin sets up).

## Hosting and connect (D-430)

SaaS only: the REST API is `https://api.zoom.us/v2` for every account (an account's vanity domain such
as `<org>.zoom.us` is only the web front end). Two connect methods:

1. **`oauth` — "My Zoom account"** (`kind: oauth2`): a **General app** with user-managed authorization,
   created in the Zoom App Marketplace by the organization that owns the Zoom account and registered in
   Aurora as an OAuth client (`{{aurora.page:oauth_clients}}`; BYOC, no Aurora-wide Zoom app).
   - `oauth_clients`: `authorize_url: https://zoom.us/oauth/authorize`, `token_url:
     https://zoom.us/oauth/token`, `client_auth: basic`, `pkce: true` (verify PKCE in Zoom's docs; drop it
     if unsupported), fields `client_id`, `client_secret`.
   - `token`: `oauth2_refresh`, **`rotate_refresh_token: true`** (Zoom returns a new refresh token on every
     refresh and the old one stops working), access tokens last one hour; `reauth_codes` from Zoom's
     documented token errors (`invalid_grant` at least).
   - **Scopes: Zoom's granular scopes** (`<product>:<read|write>:<action>`, e.g. `meeting:read:list_meetings`,
     `meeting:write:meeting`, `cloud_recording:read:list_user_recordings`, `user:read:user`). Take the
     exact names from Zoom's scope reference for each endpoint below; `default_scopes` are the read ones.
     `access_levels` `read` / `write` with `permission_hints` naming the scopes to tick in the Marketplace
     form.
   - Probe: `GET https://api.zoom.us/v2/users/me` names the account (`email`).
2. **`account` — "Whole Zoom account (admin)"** (`kind: fields`, its own `token: { kind: hook }`): a
   **Server-to-Server OAuth app** created by a Zoom account admin. Fields: `account_id` (metadata),
   `client_id` (credential), `client_secret` (credential, secret), `name` (`store: label`).
   - **`token.mint` hook** (`hooks/`, copy the shape of `plugins/builtin/aws-keys/hooks/`): `POST
     https://zoom.us/oauth/token?grant_type=account_credentials&account_id=<account_id>` with HTTP basic
     `client_id:client_secret`; returns `access_token` and the expiry from `expires_in` (one hour), capped by
     `max_lifetime_secs: 3600`. Egress: `zoom.us` only. Errors: a 400/401 from the token endpoint is
     `reauth-required` (the app was deactivated or its secret rotated), never echoing the secret.
   - Optional `connect.exchange` that mints once to validate the form (the existing hook-example shape),
     same error rule.
   - The scopes are whatever the admin gave the S2S app; `connect_note` and `permission_hints` list the
     same read/write scopes as method 1.
   - With this method `me` means the app, not a person: the guide tells the agent to address users by
     e-mail or id (`/users/{userId}/meetings`).

Both inject `bearer` on `api.zoom.us` under `/v2`.

**Recording downloads.** A recording file's `download_url` is on a `*.zoom.us` host (e.g.
`zoom.us/rec/download/…` or a regional web host) and accepts the same bearer token. Check Zoom's
current docs for the exact hosts and paths. If they are a documented, bounded set, add a host entry for
exactly those hosts and that path prefix (`/rec/download`, `/rec/archive` or whatever the docs name), plus a
read tool `download_recording_file`. If they are not bounded, leave downloads out, keep transcripts via the
API only, and report it. Never claim `*.zoom.us` as a whole.

## Tool catalogue (verify each against Zoom's REST reference, OpenAPI 3.1 per product)

**Read (GET):** `users/me` and `users/*` (profile, settings not included); `users/*/meetings` (upcoming,
scheduled, live, by `type`); `meetings/*` (details, join URL, settings); `past_meetings/*` and
`past_meetings/*/participants`; `past_meetings/*/instances`; `meetings/*/invitation`;
`users/*/recordings` (cloud recordings in a date range); `meetings/*/recordings` (one meeting's files,
including the transcript file entry); `meetings/*/meeting_summary` (AI Companion summary, where the account
has it; leave out if the docs mark it unavailable to the scopes above); `report/meetings/*/participants`
only if the `user`-level scopes allow it (it usually needs an admin scope; S2S only).

**Write (each with an approval template):**
- `create_meeting` (`POST /users/*/meetings`): title "schedule a Zoom meeting"; fields: topic, start
  time, duration, timezone, invitees (from `settings.meeting_invitees`), recurrence.
- `update_meeting` (`PATCH /meetings/*`): title "change a Zoom meeting"; fields: meeting id (path), topic,
  start time, duration.
- `delete_meeting` (`DELETE /meetings/*`): title "cancel a Zoom meeting"; fields: meeting id,
  `schedule_for_reminder` / `cancel_meeting_reminder` (whether invitees are told).

**Refused** (`REFUSED["zoom"]`): recording deletion (`DELETE /meetings/*/recordings`, `…/recordings/*`),
user creation/deletion (`POST /users`, `DELETE /users/*`), account settings changes (`PATCH
/accounts/*/settings`, `PATCH /users/*/settings`), and any app or token management endpoint the docs
list (SEC-145).

Team Chat, Phone, Webinars, Rooms and Whiteboard are **not in this package**; say in the report which
of them look worth a follow-up. Zoom's own MCP server offers semantic search over meeting content, which the
REST API lacks; that is an MCP-lane question (D-172(b)), not this brief.

## Agent text

- `agent.summary`: "Zoom meetings and cloud recordings: list and schedule meetings, read past attendees, recordings and transcripts."
- `agent.instructions`: REST at `https://api.zoom.us/v2`; Aurora adds the token; times are ISO 8601
  with a `timezone`; with an account-wide connection use the person's e-mail as `userId`; never print a
  meeting passcode back into chat unless the person asked for the invitation.
- Guide: `api-notes.md` (meetings vs past meetings and UUID double-encoding, recurrence, recordings,
  transcripts as a VTT file, pagination with `next_page_token`, rate-limit headers); recipe
  `summarize-a-recording.md` ("Summarize a recorded meeting": find the meeting, list its recording files,
  fetch the transcript file, summarize, cite timestamps).
- `operations.json`: from `aurora plugin import-openapi` on Zoom's Meetings OpenAPI document, cut down to
  the tools above.

## Fixtures

Every host entry, tool, approval and refused item; OAuth lifecycle cases (`refreshed` with a new refresh
token stored, `reauth_required`); `token.mint` success, failure (`reauth-required` on 401), egress denied
for a host other than `zoom.us`; the account id reaches the mint request only as the query parameter.

## Owner checks (not blocking)

CERN's Zoom account (`cern.zoom.us`) admins must create the General app (and, for shared connections, the
Server-to-Server app). Whether an unpublished, account-only General app is enough without Marketplace review
is Zoom's rule to confirm when the app is created; the setup guide says to choose "account-level, not
published".
