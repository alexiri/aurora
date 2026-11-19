# Connect GitHub

This package has two apps. Aurora adds the credentials to requests your agents send to GitHub;
agents never see the token.

- **GitHub** connects **github.com** (REST API at `api.github.com`, GraphQL, and git over HTTPS).
- **GitHub Enterprise Server** connects a GitHub Enterprise Server that an administrator has
  registered (REST API at `https://<host>/api/v3`, GraphQL at `https://<host>/api/graphql`, and git
  over HTTPS on the same host).

Each app asks **"How do you want to connect?"** and offers up to four ways, and (for every one of
them) **Read only** or **Read and write**:

- **GitHub App (recommended for teams).** An app installed on the repositories you choose.
  Permissions are fine-grained and per repository, tokens last one hour, and access does not depend
  on one person's account.
- **GitHub App sign-in.** Acts as you through a GitHub App: only the permissions the app was
  given, on the repositories both you and the app can reach. Tokens last eight hours and refresh
  by themselves.
- **OAuth.** Acts as you, on everything your account can reach, with the scopes you tick.
- **Personal access token.** Acts as you; the quickest for personal use. A fine-grained token can
  be limited to chosen repositories.

## Read only: what GitHub enforces and what Aurora enforces

**Read only** is Aurora's: the gateway lets only the app's read tools through that connection and
blocks everything else (every write tool and any request it cannot identify as a read), whatever
grants an agent has. It works with every method and can be changed later on the connection. It
is the default choice for a new connection. Whether **GitHub** also refuses writes depends on the
method:

