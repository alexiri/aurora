---
name: jira-dc-search
description: "Search Jira Data Center issues with JQL, and look for existing issues before filing one"
---

# Search Jira issues, and look before you create

Use for "find the issues about X", "what is open in project ABC", and before filing any new issue.

## The call

```sh
aurora-agent call jira-dc search_issues 'jql=project = ABC AND statusCategory != Done ORDER BY updated DESC' fields=summary,status,updated maxResults=50 --out /tmp/aurora-agent/s.json
```

Query parameters are bare `name=value` arguments (there is no `--query`); `aurora-agent describe jira-dc
search_issues` lists them. Quote an argument that has spaces. `fields=` limits what comes back (without it
every field of every issue is returned, which is large). The answer has `total`, `startAt`, `maxResults` and
`issues`: when `total` is more than the issues returned, ask for the next page with `startAt=50`, `100`, and so on.

## JQL you will need

- Open issues: `statusCategory != Done`. Do not list status names; they differ between projects.
- `labels = ops`, `assignee = currentUser()`, `updated >= -7d`, `created >= -30d`, `ORDER BY updated DESC`.
- `text ~ "disk"` searches summary, description, comments and more; `summary ~ "disk"` only the summary.
  Several words: `text ~ "disk" OR text ~ "quota"`. An exact phrase needs inner quotes: `text ~ "\"disk full\""`.
- Text search reserved characters are `+ - & | ! ( ) { } [ ] ^ ~ * ? \ :` (and `/`). They are not indexed;
  searching for a path or an error message verbatim finds nothing or fails. Search for its words.
  `*` and `?` are wildcards: `text ~ "dis*"`.
- A `400` from the search shows the JQL error; fix the query rather than retrying it.

## Before filing: search broadly

Run the script with broad words: the component, the host, the product, the key word of the error, not an
exact path or message. Tickets are written in people's own words, and an exact phrase misses them.

```sh
"$SKILLS_DIR/jira-dc-search/scripts/jira-find-similar" --project ABC disk quota demo-host
"$SKILLS_DIR/jira-dc-search/scripts/jira-find-similar" --project ABC --label my-agent    # your own open issues
```

It searches the open issues of the project that match any word (`--include-done` for finished ones too,
`--max N`, `--account A`), prints the JQL it sent, the count, and one line per issue: key, status, updated,
labels, summary. Words are quoted for you; reserved characters are dropped.

- A match that is the same problem: comment on it (`jira-dc-create-issue` has the comment steps) or tell the
  person; do not file a second one.
- A related one: file the new issue and mention it in the description (`Related: ABC-123`).
- Quote the printed JQL when you report the search.
