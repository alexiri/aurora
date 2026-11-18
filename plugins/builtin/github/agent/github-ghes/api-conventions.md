# GitHub Enterprise Server REST API conventions

`$H` is the Enterprise Server address shown with the account in `guide.md`. The REST API is at
`https://$H/api/v3`, GraphQL at `https://$H/api/graphql`. Requests go through the proxy with no
token and no `-k`; never send them to another host. Several accounts: add the header
`x-aurora-connection-id: <connection id>`.

## What this connection can reach

- A personal or OAuth connection acts as the person, limited by the token's own scopes. `403` or
  `404` on a repository you expect to exist means the connection has no access to it; say so.
- A GitHub App installation connection reaches only the repositories the app is installed on and
  only the permissions it was granted. Endpoints that act as a user (`/api/v3/user`,
  `/api/v3/user/repos`) are not available to it: list repositories with
  `GET /api/v3/installation/repositories`.

## Requests the catalogue covers (`tools.md` shows your access to each)

| Task | Request |
|---|---|
| A repository | `GET /api/v3/repos/<owner>/<repo>` |
| Issues, one issue, comments | `GET /api/v3/repos/<owner>/<repo>/issues`, `/issues/<n>`, `/issues/<n>/comments` |
| Pull requests, one, its files | `GET /api/v3/repos/<owner>/<repo>/pulls`, `/pulls/<n>`, `/pulls/<n>/files` |
| File contents | `GET /api/v3/repos/<owner>/<repo>/contents/<path>?ref=<branch>` |
| Commits | `GET /api/v3/repos/<owner>/<repo>/commits?sha=<branch>` |
| Search | `GET /api/v3/search/issues?q=<query>`, `GET /api/v3/search/code?q=<query>` |
| GraphQL | `POST /api/graphql` with `{"query": "..."}` (queries are reads, mutations are writes) |

A pull request is also an issue: **comment on a pull request through the issue comment endpoint**
(`POST /api/v3/repos/<owner>/<repo>/issues/<n>/comments`, body `{"body": "..."}`). Send write bodies
as JSON so approvals can show what a change does.

## Pagination and errors

Lists return 30 items. Use `per_page=100` and `page=<n>`; follow the `rel="next"` URL of the `link`
response header until it is absent. `401` bad credential; `403` forbidden or rate limited (wait, do
not loop); `404` missing **or not visible**; `422` validation, with `errors[]`. A refusal by Aurora
has a JSON body with `error.code`: read it and tell the person. The features of an Enterprise Server
depend on its version: on a `404` for an endpoint the catalogue lists, say that this server may be
too old for it.
