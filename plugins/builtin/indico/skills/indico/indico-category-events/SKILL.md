---
name: indico-category-events
description: "Find the events of one or more categories in a date range with the Indico export API: category ids, filters and the answer's shape."
---

# Find the events of a category in a date range

Use for "what meetings are in category 12 next week". `$H` is the Indico address from `app.md`.
Needs the scope `read:legacy_api` (official export API, documented). No credential in the request.

## 1. Get the category id
The id is the number in the category's URL (`https://$H/category/12/`). With only a name, search:
`aurora-agent call indico search_categories q=<name> --raw` (internal endpoint, scope
`read:everything`; on a 404 or an unexpected shape ask the person for the id). The person's own
favorites: `aurora-agent call indico list_favorite_categories --raw` (`read:everything`).

## 2. List the events

```sh
aurora-agent call indico list_category_events categories=12.json from=2026-11-16 to=2026-11-22 detail=events type=meeting
```

- The `categories` value is the id followed by `.json`; several categories are joined with a
  hyphen, `12-13-14.json`. The person's favorites in one go: `--path /export/categ/favorites.json`.
- `from` and `to` are dates, `YYYY-MM-DD`; `type` is the event type (for example `meeting`);
  `location` and `room` filter by place (`room=40-*` takes a wildcard), `limit=N` caps the count.
- `detail=events` lists events. For one event's programme use `get_event event=<id>.json
  detail=contributions` (or `sessions`), or run `indico-meeting-prep`'s `event-brief`.
- The answer is JSON with the events under `results`; each has `id`, `title`,
  `startDate.{date,time,tz}`, `endDate`, `roomFullname`, `location`, `url`. Read the fields the
  server actually returns instead of assuming a shape. The server may cap the number of results:
  when the list is long, narrow the dates. Add `--raw` and pipe into `jq` to cut the output:
  `jq -r '.results|sort_by(.startDate.date+.startDate.time)[]|"\(.id) \(.startDate.date) \(.startDate.time[0:5]) \(.title)"'`.

## 3. Present
Title, start (the server's local time), place and the event URL, sorted by start. Say how many
events were found and the range used. Events cannot be created through the API: prepare the details
and give the person the link to Indico's event creation page.
