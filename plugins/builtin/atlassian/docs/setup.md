# Connect Jira and Confluence

This package has four apps. Aurora adds the credentials to requests your agents send; agents never
see a token.

- **Jira Cloud** and **Confluence Cloud** connect an Atlassian Cloud site (`*.atlassian.net`)
  through `api.atlassian.com`. Both use the **same** Atlassian OAuth 2.0 (3LO) client.
- **Jira Data Center** and **Confluence Data Center** connect a self-hosted instance that an
  administrator has registered, with a personal access token.

## Atlassian Cloud (Jira Cloud and Confluence Cloud)

### Register the OAuth app (admins, once for both apps)

1. In the [Atlassian developer console](https://developer.atlassian.com/console/myapps/) create an
   **OAuth 2.0 (3LO)** app.
2. Under *Authorization* set the callback URL to `<your Aurora URL>/v1/oauth/callback`.
3. Under *Permissions* add the Jira API and the Confluence API and, for each, only the scopes you
   want to offer (see below).
4. Under *Settings* copy the Client ID and the Secret and enter them in Aurora under *Admin, Apps,
   Jira Cloud* (or Confluence Cloud). The client is shared: both apps use it.
5. To let people other than you sign in, set the app's distribution to *Sharing* in the console.

### Permissions (scopes)

Read access is offered by default; each write scope is a separate, clearly marked choice.

| App | Read (default) | Write |
|---|---|---|
| Jira Cloud | `read:jira-user`, `read:jira-work`, `offline_access` | `write:jira-work` |
| Confluence Cloud | `read:space:confluence`, `read:page:confluence`, `read:comment:confluence`, `search:confluence`, `offline_access` | `write:page:confluence`, `write:comment:confluence` |

Jira uses Atlassian's classic scopes (the ones Atlassian recommends; its granular Jira scopes are
beta). Confluence's v2 API only documents granular scopes, and its search endpoint uses the classic
`search:confluence`. `offline_access` lets Aurora refresh the sign-in; Atlassian rotates refresh
tokens, so Aurora stores the new one every time. Aurora holds every changing request for approval.

### Which site is connected

Atlassian's `accessible-resources` endpoint lists every site a sign-in can reach. When you connect,
Aurora records the **first site in that list** (its name labels the connection; its id and URL are
stored as `cloud_id` and `site_url`). Limitations:

- If you authorize several sites, the connection is **not pinned** to one of them. The gateway
  can only match `api.atlassian.com/ex/jira/<cloudid>/...` by path prefix, so an agent can use any
  cloud id that `GET /oauth/token/accessible-resources` returns, and the connection's token is valid
  for all of them (the user's own permissions still apply). Choose the site on Atlassian's consent
  screen if you want to restrict it.
- Agents are told to look the cloud id up themselves; the *List the Atlassian sites* tool is a read
  tool of both apps.

## Data Center (Jira Data Center and Confluence Data Center)

1. An administrator registers the host once: open *Instances* (organization owners and admins for
   one organization, instance admins for all), choose Jira Data Center or Confluence Data Center and
   enter the host name, for example `jira.example.com` (no `https://`, port or path). Aurora checks
   `/rest/api/2/serverInfo` (Jira) or `/rest/api/server-information` (Confluence) first. Registering
   decides where the credentials may be sent, so only register hosts you trust.
2. Each person creates a **personal access token** in their own profile (Jira 8.14+, Confluence
   7.9+; *Profile, Personal Access Tokens*) and pastes it when connecting. The token acts as that
   user with that user's permissions; create it with a short expiry.
3. If the server has a private address, the operator must allow it with `gateway.allowed_private`
   before registration and requests can reach it.

Limits: the instance host must be a bare DNS name, so a context path (`https://example.com/jira`)
is not supported. Jira Data Center is addressed through REST API v2, Confluence Data Center through
its `/rest/api` resources.

## What agents can do

Reading is allowed by default:

- **Jira** (cloud REST v3, Data Center REST v2): the connected user, projects, issues, JQL search,
  comments and the transitions available for an issue.
- **Confluence** (cloud REST v2 plus the v1 search endpoint, Data Center `/rest/api`): spaces, pages
  and their child pages, comments, and CQL search.

Changing things is a separate permission and is held for approval:

- **Jira**: create an issue, edit an issue, comment, move an issue through a transition. The
  approval shows the project, type and summary, or the issue key and the comment or transition.
- **Confluence**: create a page, update a page, comment on a page. The approval shows the space,
  title and version, or the page and the comment.

Deleting issues, pages or comments, administration, attachments, webhooks and Jira Service
Management / Jira Software (boards, sprints) endpoints are not offered. The Jira JQL search via
`POST /search/jql` is not offered either (it is a read over POST); long queries go in the `jql`
query parameter of the GET form.
