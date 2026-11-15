# Connect Sentry

Sentry is one app. When you connect, you choose **where your Sentry organization lives**; Aurora
adds your token to requests your agents send to that host only, and agents never see it.

| Choice | Host | Use it when |
|---|---|---|
| **sentry.io** (default) | `sentry.io` | your organization has no region host |
| **US region** | `us.sentry.io` | the organization's data storage location is the US region |
| **EU region (DE)** | `de.sentry.io` | the organization's data storage location is the DE (EU) region |
| a **registered server** | your own host | you run Sentry yourself (an administrator registers it first) |

Sentry documents that you can use your organization's region domain for lower latency. Open
*Organization settings* in Sentry: **Data Storage Location** shows which region your organization is
in. A token only works on the host of its organization: if requests for an organization you know
exists answer `404` or `403`, connect again and choose the other region.

## Create a token

1. In Sentry open *Settings, Developer Settings* and choose *Create New Integration*, then
   *Internal Integration*. (A personal token from *User settings, Personal Tokens* works too, but
   an internal integration is not tied to a person.)
2. Under *Permissions* choose the narrowest access that you need. Reading needs `Organization: Read`,
   `Project: Read` and `Issue & Event: Read` (the scopes `org:read`, `project:read`, `event:read`).
   Resolving or updating issues needs `Issue & Event: Write` (`event:write`). Sentry's
   `project:releases` scope covers reading **and** changing releases, so only grant it when you need
   releases, and keep in mind Aurora only treats the request method, not the scope, as the permission
   boundary.
3. Save, then copy the token. Organization auth tokens (`sntrys_…`) are meant for CI (releases and
   source maps) and cannot read issues; use an internal integration or a personal token.

## Connect

1. Choose where your organization lives (above) and enter the **token** and your **organization
   slug**. The slug is the part of your Sentry URL after `/organizations/` (or the subdomain of
   `<slug>.sentry.io`). The optional **default project slug** is recorded with the connection for
   your reference.
2. Aurora checks the token by listing the organization's projects on the chosen host
   (`GET /api/0/organizations/<slug>/projects/`); a wrong slug, a wrong region or a token without
   `project:read` fails the connection.

## Self-hosted Sentry

1. An administrator registers the host once: open *Self-hosted apps* (organization owners and admins
   for one organization, instance admins for all), choose Sentry and enter the host name, for example
   `sentry.example.com` (no `https://`, port or path). Aurora checks `GET /api/0/` first.
   Registering decides where credentials may be sent, so only register hosts you trust.
2. Create an internal integration in your Sentry as above, connect, and choose the registered server.

If the host has a private address, the operator must allow it with `gateway.allowed_private` (and the
gateway's network policy). The two regions are built in: nobody registers `us.sentry.io` or
`de.sentry.io`, and they cannot be changed or deleted.

## What agents can do

Reading is allowed by default: organizations, projects, issues (with Sentry's search query), an
issue's events, and releases. Agents can also list the organizations the token can see; Sentry
documents that endpoint on `sentry.io` and self-hosted servers only, so on a region host it is not
served and the organization slug comes from the connection.

Changing an issue (resolve, ignore, assign, bookmark: `PUT` on the issue) is a separate permission
and is held for approval. The approval shows the organization, the issue, the new status and the
assignee.

## Upgrading from the four Sentry apps

Earlier versions listed Sentry, Sentry (US region), Sentry (DE region) and Sentry (self-hosted) as
four apps. Version 2.0.0 is one app. Existing connections move with it: connection ids, agent
permissions and history are unchanged; a US or DE connection now shows the matching region as its
server, and a self-hosted connection keeps its registered server.

## Limits

- Not included: commenting on issues (Sentry's API reference does not document an endpoint for it),
  deleting issues, bulk updates, releases and deploys changes, alerts, monitors, dashboards, members and
  settings.
- The token is only sent to the host of the connection (`/api/0`). Organization subdomains
  such as `<slug>.sentry.io` and Sentry's ingest hosts are not covered.
