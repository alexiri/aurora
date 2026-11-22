---
name: indico-search-talks-and-files
description: "Find talks, slides, files and minutes across Indico with the search API (type, category, person and date filters), then read the files."
---

# Find talks, slides and minutes in Indico

Use for "find the slides from that talk at last year's workshop", "who presented about X?",
"minutes mentioning Y". Indico's search is an internal endpoint (scope `read:everything`, checked on
Indico 3.3 on 2026-11-21; it may change between versions). `$H` is the Indico address from `app.md`.
Several accounts: `--account <letter>` on the calls.

Reading files uses the scripts of `indico-meeting-prep` (`doc2text`, `event-brief`): run them as
`"$SKILLS_DIR/indico-meeting-prep/scripts/doc2text"`. If that skill is missing from this agent,
searching still works, but say that reading files needs it.

## 1. Search

```sh
aurora-agent call indico search q=AlmaLinux type=attachment category_id=12 sort=mostrecent --out /tmp/aurora-agent/s.json
```

- `type` is **required** (without it Indico answers HTTP 422): `event`, `contribution` (talks),
  `attachment` (files), `event_note` (minutes), `category`.
- Narrowing: `category_id=<id>` (ids from `list_favorite_categories` or `search_categories`),
  `person_name=<name>` (speaker, author or uploader), `start_range=[2026-01-01 TO 2026-11-21]`
  (also `[* TO 2025-01-01]`), `sort=mostrecent` (default is relevance), `page=2` (10 results per
  page; `pages` and `total` are in the answer).
- The answer's `aggregations` (category, person_name, start_range, venue, …) give bucket counts
  and the exact `filter` value: use them to refine a broad query instead of paging through
  hundreds of hits.
- Start specific: keyword, type and category or person. If `total` is over 30, refine; if 0, drop
  filters one at a time, try synonyms, or switch type (event, contribution, attachment).
- Results are public events plus what the person can see. Do not paste unrelated hits (other
  groups' minutes, say) into a chat.

## 2. Read the hits

```sh
jq -r '.total, (.results[] | [.type, (.title // .filename), (.start_dt // .modified_dt // "")[0:10], ((.event_path // [])|map(.title)|join(" > ")), .url] | @tsv)' /tmp/aurora-agent/s.json
```

Field shapes:
- event: `event_id, title, start_dt (UTC), location.room_name, persons[].name, url`
- contribution: `contribution_id, event_id, title, persons, start_dt, url`
- attachment: `event_id, contribution_id, folder_id, attachment_id, filename, attachment_type (file|link), modified_dt, user.name, event_path[], url`
- event_note: `event_id, contribution_id, content (plain text), url`

`start_dt` is UTC: convert it to the server's local time when reporting. URLs are relative: prefix
`https://$H`.

## 3. Open the file (when asked what it says)

```sh
aurora-agent call indico get_event_file event=<event_id> folder=<folder_id> attachment=<attachment_id>.bin --out /tmp/aurora-agent/f.bin
"$SKILLS_DIR/indico-meeting-prep/scripts/doc2text" /tmp/aurora-agent/f.bin 20000
```

The hit's `url` (`/event/.../attachments/...`) is not reachable through the gateway. `link`
attachments cannot be fetched: give the URL. The whole event's context (agenda, other slides,
download arguments): `"$SKILLS_DIR/indico-meeting-prep/scripts/event-brief" <event_id>`.

## 4. Report
The top 3-5 matches: title, speaker, event, local date and the direct link `https://$H<url>`. When
summarising content say which file and pages you read. If several talks fit, list them and ask
instead of guessing.

Other lookups: events by title only, `search_events term=<word>.json` (official API); a category by
name, `search_categories q=<name>`.
