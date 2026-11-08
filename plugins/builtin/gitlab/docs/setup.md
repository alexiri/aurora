# Connect GitLab

This package connects **gitlab.com**. Aurora adds the credentials to requests your agents send
to GitLab; agents never see the token.

## Which connection method

- **Access token** (simplest). In GitLab open *User settings, Access tokens* and create a personal
  access token, or a project or group access token to limit the reach to one project or group.
  Choose the narrowest scopes you need and paste the token when connecting.
  - `read_api` and `read_repository` let agents read projects, issues, merge requests,
    pipelines and files, and clone over HTTPS.
  - `api` additionally lets them create and change things. `write_repository` lets them push code.
- **OAuth**. An instance admin registers a GitLab application once (see below), then people
  connect with *Sign in with GitLab*. Aurora offers read access by default; write access is a
  separate, clearly marked choice. Tokens refresh automatically.

## Register the OAuth application (admins)

1. In GitLab open *User settings, Applications* (or the group or instance application list) and add an application.
2. Set the redirect URI to `<your Aurora URL>/v1/oauth/callback`.
3. Mark it **Confidential** and tick only the scopes you want to offer: `read_user`, `read_api`,
   `read_repository`, and optionally `api` and `write_repository`.
4. Enter the Application ID and Secret in Aurora under *Admin, Apps, GitLab*.

Aurora uses the authorization code flow with PKCE.

## What agents can do

Reading is allowed by default: your profile, projects and groups, issues and comments, merge
requests, diffs and comments, pipelines, jobs and job logs, branches, commits, repository
files, and `git clone` / `git fetch`.

Changing things is a separate permission and is held for approval: creating or updating issues and
merge requests, commenting, merging, running, retrying or cancelling pipelines, creating or deleting branches,
committing and changing files, and `git push`. The approval shows the project, the item and, for
JSON requests, the title or message.

## Self-managed GitLab

gitlab.com works out of the box. For your own GitLab, an administrator registers it once:

1. Open *Instances* (organization owners and admins for one organization, instance admins for all),
   choose GitLab, enter the host, for example `gitlab.example.com` (host name only: no `https://`,
   port or path), and register it. Aurora checks `GET /api/v4/version` first. Registering decides
   where GitLab credentials may be sent, so only register hosts you trust.
2. Create the OAuth application **on that GitLab** (see above) and add it under *Credentials,
   OAuth clients*, choosing the instance. Every instance has its own application.
3. People then pick the instance when they connect (access token or OAuth).

If the GitLab has a private address, the operator must allow it with `gateway.allowed_private`
(and the gateway's network policy) before registration and requests can reach it.
Deleting an instance disables its connections and removes its OAuth clients.

## Limits

- Requests to other GitLab hosts, such as `registry.gitlab.com`, are not covered.
- GitLab's GraphQL API is not offered; use the REST API.
