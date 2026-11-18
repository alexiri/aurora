# GitLab REST API v4 conventions

`$H` is the GitLab address shown with the account in `guide.md`. Everything below goes through the
proxy with no credential and no `-k`. Several accounts: add the header
`x-aurora-connection-id: <connection id>` (see `guide.md`).

## Addressing

- A project is `$P`: its numeric id, or its full path with every `/` encoded as `%2F`
  (`group%2Fsubgroup%2Fproject`). A group is addressed the same way.
- Git over HTTPS: `https://$H/<group>/<project>.git`.

## Pagination

List endpoints return 20 items by default. Ask for more with `per_page=100` (the maximum) and walk the
pages with `page=<n>`; the response headers `x-next-page` (empty on the last page), `x-total-pages` and
`x-total` say where you are. Stop when `x-next-page` is empty. Large lists can be sampled with
`per_page=100&page=1` before walking them all.

## Request bodies and errors

- Send write bodies as JSON with `-H 'content-type: application/json'`, so the approval can show
  what a change does.
- `401` means the connection has no valid credential; `403` that the token's scope or the person's role
  does not allow it; `404` that the project does not exist **or is not visible**; `409` and `422` come
  with a `message` that names the field.
- A refusal by Aurora (policy or approval) has a JSON body with `error.code`; read it and tell the person.

## Common requests

| Task | Request |
|---|---|
| Find a project | `GET /api/v4/projects?search=<words>&membership=true` |
| Merge requests | `GET /api/v4/projects/$P/merge_requests?state=opened` |
| One merge request's diffs | `GET /api/v4/projects/$P/merge_requests/<iid>/diffs` |
| Issues | `GET /api/v4/projects/$P/issues?state=opened&labels=<l>` |
| A file | `GET /api/v4/projects/$P/repository/files/<url-encoded path>/raw?ref=<branch>` |
| Pipelines and jobs | see `pipeline-failed.md` |

`iid` is the number shown in the web UI (per project); `id` is the global id. The path of a file must
be URL-encoded (`src%2Fmain.rs`). `tools.md` lists the operations the catalogue covers and what you
are allowed to do with each; a request that matches none is refused.
