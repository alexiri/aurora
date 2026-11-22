---
name: jira-dc-create-issue
description: "Create an issue with the right project, type and fields"
---

# Create an issue with the right project, type and fields

Use this for "file a bug in project ABC". `$J` is `https://$H/rest/api/2`, where `$H` is the Jira Data
Center address shown with the account in `app.md`. Send no token, no `-k`, and never send these
requests to another host. Creating an issue is a write: it may wait for approval (see `tools.md`:
`create_issue`).

## 1. Find the project and its issue types

```sh
curl -s "$J/project"
curl -s "$J/project/ABC"
```

The first lists the projects visible to the token's user (`key`, `name`); pick the key. The second shows
the project, including its `issueTypes` (`id`, `name`, `subtask`). Use only a type the project lists; ask
the person when it is not clear.

## 2. Create

```sh
curl -s -X POST "$J/issue" -H 'content-type: application/json' -d '{
  "fields": {
    "project": {"key": "ABC"},
    "issuetype": {"id": "10001"},
    "summary": "Login fails with SSO",
    "description": "Steps: ..."
  }
}'
```

- In API v2 `description` and comments are plain text with Jira wiki markup (not Markdown, not ADF).
- Optional: `priority: {"name": "High"}`, `labels: ["a","b"]`, `assignee: {"name": "<login>"}` (Data Center
  uses the user name), `parent: {"key": "ABC-1"}` for a subtask.
- A `400` names the missing or unknown field in `errors`: a project can require fields. Report the
  field to the person; do not guess a value.
- Confirm project, type and summary with the person first. Create **once**; after an error or a timeout
  search for the issue (`GET $J/search?jql=...`, URL-encoded) before trying again.

## 3. Report

The answer has `key`; the link is `https://$H/browse/<key>`. Tell the person the key, the type and the
link.
