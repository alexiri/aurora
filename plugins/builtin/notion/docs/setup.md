# Connect Notion

Aurora adds the credentials and the required `Notion-Version` header to requests your agents send to
`api.notion.com`; agents never see the token. This package pins Notion API version **2026-03-11**
(the latest version in Notion's versioning reference when the package was written); an agent cannot
send a different version.

## Register the Notion connection (admins)

1. In Notion open the Developer portal (*Settings, Connections, Develop or manage integrations*) and
   create a **Public** connection (Notion's "public integration" with OAuth).
2. Add `<your Aurora URL>/v1/oauth/callback` as the redirect URI.
3. Under *Capabilities* enable only what you will offer: for reading, the content read, comment read
   and user information capabilities; for writing, content insert and update and comment insert.
   Notion has **no OAuth scopes**: the connection's capabilities decide what it may do, and Aurora
   still holds every changing request for approval.
4. Copy the OAuth client ID and client secret into Aurora under *Admin, Apps, Notion*.

Aurora uses the authorization code flow and authenticates at the token endpoint with HTTP Basic
(client ID and secret), as Notion documents. Notion returns an access token and a refresh token;
Notion does not state a lifetime for the access token, so Aurora refreshes after a `401` and stores
the new refresh token Notion issues with every refresh.

## Connect

1. Choose *Notion* and sign in. Notion has **no OAuth scopes**: access is granted by choosing pages
   in Notion's consent screen. Notion asks which pages to share with the connection; sharing a parent
   page also shares its children. Pages that are not shared answer `404 object_not_found`.
2. Aurora names the connection after the bot user (`GET /v1/users/me`) and records the workspace id.

## What agents can do

Reading is allowed by default: the bot user, users, pages and their properties, databases and data
sources, blocks and child blocks, comments.

Everything else is a separate permission and is held for approval:

- **Search** (`POST /v1/search`) and **data source queries** (`POST /v1/data_sources/<id>/query`).
  These only read, but they are `POST` requests and Aurora's read permission is limited to `GET`, so
  they are offered as approved tools.
- Creating pages, updating or trashing pages, appending blocks, and creating comments. The approval
  shows the parent or page and, for JSON requests, the title, trash flag or comment text.

## Databases and data sources

Since API version 2025-09-03 a database holds one or more *data sources*. `GET /v1/databases/<id>`
returns the list; rows are queried through `/v1/data_sources/<id>/query` and pages are created with
`parent.data_source_id`.

## Limits

- Not included: updating or deleting blocks, creating databases or data sources, views, file
  uploads, webhooks, and Notion MCP.
- Internal connection tokens and personal access tokens are not offered here; this app connects
  through OAuth only.
- The token is only sent to `api.notion.com` under `/v1`.
