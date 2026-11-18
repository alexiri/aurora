# Operations: Items

## POST /a

Create an item

Body fields (JSON, top level):
- `title` (body, required, string): The title.
- `tags` (body, optional, array<string>)

## GET /a/search

Search items

Parameters:
- `page` (query, optional, integer): Page number, starting at 1.
- `q` (query, required, string)

## GET /a/{id}

Read an item

Parameters:
- `id` (path, required, string)

## PUT /a/{id}

Replace an item

Parameters:
- `id` (path, required, string)

Body fields (JSON, top level):
- `title` (body, required, string): The title.
- `tags` (body, optional, array<string>)

