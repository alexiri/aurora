# Connect Sentry

This package has four apps. Pick the one that matches where your Sentry organization lives; Aurora
adds your token to requests your agents send to Sentry, and agents never see it.

| App | Host | Use it when |
|---|---|---|
| **Sentry** | `sentry.io` | your organization has no region host |
| **Sentry (US region)** | `us.sentry.io` | the organization's data storage location is the US region |
| **Sentry (DE region)** | `de.sentry.io` | the organization's data storage location is the DE (EU) region |
| **Sentry (self-hosted)** | your own host | you run Sentry yourself |

Sentry documents that you can use your organization's region domain for lower latency. Open
*Organization settings* in Sentry to see which region your organization is in. A token only works on
the host of its organization: if requests for an organization you know exists answer `404` or `403`,
connect the app of the other region.

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

1. Choose the app (above) and enter the **token** and your **organization slug**. The slug is the
   part of your Sentry URL after `/organizations/` (or the subdomain of `<slug>.sentry.io`).
   The optional **default project slug** is recorded with the connection for your reference.
2. Aurora checks the token by listing the organization's projects on the chosen host
   (`GET /api/0/organizations/<slug>/projects/`); a wrong slug, a wrong region or a token without
   `project:read` fails the connection.

## Self-hosted Sentry

1. An administrator registers the host once: open *Instances* (organization owners and admins for
   one organization, instance admins for all), choose Sentry (self-hosted) and enter the host name,
   for example `sentry.example.com` (no `https://`, port or path). Aurora checks `GET /api/0/` first.
   Registering decides where credentials may be sent, so only register hosts you trust.
2. Create an internal integration in your Sentry as above and connect with the token.

A self-hosted Sentry that is served under a path prefix is not supported (an instance is a bare host).
If the host has a private address, the operator must allow it with `gateway.allowed_private` (and the
gateway's network policy).

## What agents can do

Reading is allowed by default: organizations, projects, issues (with Sentry's search query), an
issue's events, and releases. On `sentry.io` and self-hosted Sentry agents can also list the
organizations the token can see; Sentry documents that endpoint on `sentry.io` only, so the region
apps do not offer it.

Changing an issue (resolve, ignore, assign, bookmark: `PUT` on the issue) is a separate permission
and is held for approval. The approval shows the organization, the issue, the new status and the
assignee.

## Limits

- Not included: commenting on issues (Sentry's API reference does not document an endpoint for it),
  deleting issues, bulk updates, releases and deploys changes, alerts, monitors, dashboards, members and
  settings.
- The token is only sent to the hosts of the app you connected (`/api/0`). Organization subdomains
  such as `<slug>.sentry.io` and Sentry's ingest hosts are not covered.
