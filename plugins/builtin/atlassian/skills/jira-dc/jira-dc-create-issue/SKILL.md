---
name: jira-dc-create-issue
description: "Create an issue with the right project, type and fields"
---

# Create an issue with the right project, type and fields

Use this for "file a bug in project ABC". Send no token and never send these requests to another host;
`aurora-agent` adds the credentials and the instance's base path. Creating an issue or a comment is a write:
it may wait for approval (see `tools.md`: `create_issue`, `add_issue_comment`). (Plain `curl` to
`<address>/rest/api/2/...`, with the address shown with the account in `app.md`, also works, without
`aurora-agent`'s conveniences.)

## 0. Search first

Look for an existing issue before you file: read `jira-dc-search` and run
`"$SKILLS_DIR/jira-dc-search/scripts/jira-find-similar" --project ABC <broad words>`. The same problem
already open: comment on it or tell the person instead of filing.

## 1. Find the project and its issue types

```sh
aurora-agent call jira-dc list_projects --out /tmp/aurora-agent/projects.json
aurora-agent call jira-dc get_project projectIdOrKey=ABC --out /tmp/aurora-agent/project.json
```

The first lists the projects visible to the token's user (`key`, `name`); pick the key. The second shows
the project, including its `issueTypes` (`id`, `name`, `subtask`). Use only a type the project lists; ask
the person when it is not clear.

## 2. Create

Write the description in Markdown to `draft.md`; `md2jira` converts it to Jira wiki markup and builds the
body (text that Jira would read as markup is escaped, so a draft cannot add macros):

```sh
"$SKILLS_DIR/jira-dc-create-issue/scripts/md2jira" --issue-json --project ABC --type-id 10001 --summary "Login fails with SSO" < draft.md > issue.json
aurora-agent call jira-dc create_issue --body @issue.json --out /tmp/aurora-agent/created.json
```

- `--type NAME` instead of `--type-id ID` also works; `--label L` (repeat) and `--priority High` are optional.
  No label is added for you. The summary is one line, at most 255 characters.
- In API v2 `description` and comments are Jira wiki markup (not Markdown, not ADF); that is why you write
  Markdown and convert. `md2jira < draft.md` alone prints the markup.
- Other optional fields go into `issue.json` under `fields`: `assignee: {"name": "<login>"}` (Data Center
  uses the user name), `parent: {"key": "ABC-1"}` for a subtask.
- A `400` names the missing or unknown field in `errors`: a project can require fields. Report the
  field to the person; do not guess a value.
- Confirm project, type and summary with the person first. Create **once**; after an error or a timeout
  search for the issue (`jira-find-similar`, or `search_issues` on the summary) before trying again.

## 3. Comment on an existing issue

```sh
"$SKILLS_DIR/jira-dc-create-issue/scripts/md2jira" --comment-json < comment.md > comment.json
aurora-agent call jira-dc add_issue_comment issueIdOrKey=ABC-1 --body @comment.json --out /tmp/aurora-agent/comment.json
```

## 4. Report

The answer has `key`; the link is `https://<address>/browse/<key>`. Tell the person the key, the type and
the link.
