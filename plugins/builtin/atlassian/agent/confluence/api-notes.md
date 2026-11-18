# Confluence Cloud API notes

`$C` is `https://api.atlassian.com/ex/confluence/<cloudid>/wiki` (`<cloudid>` is the `id` of the site in
`GET https://api.atlassian.com/oauth/token/accessible-resources`; that lookup goes through the Jira Cloud
connection, so if only Confluence is connected, ask the person for the cloud id). Aurora adds the
credentials: send no token, no `-k`.

## Find and read

```sh
curl -s "$C/api/v2/spaces?limit=25"
curl -s "$C/api/v2/spaces/<spaceId>/pages?limit=25"
curl -s "$C/api/v2/pages/<pageId>?body-format=storage"
curl -s "$C/rest/api/search?cql=type=page+and+text~%22onboarding%22&limit=10"
```

- API v2 has no search; the search lives at `$C/rest/api/search?cql=...` (CQL: `space = "DOC"`,
  `title ~ "..."`, `text ~ "..."`, `type = page`, `lastmodified > now("-7d")`). URL-encode it.
- Spaces are addressed by numeric `id` in v2, not by key; `GET $C/api/v2/spaces?keys=DOC` finds the id.
- A page answer holds the body only with `body-format=storage` (Confluence's XHTML-like storage format);
  `GET .../pages/<id>/children` lists the pages below it; comments are at `.../pages/<id>/footer-comments`
  and `.../inline-comments`.
- Lists are paged: follow `_links.next` (a path starting with `/wiki/...`, send it to
  `https://api.atlassian.com/ex/confluence/<cloudid>` unchanged) until it is absent.

## Create and update (held for approval)

```sh
curl -s -X POST "$C/api/v2/pages" -H 'content-type: application/json' -d '{
  "spaceId": "<spaceId>", "status": "current", "title": "Release notes",
  "body": {"representation": "storage", "value": "<p>Hello</p>"}
}'
```

- Updating is `PUT $C/api/v2/pages/<pageId>` with `id`, `status`, `title`, `body` and
  `version: {"number": <current + 1>}`. Read the page first to get the current version; a wrong number is
  refused with `409`. The update replaces the body, so send the whole body, not only the change.
- A comment is `POST $C/api/v2/footer-comments` with `{"pageId": "...", "body": {"representation":
  "storage", "value": "<p>...</p>"}}`.
- Tell the person the page title and its link (`_links.webui`, prefixed with the site's address).

## Errors

`403` and `404` both mean the connected user cannot see that item.
