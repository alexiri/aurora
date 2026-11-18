# Create an issue with the right project, type and fields

Use this for "file a bug in project ABC". `$J` is
`https://api.atlassian.com/ex/jira/<cloudid>/rest/api/3` (`<cloudid>` is the `id` of the site from
`GET https://api.atlassian.com/oauth/token/accessible-resources`). Send no token, no `-k`. Creating an
issue is a write: it may wait for approval (see `tools.md`: `create_issue`).

## 1. Find the project

```sh
curl -s "$J/project/search?query=abc&maxResults=10"
```

Use the project `key` (for example `ABC`). If several projects match, ask the person which one.

## 2. Find the issue types of the project

```sh
curl -s "$J/project/ABC"
```

The answer lists `issueTypes` (`id`, `name`, `subtask`). Take the type the person means (Bug, Task,
Story...); never use a type the project does not list. Subtasks need a `parent` and are created like
other issues.

## 3. Create

```sh
curl -s -X POST "$J/issue" -H 'content-type: application/json' -d '{
  "fields": {
    "project": {"key": "ABC"},
    "issuetype": {"id": "10001"},
    "summary": "Login fails with SSO",
    "description": {"type": "doc", "version": 1, "content": [
      {"type": "paragraph", "content": [{"type": "text", "text": "Steps: ..."}]}]}
  }
}'
```

- `description` and comments are **Atlassian Document Format** JSON, not plain text or Markdown.
  Send one `paragraph` per paragraph; plain text in `description` is a 400.
- Optional fields are the same shape as above: `priority: {"name": "High"}`, `labels: ["a","b"]`,
  `assignee: {"accountId": "..."}` (the account id, not the e-mail), `parent: {"key": "ABC-1"}`.
- A `400` names the missing or unknown field in `errors`: a project can require fields (for example
  components or a custom field). Report that field to the person; do not guess a value.
- Confirm project, type and summary with the person first. Create **once**; after an error or a
  timeout look the issue up (`GET $J/search/jql?jql=project=ABC AND summary~"..."` URL-encoded) before
  trying again.

## 4. Report

The answer has `key` and `id`; the link is the site's `url` (from `accessible-resources`) followed by `/browse/<key>`. Tell the person
the key, the type and the link.