| Method | GitHub refuses writes when… | With Read only, Aurora… |
|---|---|---|
| GitHub App | the app lacks write permissions, **or** you chose Read only: Aurora then asks GitHub for a token with each permission it holds limited to `read` (Contents, Issues, Pull requests, Actions, Checks, Statuses, Deployments, Discussions, Packages, Metadata; never administration, secrets or workflows) | also blocks writes itself |
| GitHub App sign-in | the app lacks write permissions (a user token never exceeds the app's permissions or yours) | blocks writes itself |
| Personal access token | the token is fine-grained and has no write permission; a classic token's `repo` scope allows writes | blocks writes itself |
| OAuth | never: GitHub has no read-only repository scope (`repo` is read and write) | blocks writes itself |

Where GitHub does not stop writes, Aurora's block is the only protection: it applies to requests
that go through the Aurora gateway, which is every request an agent makes with this connection.

(Before Aurora merged them, the GitHub App was a separate "GitHub App" app. Existing connections
were moved to **GitHub** with their grants intact; see *Upgrading* below.)

## GitHub (github.com)

### Which connection method

- **GitHub App** (recommended for teams): see *Connect a GitHub App* below.
- **Personal access token** (simplest). Prefer a **fine-grained** token: the link in the connect
  window opens GitHub's token form pre-filled for read-only use (*Contents*, *Issues*, *Pull
  requests* and *Actions*: Read, 90 days; GitHub's documented URL parameters, which you can edit
  before creating the token). You choose the repositories there; GitHub then refuses everything
  else. A classic token needs the `repo` scope for private repositories, which also allows
  writing: choose Read only in Aurora with one. Paste the token when connecting. For a classic
  token Aurora shows the scopes GitHub reports (`X-OAuth-Scopes`); GitHub reports nothing for a
  fine-grained token (the `X-Accepted-GitHub-Permissions` header names what an *endpoint* needs, not
  what your token has), so check its permissions on GitHub.
  The connect window lists the permissions to tick on the fine-grained token form for the access
  you chose: for **Read only**, *Contents*, *Issues*, *Pull requests*, *Metadata* and *Actions*, each
  **Read**; for **Read and write**, *Contents*, *Issues* and *Pull requests* **Read and write** (plus
  *Metadata* and *Actions* Read). *Metadata: Read* is always on. Source: GitHub's
  [permissions required for fine-grained personal access tokens](https://docs.github.com/en/rest/authentication/permissions-required-for-fine-grained-personal-access-tokens).
- **GitHub App sign-in** (acts as you, fine-grained). An administrator registers a GitHub App
  once (below) and people connect with *Sign in with GitHub*.
- **OAuth**. An instance admin registers a GitHub OAuth app once (below), then people connect with
  *Sign in with GitHub*. Aurora offers only `read:user` and `read:org` by default, which reaches
  public data. GitHub has **no read-only scope for repository contents**: `repo` (private and
  public) and `public_repo` are marked as write choices because they allow changing code, and
  Aurora still holds every changing request for approval.

### Register the OAuth app (admins)

1. On GitHub open *Settings, Developer settings, OAuth Apps* (your own, or the organization's) and
   choose *New OAuth App*.
2. Set the authorization callback URL to {{aurora.callback_url}}
3. Generate a client secret.
4. Enter the Client ID and Client secret in Aurora under {{aurora.page:oauth_clients}}. A workspace
   owner can register a client for their own workspace on the app's setup page,
   {{aurora.app_setup}}, instead.

Aurora uses the authorization code flow with PKCE. If your OAuth app is configured to expire user
tokens, Aurora refreshes them with the refresh token; otherwise the token stays valid until it is
revoked.

### Register the GitHub App for sign-in (admins)

1. On GitHub open *Settings, Developer settings, GitHub Apps* and choose *New GitHub App* (on
   Enterprise Server, on your server).
2. Set the callback URL to {{aurora.callback_url}}, keep **Expire user authorization
   tokens** enabled, and tick *Request user authorization (OAuth) during installation* only if you
   want it. Webhooks can stay disabled.
3. Grant the permissions you want agents to have (for read-only: *Contents*, *Issues*, *Pull
   requests*: Read). A user token has only the permissions that both the app and the person have,
   and only on repositories both can reach. Install the app where it is needed.
4. Generate a client secret and enter the **Client ID** (not the App ID) and secret in Aurora under
   {{aurora.page:oauth_clients}}, for the *github-app* client. A workspace owner can register it
   on the app's setup page, {{aurora.app_setup}}, instead.

There are no scopes to tick: GitHub Apps get their permissions from the registration. Aurora
uses the authorization code flow with PKCE; the user token expires after eight hours and Aurora
refreshes it with the refresh token (which rotates on every use and expires after six months; if
it expires, the connection asks you to reconnect).

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
   under {{aurora.page:self_hosted}} (below); then choose that instance and enter the same host name as *Server
   host*. A server host that differs from the registered instance is refused.

**Limit to repositories** (optional) takes repository names (not `owner/name`, comma separated,
at most 500) from the installation's account; the token then covers only those. **Read only**
narrows the token to read permissions: Aurora first reads the installation
(`GET /app/installations/<id>`), then asks for `permissions` = the installation's permissions that
are on a fixed read allowlist (the list in the table above), each as `read`. If the installation
holds none of them, connecting fails with a message saying so. Changing a connection to or from
Read only takes effect when the current token (one hour) is replaced.

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

1. An administrator registers the server once: open {{aurora.page:self_hosted}} (organization owners
   and admins for one organization, instance admins for all), choose GitHub Enterprise Server and enter the
   host name, for example `github.example.com` (no `https://`, port or path). Aurora checks
   `GET /api/v3/meta` first. Registering decides where the credentials may be sent, so only
   register hosts you trust.
2. For OAuth, create an OAuth app **on that server** (as above) and add it under *Credentials,
   OAuth clients*, choosing the instance. Every instance has its own application.
3. For GitHub App sign-in, register a GitHub App **on that server** (as above) and add it under
   *Credentials, OAuth clients* as the *github-app-ghes* client, choosing the instance.
4. People pick the instance when they connect, with a GitHub App installation, GitHub App sign-in,
   a personal access token or OAuth.

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
