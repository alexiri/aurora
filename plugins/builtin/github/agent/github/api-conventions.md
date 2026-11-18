# GitHub REST API conventions

Requests go to `https://api.github.com` through the proxy with no token and no `-k`. Several
accounts: add the header `x-aurora-connection-id: <connection id>` (see `guide.md`).

## What this connection can reach

- A personal or OAuth connection acts as the person, limited by the token's own scopes (a
  fine-grained token only sees the repositories the person picked). `403` or `404` on a repository
  you expect to exist means the connection has no access to it; say so.
- A GitHub App installation connection reaches only the repositories the app is installed on and
  only the permissions it was granted. Endpoints that act as a user (`/user`, `/user/repos`) are
  not available to it: list repositories with `GET /installation/repositories`.

## Requests the catalogue covers (`tools.md` shows your access to each)

| Task | Request |
|---|---|
| A repository | `GET /repos/<owner>/<repo>` |
| Issues, one issue, comments | `GET /repos/<owner>/<repo>/issues`, `/issues/<n>`, `/issues/<n>/comments` |
| Pull requests, one, its files | `GET /repos/<owner>/<repo>/pulls`, `/pulls/<n>`, `/pulls/<n>/files` |
| File contents | `GET /repos/<owner>/<repo>/contents/<path>?ref=<branch>` |
| Commits | `GET /repos/<owner>/<repo>/commits?sha=<branch>` |
| Search | `GET /search/issues?q=<query>`, `GET /search/code?q=<query>` |
| GraphQL | `POST /graphql` with `{"query": "..."}` (queries are reads, mutations are writes) |

A pull request is also an issue: **comment on a pull request through the issue comment endpoint**
(`POST /repos/<owner>/<repo>/issues/<n>/comments`, body `{"body": "..."}`). Send write bodies as
JSON (`-H 'content-type: application/json'`) so approvals can show what a change does.

## Pagination

Lists return 30 items. Use `per_page=100` and `page=<n>`; the `link` response header carries
`rel="next"` and `rel="last"` URLs: follow `next` until it is absent. Search results are capped by
GitHub at 1000 items.

## Errors

`401` bad or missing credential; `403` forbidden, or a rate limit (see the `retry-after` and
`x-ratelimit-reset` headers; wait, do not loop); `404` missing **or not visible**; `422` validation,
with `errors[]` naming the field. A refusal by Aurora (policy or approval) has a JSON body with
`error.code`: read it and tell the person.
