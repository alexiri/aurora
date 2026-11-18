# Find the events of a category in a date range

Use this for "what meetings are in category 12 next week". `$H` is the Indico address shown with the
account in `guide.md`. Needs the scope `read:legacy_api`. No credential in the request, no `-k`.

## 1. Get the category id

The id is the number in the category's URL (`https://$H/category/12/`). If the person gives only a
name, search: `GET /category/search?q=<name>` (internal endpoint, scope `read:everything`; on a 404 or
an unexpected shape ask the person for the id).

## 2. List the events

```sh
curl -s "https://$H/export/categ/12.json?from=2026-11-16&to=2026-11-22&detail=events&type=meeting"
```

- Several categories: `12-13-14.json`. The favorites of the person: `/export/categ/favorites.json`.
- `from` and `to` are dates, `YYYY-MM-DD`; `type` is the event type (for example `meeting`);
  `location` and `room` filter by place (`room=40-*` takes a wildcard).
- `detail=events` lists events; use `detail=contributions` or `detail=sessions` on one event
  (`/export/event/<id>.json`) when you need the programme.
- The answer is JSON with the events under `results`. Read the fields it actually returns (title,
  dates, URL) instead of assuming a shape; the server may cap the number of results, so when the
  list is long narrow the dates.

## 3. Present

Give title, start (the server's local time), place and the event URL, sorted by start. Say how many
events were found and the range you used.

Events cannot be created through the API: prepare the details and give the person the link to
Indico's event creation page.
