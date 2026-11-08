# Connect Linear

Aurora adds your OAuth token to GraphQL requests your agents send to `https://api.linear.app/graphql`;
agents never see it.

## Register the OAuth application (admins)

1. In Linear open *Settings, API, OAuth applications* and create an application.
2. Add `<your Aurora URL>/v1/oauth/callback` as a callback URL.
3. Copy the client ID and client secret into Aurora under *Admin, Apps, Linear*.

Aurora uses the authorization code flow with PKCE. Linear access tokens last 24 hours and every
sign-in comes with a refresh token that lasts 30 days; each refresh returns a new pair and starts a
new 30-day period, which Aurora stores. If the refresh token expires or is revoked, connect again.

## Scopes

- `read` (default): read access to the account. Linear always includes it.
- `write`: read and write everything the account can. Covers every change this app offers.
- `issues:create`: create and update issues and their attachments.
- `comments:create`: create comments.

Choose **one** scope. Linear documents the `scope` parameter as comma separated while Aurora sends
the chosen scopes separated by spaces, so a combination such as `issues:create` plus
`comments:create` is not reliable; `write` is the safe way to allow both.

## What agents can do

All requests go to the one GraphQL endpoint, so Aurora classifies each request by the operations in
its body (as it does for GitHub's GraphQL API):

- A request that contains only **queries** is a read and is allowed by default.
- A request that contains **any mutation** (create or update an issue, create a comment, and so on) is
  a write and is held for approval. The approval shows the operation name and the mutation fields.
- A body Aurora cannot parse as one operation matches no tool and is refused.

## Limits

- Personal API keys (`lin_api_…`, sent without `Bearer`) are not offered; this app connects through
  OAuth only.
- No connection probe: Linear's GraphQL endpoint has no `GET` check Aurora could call, so the
  connection is not named after the user.
- Client-credentials tokens, `actor=app` installs, webhooks and file uploads are not covered.
