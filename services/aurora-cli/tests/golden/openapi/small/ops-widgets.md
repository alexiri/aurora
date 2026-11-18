# Operations: widgets

## GET /v1/widgets

List widgets

Parameters:
- `limit` (query, optional, integer): Page size, at most 100.

## POST /v1/widgets

Create a widget

Body fields (JSON, top level):
- `name` (body, required, string): Display name.
- `size` (body, optional, integer)

## DELETE /v1/widgets/{id}

Delete a widget

Parameters:
- `id` (path, required, string)

## GET /v1/widgets/{id}

Read one widget

Parameters:
- `id` (path, required, string)

