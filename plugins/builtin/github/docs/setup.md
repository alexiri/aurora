# Connect GitHub

This package has two apps. Aurora adds the credentials to requests your agents send to GitHub;
agents never see the token.

- **GitHub** connects **github.com** (REST API at `api.github.com`, GraphQL, and git over HTTPS).
- **GitHub Enterprise Server** connects a GitHub Enterprise Server that an administrator has
  registered (REST API at `https://<host>/api/v3`, GraphQL at `https://<host>/api/graphql`, and git
  over HTTPS on the same host).

Each app asks **"How do you want to connect?"** and offers up to three ways:

- **GitHub App (recommended for teams).** An app installed on the repositories you choose.
  Permissions are fine-grained and per repository, tokens last one hour, and access does not depend
  on one person's account.
- **OAuth.** Acts as you, on everything your account can reach, with the scopes you tick.
- **Personal access token.** Acts as you; the quickest for personal use. A fine-grained token can
  be limited to chosen repositories.

(Before Aurora merged them, the GitHub App was a separate "GitHub App" app. Existing connections
were moved to **GitHub** with their grants intact; see *Upgrading* below.)

## GitHub (github.com)

### Which connection method

- **GitHub App** (recommended for teams): see *Connect a GitHub App* below.
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

## Connect a GitHub App

A GitHub App acts as itself, not as a person. It is installed on chosen accounts and repositories,
has fine-grained permissions, and uses tokens that expire after one hour. Aurora keeps the app's
private key and mints a fresh installation token whenever one is needed; agents never see the key
or the token.

1. On GitHub open *Settings, Developer settings, GitHub Apps* (your own, or the organization's)
   and choose *New GitHub App*. On Enterprise Server do this on your server.
2. Choose only the repository permissions you want to offer. For read-only use select *Read-only*
   for *Contents*, *Issues*, *Pull requests* and *Metadata*. Add *Read and write* only where agents
   should change things; Aurora still holds every changing request for approval. Webhooks can stay
   disabled.
3. Create the app, note its **App ID** (or **Client ID**), and under *Private keys* choose
   *Generate a private key*. GitHub downloads a `.pem` file once.
4. Install the app on the account or organization and select **only the repositories** agents need.
5. The **installation ID** is the number at the end of the installation's settings URL
   (`https://github.com/settings/installations/<id>`, or the organization's equivalent).
6. In Aurora choose *Connect*, *GitHub App*, enter the App ID and the installation ID and upload
   the `.pem` file. On **GitHub Enterprise Server** the administrator first registers the server
   under *Instances* (below); then choose that instance and enter the same host name as *Server
   host*. A server host that differs from the registered instance is refused.

Aurora signs a short-lived JWT with the key (RS256, issued 60 seconds in the past, valid for at most
10 minutes, issuer = the App ID), calls
`POST /app/installations/<installation id>/access_tokens` and uses the returned installation token
(valid for one hour) as the credential. It checks the values when you connect, so a wrong key or
installation id is reported immediately. The private key is stored encrypted and is never shown,
logged or included in approval summaries or error messages. To rotate it, generate a new key on
GitHub, delete the old one, and reconnect. If the app is uninstalled or the key is deleted, the
connection asks you to reconnect.

Installation tokens cannot call endpoints that act as a user, such as `GET /user`: the tools that
act as you (*Get the connected user*, *List your repositories*) appear only for OAuth and personal
token connections, and *List repositories of the installation* only for GitHub App connections.

## GitHub Enterprise Server

1. An administrator registers the server once: open *Instances* (organization owners and admins
   for one organization, instance admins for all), choose GitHub Enterprise Server and enter the
   host name, for example `github.example.com` (no `https://`, port or path). Aurora checks
   `GET /api/v3/meta` first. Registering decides where the credentials may be sent, so only
   register hosts you trust.
2. For OAuth, create an OAuth app **on that server** (as above) and add it under *Credentials,
   OAuth clients*, choosing the instance. Every instance has its own application.
3. People pick the instance when they connect, with a GitHub App installation, a personal access
   token or OAuth.

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

## Upgrading from the separate GitHub App package (instance admins)

Earlier versions shipped the GitHub App as its own packages (`github-app`, with the apps *GitHub
App* and *GitHub App on Enterprise Server*). Version 2.0 of this package replaces them. The new
version adds hooks and egress to the hosts the old package already used, which is a **permission
widening**: after the upgrade, approve the new `github` version once under *Admin, Plugins*. When
you do, Aurora moves every connection of the old apps to **GitHub** / **GitHub Enterprise
Server** (the connection keeps its id, its stored key, its grants and its history), moves
registered Enterprise Server instances, skill libraries and app availability along, and disables
the old package. Nothing needs reconnecting. Until you approve, the old package keeps working.

## Limits

- Installation tokens (GitHub App connections) cannot call endpoints that act as a user.
- GraphQL: queries are read tools; any mutation is a write tool.
- Hosts other than the ones above, such as `uploads.github.com`, `gist.github.com` or
  `ghcr.io`, are not covered.
- Webhooks are not part of this package.
