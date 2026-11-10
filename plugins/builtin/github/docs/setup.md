# Connect GitHub

This package has two apps. Aurora adds the credentials to requests your agents send to GitHub;
agents never see the token.

- **GitHub** connects **github.com** (REST API at `api.github.com`, GraphQL, and git over HTTPS).
- **GitHub Enterprise Server** connects a GitHub Enterprise Server that an administrator has
  registered (REST API at `https://<host>/api/v3`, GraphQL at `https://<host>/api/graphql`, and git
  over HTTPS on the same host).

For fine-grained, repository-scoped, short-lived access prefer the **GitHub App** package
(`github-app`). OAuth apps and personal access tokens act as the user and are limited only by their
scopes.

## GitHub (github.com)

### Which connection method

- **Personal access token** (simplest). On GitHub open *Settings, Developer settings, Personal
  access tokens*. A **fine-grained** token can be limited to chosen repositories and to read-only
  permissions (Contents, Issues, Pull requests: Read); prefer it. A classic token needs the `repo`
  scope for private repositories, which also allows writing. Paste the token when connecting.
- **OAuth**. An instance admin registers a GitHub OAuth app once (below), then people connect with
  *Sign in with GitHub*. Aurora offers only `read:user` and `read:org` by default, which reaches
  public data. GitHub has **no read-only scope for repository contents**: `repo` (private and
  public) and `public_repo` are marked as write choices because they allow changing code, and
  Aurora still holds every changing request for approval.

### Register the OAuth app (admins)

1. On GitHub open *Settings, Developer settings, OAuth Apps* (your own, or the organization's) and
   choose *New OAuth App*.
2. Set the authorization callback URL to `<your Aurora URL>/v1/oauth/callback`.
3. Generate a client secret.
4. Enter the Client ID and Client secret in Aurora under *Admin, Apps, GitHub*.

Aurora uses the authorization code flow with PKCE. If your OAuth app is configured to expire user
tokens, Aurora refreshes them with the refresh token; otherwise the token stays valid until it is
revoked.

## GitHub Enterprise Server

1. An administrator registers the server once: open *Instances* (organization owners and admins
   for one organization, instance admins for all), choose GitHub Enterprise Server and enter the
   host name, for example `github.example.com` (no `https://`, port or path). Aurora checks
   `GET /api/v3/meta` first. Registering decides where the credentials may be sent, so only
   register hosts you trust.
2. For OAuth, create an OAuth app **on that server** (as above) and add it under *Credentials,
   OAuth clients*, choosing the instance. Every instance has its own application.
3. People pick the instance when they connect, with a personal access token or OAuth.

If the server has a private address, the operator must allow it with `gateway.allowed_private`
(and the gateway's network policy) before registration and requests can reach it. Deleting an
instance disables its connections and removes its OAuth clients.

## What agents can do

Reading is allowed by default: your profile and repositories, repository metadata, issues and
pull requests with their comments, changed files, repository files, commits, issue and code
search, GraphQL queries, and `git clone` / `git fetch`.

Changing things is a separate permission and is held for approval: creating and updating issues,
commenting, creating and merging pull requests, creating, changing or deleting repository files,
creating branches and tags, GraphQL mutations, and `git push`. The approval shows the repository,
the item and, for JSON requests, the title or message.

## Skill libraries

This package can serve a **skill library**: an organization owner or admin connects a repository
(Skills, Skill libraries) through an **organization-level** connection, and people add skills from it
to an agent, a workspace or the organization. Aurora reads the repository's files from the GitHub
API with `GET` requests only. Use a fine-grained token limited to the skills repository with
*Contents: read*. A push never changes what agents are told: a person reviews and approves the change.

## Limits

- GraphQL: queries are read tools; any mutation is a write tool.
- Hosts other than the ones above, such as `uploads.github.com`, `gist.github.com` or
  `ghcr.io`, are not covered.
- Webhooks are not part of this package.
