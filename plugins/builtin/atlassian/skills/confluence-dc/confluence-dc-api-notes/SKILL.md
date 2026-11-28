---
name: confluence-dc-api-notes
description: "How to find and read Confluence Data Center pages, search, create and update them; read before the first request to Confluence"
---

# Confluence Data Center API notes

`$C` is `https://<address>`, with the address shown with the account in `app.md` (it has no scheme, for
example `confluence.example.org`); the REST API is at `$C/rest/api`. Send every request to that address only.
Aurora adds the credentials: send no token, no `-k`.

## Find and read

```sh
curl -s "$C/rest/api/space?limit=25"
curl -s "$C/rest/api/content?spaceKey=DOC&type=page&limit=25"
curl -s "$C/rest/api/content/<id>?expand=body.storage,version"
curl -s "$C/rest/api/content/search?cql=space%3DDOC+and+title~%22onboarding%22"
curl -s "$C/rest/api/search?cql=text~%22onboarding%22"
```

- Spaces are addressed by `key`. `content` lists pages and blog posts; `content/search` is CQL over content
  and returns content objects, `search` is CQL over everything with excerpts. CQL: `space = "DOC"`,
  `title ~ "..."`, `text ~ "..."`, `type = page`.
- A content object has the body only when asked for with `expand=body.storage` (storage format, XHTML-like);
  `expand=version` adds the version number. Comments: `GET $C/rest/api/content/<id>/child/comment`.
- Lists are paged with `start` and `limit`; follow `_links.next` until it is absent.

## Create and update (held for approval)

```sh
curl -s -X POST "$C/rest/api/content" -H 'content-type: application/json' -d '{
  "type": "page", "title": "Release notes", "space": {"key": "DOC"},
  "body": {"storage": {"value": "<p>Hello</p>", "representation": "storage"}}
}'
```

- `ancestors: [{"id": "<parentId>"}]` puts the page below another page.
- A comment is `POST $C/rest/api/content` with `"type": "comment"`, `"container": {"id": "<pageId>",
  "type": "page"}` and the body as above.
- Updating is `PUT $C/rest/api/content/<id>` with `type`, `title`, `body` and
  `version: {"number": <current + 1>}`. Read the page first (`expand=version`); a wrong number is refused with
  `409`. The update replaces the body, so send the whole body.
- The page link is `$C/pages/viewpage.action?pageId=<id>`.

## Errors

`403` and `404` both mean the token's user cannot see that item.
