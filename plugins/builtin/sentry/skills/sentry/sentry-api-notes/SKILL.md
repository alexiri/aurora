---
name: sentry-api-notes
description: "How to make common Sentry requests, address organizations and page through results; read before the first request"
---

# Sentry API notes

`$H` is the Sentry address shown with the account (`https://sentry.io`, `https://us.sentry.io`,
`https://de.sentry.io` or a self-hosted server). The token works only on the host of its organization's
region: never send this connection's requests to another host. Aurora adds the token: send none, no `-k`.
Paths end with a slash.

## Common requests

| Task | Request |
|---|---|
| Unresolved issues of an organization | `GET $H/api/0/organizations/<org>/issues/?query=is:unresolved` |
| One issue | `GET $H/api/0/organizations/<org>/issues/<issue id>/` |
| Latest event of an issue (stack trace) | `GET $H/api/0/organizations/<org>/issues/<issue id>/events/latest/` |
| Events of an issue | `GET $H/api/0/organizations/<org>/issues/<issue id>/events/` |
| Projects, releases | `GET $H/api/0/organizations/<org>/projects/`, `.../releases/` |
| Resolve an issue | `PUT $H/api/0/organizations/<org>/issues/<issue id>/` with `{"status": "resolved"}` |

`query` uses Sentry's search syntax (`is:unresolved`, `level:error`, `assigned:me`); add `project=<id>`
and `statsPeriod=24h` to narrow. Changes are JSON bodies with `content-type: application/json` so
approvals can show what changes.

## Organizations

Listing the organizations a token can see (`GET $H/api/0/organizations/`) is only served on `sentry.io`
and on self-hosted servers. On a region host (`us.` or `de.`) the organization slug is in the
connection's metadata (shown in `app.md`); if it is not there, ask the person for it.

## Pagination

Results come in pages: the `Link` response header has `rel="next"` with `results="true"` when there is
another page; request its URL as it is (it carries the `cursor`) until `results="false"`.
