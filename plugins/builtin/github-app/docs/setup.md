# Connect a GitHub App

A GitHub App acts as itself, not as a person. It is installed on chosen accounts and repositories,
has fine-grained permissions, and uses tokens that expire after one hour. Aurora keeps the app's
private key and mints a fresh installation token whenever one is needed; agents never see the key
or the token.

This package has two apps:

- **GitHub App** connects an installation on **github.com**.
- **GitHub App on Enterprise Server** connects an installation on a GitHub Enterprise Server that
  an administrator has registered.

## 1. Create and install the app

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

## 2. Connect it in Aurora

- **GitHub App** (github.com): choose *Connect*, enter the App ID, the installation ID, and upload
  the `.pem` file.
- **GitHub App on Enterprise Server**: an administrator first registers the server under
  *Instances* (host name only, for example `github.example.com`; Aurora checks
  `GET /api/v3/meta`). Then choose that instance when connecting and enter the same host name as
  *Server host*, together with the App ID, the installation ID and the `.pem` file. A server host
  that differs from the registered instance is refused.

Aurora signs a short-lived JWT with the key (RS256, issued 60 seconds in the past, valid for at most
10 minutes, issuer = the App ID), calls
`POST /app/installations/<installation id>/access_tokens` and uses the returned installation token
(valid for one hour) as the credential. It checks the values when you connect, so a wrong key or
installation id is reported immediately. The private key is stored encrypted and is never shown,
logged or included in approval summaries or error messages. To rotate it, generate a new key on
GitHub, delete the old one, and reconnect.

If the app is uninstalled or the key is deleted, the connection asks you to reconnect.

## What agents can do

Reading is allowed by default: the repositories of the installation, repository metadata, issues
and pull requests with comments, changed files, repository files, commits, issue and code search,
GraphQL queries, and `git clone` / `git fetch`.

Changing things is a separate permission and is held for approval: creating and updating issues,
commenting, creating and merging pull requests, creating, changing or deleting repository files,
creating branches and tags, GraphQL mutations, and `git push`. The app's own permissions still
apply on top: a write the app was not granted is refused by GitHub.

## Limits

- Installation tokens cannot call endpoints that act as a user, such as `GET /user`.
- Inbound webhooks (including GitHub's `X-Hub-Signature-256` signature) are an Aurora agent
  webhook feature and are not part of this package.
- Hosts other than the ones above, such as `uploads.github.com`, are not covered.
