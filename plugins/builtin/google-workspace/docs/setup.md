# Connect Google Workspace

This package has three apps, **Gmail**, **Google Calendar** and **Google Drive**, that share one Google
OAuth client. An administrator registers the client once; each person then connects the apps they
need, and each connection asks Google only for the scopes of that app. Aurora stores the tokens
encrypted, refreshes them, and injects them only on the Google API hosts and paths of the app; agents
never see a token.

| App | Host and path prefix | Documentation |
|---|---|---|
| Gmail | `gmail.googleapis.com` `/gmail/v1` | https://developers.google.com/workspace/gmail/api/reference/rest |
| Google Calendar | `www.googleapis.com` `/calendar/v3` | https://developers.google.com/workspace/calendar/api/v3/reference |
| Google Drive | `www.googleapis.com` `/drive/v3` and `/upload/drive/v3` | https://developers.google.com/workspace/drive/api/reference/rest/v3 |

## 1. Create the OAuth client (administrator)

1. In the [Google Cloud console](https://console.cloud.google.com/) create or choose a project and
   enable the **Gmail API**, **Google Calendar API** and **Google Drive API** (*APIs and services,
   Library*). Enable only the ones you will offer.
2. Open *Google Auth Platform* (OAuth consent screen), choose the user type (*Internal* if everyone
   is in your Google Workspace organization, otherwise *External*) and add only the scopes listed in
   step 3.
3. Create an OAuth client of type **Web application**. Add the Aurora redirect URI
   `<external_url>/v1/oauth/callback` as an authorized redirect URI.
4. Enter the client ID and client secret in Aurora under the `google` OAuth client of this package.

Google's web server flow uses a client secret; this package does not send PKCE parameters because
that page does not document them for web applications.

## 2. Scopes: read-only by default

Aurora requests the default scope of each app unless the person (or the administrator's policy)
asks for more. Scopes that change data are marked **write**; every request that changes data is held
for approval.

| App | Scope | Access | Default | Google classification |
|---|---|---|---|---|
| Gmail | `gmail.readonly` | read | yes | restricted |
| Gmail | `gmail.send` | write | no | sensitive |
| Gmail | `gmail.compose` | write | no | restricted |
| Gmail | `gmail.modify` | write | no | restricted |
| Calendar | `calendar.readonly` | read | yes | sensitive |
| Calendar | `calendar.events.readonly` | read | no | sensitive |
| Calendar | `calendar.events` | write | no | sensitive |
| Calendar | `calendar` | write | no | sensitive |
| Drive | `drive.readonly` | read | yes | restricted |
| Drive | `drive.metadata.readonly` | read | no | restricted |
| Drive | `drive.file` | write | no | not sensitive |
| Drive | `drive` | write | no | restricted |

All scope names are `https://www.googleapis.com/auth/<name>`. The sensitive and restricted
classification is Google's and can change: confirm it for your client on the consent screen's scope
list in the Cloud console (sensitive scopes carry a sensitive indicator there) and in Google's
[scope list](https://developers.google.com/identity/protocols/oauth2/scopes).

- `drive.file` is the narrowest way to let agents write to Drive: it covers only files the app
  creates or the person opens with it. It cannot read the rest of the Drive, so pair it with
  `drive.readonly` if agents must also read existing files.
- `calendar.events.readonly` reads events but not the calendar list, so `list_calendars` needs
  `calendar.readonly`. Free/busy queries need `calendar.readonly`, `calendar` or `calendar.freebusy`.
- Connecting asks Google for an offline grant (`access_type=offline`), with `prompt=consent` so that
  Google returns a refresh token every time, and `include_granted_scopes=true`, so a person who adds
  the Calendar app later keeps what they already granted.
- The connect check calls `GET /gmail/v1/users/me/profile` (Gmail), `GET /calendar/v3/calendars/primary/events?maxResults=1`
  (Calendar) and `GET /drive/v3/about?fields=user` (Drive), so a Gmail connection needs a scope that
  allows reading the profile (`gmail.readonly` or `gmail.modify`, not `gmail.send` alone).

## 3. Google verification of your OAuth client (administrator)

Google decides what an OAuth client may do from its scopes and publishing status. Summary of
Google's documentation (the links below are authoritative; check them, the rules change):

- **Sensitive scopes** (all Calendar scopes, `gmail.send`): the client must pass Google's OAuth app
  verification before general use: brand verification, a written justification for each scope and a
  demonstration video. No third-party security assessment is required. Google states that this can
  take up to 10 days. [Sensitive scope verification](https://developers.google.com/identity/protocols/oauth2/production-readiness/sensitive-scope-verification)
- **Restricted scopes** (`gmail.readonly`, `gmail.compose`, `gmail.modify`, `drive.readonly`,
  `drive.metadata.readonly`, `drive`): everything above, plus a security assessment by a
  Google-approved assessor for an app that reaches the data through a server, renewed at least every
  12 months, and a privacy policy that meets Google's Limited Use requirements. Google states that
  this can take several weeks. [Restricted scope verification](https://developers.google.com/identity/protocols/oauth2/production-readiness/restricted-scope-verification)
- **Not yet verified**: while the client is in *Testing* status, or unverified, Google applies a cap on
  the number of users and limits the refresh token lifetime (Google's page does not give numbers; see
  [Unverified apps](https://support.google.com/cloud/answer/7454865)). People then have to
  reconnect when Google expires the refresh token. Aurora reports this as a connection that needs to be
  authorized again.
- **Exceptions** exist (for example personal use with a few known users, or an *Internal* client used only
  inside your own Google Workspace organization). Read Google's pages to see whether yours qualifies.
- A Google Workspace administrator can additionally restrict which third-party apps may access
  Gmail and Drive data (*Security, API controls*). A person may be refused by their own organization
  even when Aurora's client is verified.

## 4. Connect (each person)

Open the app in Aurora, choose *Connect*, sign in with Google and approve the consent screen. Connect
Gmail, Calendar and Drive separately: each is its own connection with its own scopes.

## What agents can do

**Gmail.** Reading is allowed by default: the profile, listing and searching messages and threads,
reading a message or thread, and listing labels. Held for approval: sending mail, creating a draft,
sending a draft, and adding or removing labels. The approval shows every recipient (To, Cc and Bcc),
the subject, a preview of the message and the number of attachments, read from the raw message.
Deleting and trashing mail is not offered as a tool; under a full grant such a request is not
refused by the app, so grant mail access as custom tools unless the agent should be able to delete.

**Calendar.** Reading is allowed by default: the calendar list, a calendar, events (list and get).
Held for approval: creating, changing and deleting events, and free/busy queries (a read that Google
exposes only as `POST /calendar/v3/freeBusy`; Aurora treats every `POST` as a change).

**Drive.** Reading is allowed by default: about (user and quota), listing and searching files, file
metadata, `alt=media` downloads, exporting Google Docs, Sheets and Slides, and listing who a file is
shared with. Held for approval: creating files and folders, uploading (`multipart`, `media` and
`resumable` uploads), changing metadata, renaming, moving and trashing, replacing content,
copying, and sharing. Permanent deletion with `DELETE` is not offered as a tool, but a
connection with a full grant can still send it and it cannot be undone; use custom tools unless the
agent should be able to delete.

### Drive approval summaries

When a Drive request is held, a package hook (`approval.summarize`, no network access) builds the
summary from the request, so the person approving sees what the request does without file content:

- uploads, updates and copies: file name, target MIME type, parent folder ids and size, read from the
  JSON body or from the JSON metadata part of a `multipart` upload (only the first part is read, never the
  media). Resumable uploads show the announced size and content type;
- downloads and exports: the file id and, for an export, the export MIME type;
- sharing: role, type and the email address or domain.

If the beginning of the body (the part Aurora passes to the hook is limited) is cut off, the summary shows
only the values that were complete and says so. If the body cannot be read at all, Aurora falls back to
the generic summary of the package manifest.

## Limits

- Only the hosts and paths above carry the credentials; other Google hosts are outside this
  connection. A resumable upload can be started (and is summarized), but the `PUT` requests that send
  its content to the session URL are not offered, so use `multipart` or `media` uploads for content.
- Gmail's `/upload/gmail/v1` upload endpoints, attachments, history, settings, `trash`/`delete`
  methods, Calendar ACL and settings, Drive `files.delete`, `emptyTrash`, comments, revisions and
  shared-drive administration are not offered.
- One connection is one Google account and one app.
