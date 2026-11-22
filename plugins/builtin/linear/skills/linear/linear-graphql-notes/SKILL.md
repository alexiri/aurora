---
name: linear-graphql-notes
description: "How to read, page, create and update Linear issues through GraphQL; read before the first request"
---

# Linear GraphQL notes

One endpoint: `POST https://api.linear.app/graphql`, JSON body `{"query": "...", "variables": {...},
"operationName": "..."}`, `content-type: application/json`. Aurora adds the credentials: send no token, no
`-k`. A request that holds only queries is a read; one that holds a mutation is held for approval. Send
one operation per request and name it, so the approval shows what changes.

## Read

```sh
curl -s https://api.linear.app/graphql -H 'content-type: application/json' -d '{
  "operationName": "Mine",
  "query": "query Mine { viewer { id name } issues(first: 20, filter: {assignee: {isMe: {eq: true}}}) { nodes { id identifier title state { name } } pageInfo { hasNextPage endCursor } } }"
}'
```

- Ask only for the fields you need; connections (`issues`, `projects`, `teams`, `comments`) return `nodes`
  and `pageInfo`. Page with `first: N, after: "<endCursor>"` until `hasNextPage` is false.
- Find an issue by its identifier: `issue(id: "ENG-123") { id title description state { name } }` (the
  identifier works as `id`). List teams with `teams { nodes { id key name } }`: a mutation needs the team's
  `id`, not its key.
- Filter with `filter: {...}` (`state`, `assignee`, `team`, `title: {contains: "..."}`); look the exact
  input type up in the schema (`__type(name: "IssueFilter")`) instead of guessing field names.

## Create and update

```sh
curl -s https://api.linear.app/graphql -H 'content-type: application/json' -d '{
  "operationName": "NewIssue",
  "query": "mutation NewIssue($input: IssueCreateInput!) { issueCreate(input: $input) { success issue { id identifier url } } }",
  "variables": {"input": {"teamId": "<team id>", "title": "Login fails with SSO", "description": "Steps: ..."}}
}'
```

- `description` and comment bodies are Markdown.
- `issueUpdate(id: "<issue id>", input: {...})` changes fields (`stateId`, `assigneeId`, `title`);
  `commentCreate(input: {issueId: "...", body: "..."})` comments. States are per team: read the team's
  `states { nodes { id name } }` first.
- Check `success` in the answer; a GraphQL error comes back with HTTP 200 and an `errors` array, so read it.
- Do not repeat a mutation after an error without looking whether the issue exists already.

## Errors

A permission error on a mutation means the connection was made with the read scope only. `RATELIMITED` in
`errors[].extensions.code` means pause and retry later.
