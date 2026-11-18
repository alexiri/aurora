# Notion API notes

Base URL `https://api.notion.com/v1`. Aurora adds the credentials and the `Notion-Version` header
(`2026-03-11`): never send a token or a version of your own. No `-k`. Send JSON bodies with
`content-type: application/json` so approvals can show what a change does.

## What the connection can see

It sees only the pages the person shared during sign-in, and what it may do depends on the
capabilities of the Notion connection. `404 object_not_found` means the page is not shared with it;
`403` means a capability is missing: tell the person which.

## Requests the catalogue covers (`tools.md` shows your access)

| Task | Request |
|---|---|
| Who am I, users | `GET /v1/users/me`, `/v1/users`, `/v1/users/<id>` |
| A page, one property | `GET /v1/pages/<id>`, `/v1/pages/<id>/properties/<property id>` |
| Page content | `GET /v1/blocks/<block or page id>/children` (a page is a block) |
| Add content | `PATCH /v1/blocks/<id>/children` with `{"children": [...]}` |
| Create / update a page | `POST /v1/pages`, `PATCH /v1/pages/<id>` |
| Comments | `GET /v1/comments?block_id=<id>`, `POST /v1/comments` |
| Search | `POST /v1/search` with `{"query": "...", "filter": {...}}` |
| Databases | `GET /v1/databases/<id>` (lists its data sources), `GET /v1/data_sources/<id>` |
| Rows | `POST /v1/data_sources/<id>/query` with `{"filter": {...}, "sorts": [...]}` |

## Databases and data sources

A database holds one or more **data sources**. `GET /v1/databases/<id>` lists them; the rows are read
through the data source (`POST /v1/data_sources/<id>/query`), and a row is created with
`POST /v1/pages` and `"parent": {"data_source_id": "<id>"}`. Property values in a created row follow the
data source's schema (`GET /v1/data_sources/<id>` shows the property names and types).

`POST /v1/search` and data source queries are POST requests: they are **held for approval** even though
they only read.

## Pagination

List and query answers have `has_more` and `next_cursor`. Send `start_cursor=<next_cursor>` (query string
for GET, body field for POST) and `page_size` (at most 100) until `has_more` is false.

## Errors

`400 validation_error` names the field; `404 object_not_found` not shared or wrong id; `409 conflict_error`
retry once after a moment; `429` wait for `retry-after`. A refusal by Aurora has a JSON body with
`error.code`: read it and tell the person.
