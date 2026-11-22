---
name: indico-meeting-prep
description: "Brief on an upcoming or past Indico meeting: find it in the person's favorite categories, agenda, speakers, which slides are up, slides and minutes as text, summary and action items."
---

# Meeting prep briefing from Indico

Use for "what is on the agenda for the next section meeting?", "are the slides up for Friday?",
"summarise last week's meeting and its action items". Uses the official export API (scope
`read:legacy_api`), which is documented and stable. `$H` is the Indico address from `app.md`. Several
accounts: `--account <letter>` on the calls, `INDICO_ACCOUNT=<letter>` for the scripts.

## 1. Find the event
The person's favorite categories (needs `read:everything`; they differ per person, so read them,
never assume ids):

```sh
aurora-agent call indico list_favorite_categories --raw | jq -r '.[]|"\(.id)\t\(.title)"'
```

Pick the category whose title matches what was asked ("section meeting" is the category with that
word). Next or last event of a category (several categories: `12-13.json`):

```sh
aurora-agent call indico list_category_events categories=12.json from=2026-11-21 to=2026-12-21 --raw \
  | jq -r '.results|sort_by(.startDate.date+.startDate.time)[]|"\(.id) \(.startDate.date) \(.startDate.time[0:5]) \(.title)"'
```

"Next" is the first with a date from today on; "last" is the latest of `from=today-30d to=today`.
All followed events of a week: `--path /export/categ/favorites.json from=… to=…` (it also includes
the person's favorite events). Unknown category: `aurora-agent call indico search_categories q=<name>`.
Unknown event title: `search_events term=<word>.json` or `indico-search-talks-and-files`.

## 2. Read it

```sh
"$SKILLS_DIR/indico-meeting-prep/scripts/event-brief" EVENT_ID
```

It prints the header (time, room, chairs, URL), the description (HTML stripped), the links found in
the description, the agenda (time, title, duration, speakers, abstract, each talk's attachments and
minutes), "material on N" of the agenda items, the event minutes (Indico notes) and the event's own
attachments. Each file attachment ends with `-> get_event_file event=<e> folder=<f> attachment=<a>.bin`,
the arguments that download it. Raw source, when you need a field the brief leaves out:
`aurora-agent call indico get_event event=<id>.json detail=contributions --raw` (`.results[0]` with
`contributions[]`, `folders[].attachments[]`, `note.html`).

## 3. Read slides or minutes when asked to summarise
- Download with the arguments the brief printed:
  `aurora-agent call indico get_event_file event=<e> folder=<f> attachment=<a>.bin --out /tmp/aurora-agent/f.bin`.
  The `/event/.../attachments/...` URL itself is not reachable through the gateway.
- Text: `"$SKILLS_DIR/indico-meeting-prep/scripts/doc2text" /tmp/aurora-agent/f.bin 20000`. It reads PDF,
  PPTX (with speaker notes) and DOCX. PPTX and DOCX need nothing installed; a PDF needs `pypdf` or
  `pdftotext`, and the script says so when neither is there (then use the PPTX version, or say the PDF
  could not be read). Old `.ppt` and `.doc` are not supported: look for a PDF. Prefer the PDF when
  both exist. The output says when it was cut. For a deck of 100+ pages read the first pages and the
  summary slides, and say so.
- `link` attachments (recordings, shared documents) cannot be fetched: list them.
- Agenda and minutes often live in an external notes tool linked in the description. Unless it is
  reachable without signing in, say you cannot read it and give the link.

## 4. Report
Briefing: title, date and time, room (and the video link when the description has one), chairs; the
agenda as a short list with speaker and slot; which talks have slides ("slides: 2/5 uploaded"). For
summaries: 3-6 bullets per talk, plus action items and decisions when minutes exist (attribute them
to the owners as written). Always include the event URL (`https://$H/event/<id>/`). Say when the
person has an agenda item themselves. For "before the meeting" prep, offer a reminder if the agent
has a way to send one.
