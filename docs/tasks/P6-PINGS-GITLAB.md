# P6-PINGS-GITLAB: The GitLab connector — **proposal, spec batch (Opus) later — phase 6**

> **Status: proposal (2026-11-27), part of `P6-PINGS.md`** (read that first: the neutral model, linking,
> refusal, threads, setup and the security invariants). D-493 records the owner's answers. No other
> numbers are taken. This file is the GitLab-specific part. It becomes `docs/26-gitlab-connector.md`
> and brief P6-PING-2 at spec time.
>
> GitLab API details below are as of GitLab 17/18. **The spec batch checks each one against the version
> gitlab.cern.ch runs** and records it in the fixtures. Items marked *(verify)* are the ones least certain.

## Manifest (proposed)

```json
{
  "package": { "id": "gitlab", "version": "1.0.0", "contract": 1, "publisher": "aurora" },
  "capabilities": {
    "threads": true, "edits": true, "buttons": false, "menus": false, "text_input": false,
    "reactions": true, "ephemeral": false, "native_status": false,
    "presence_dedicated": true, "presence_shared": false, "user_tokens": false,
    "email_verified": false, "commands": false, "history": true,
    "open_spaces": true,
    "addressed": ["assigned", "review_requested", "approval_requested", "review_submitted"],
    "files": { "max_bytes": 10485760, "max_count": 10 }
  },
  "integration_setup": { "fields": [ "server_url", "poll_seconds", "sso_username_autolink", "identity_provider", "webhook_doorbell" ] },
  "presence_setup": { "dedicated": { "fields": [ "token" ] } },
  "limits": { "message_max_chars": 60000, "writes_per_space_per_minute": 20 },
  "public_callbacks": ["doorbell"]
}
```

- `email_verified: false`: GitLab shows a user's e-mail only if they made it public, or to an admin
  token. Linking uses the SSO username (`P6-PINGS.md`).
- The `message_max_chars` value is a proposal. GitLab's own note limit is far higher (1 000 000 characters
  *(verify)*). A shorter limit keeps answers readable, and longer ones split between blocks as today.
- Files: an answer's attachments are uploaded to the project (`POST /projects/:id/uploads`) and linked in
  the comment. Inbound files linked in a comment are fetched through `file.fetch` when they are project
  uploads.

## The token

- **A personal access token of the agent's own account, scope `api`.** `read_api` is not enough: the
  connector writes comments and award emoji and marks to-dos done.
- The account should be a regular user or a GitLab service account. Group and project access tokens
  create bot users. Whether a bot user gets to-dos when mentioned is uncertain *(verify)*, so they are
  not the documented path.
- `credentials.validate`:
  - `GET /user` gives `id`, `username` (the presence handle) and `bot`.
  - `GET /personal_access_tokens/self` gives `scopes` and `expires_at`.

  A token without `api` is refused with a message that names the scope.
- **Stored once** (D-494). The token is the credential of the GitLab app connection the presence points
  at: the app's `token` connect method on the same `{instance}`. The token also needs the `self_rotate`
  scope where the server has it *(verify: the scope's name, and whether `api` already allows
  self-rotation)*. The connect form's scope hint for agent accounts says so.
- **Expiry.** Recent GitLab versions require an expiry, at most a year by default (an admin can change
  that).

## Rotation (D-494)

- Once a day per presence, the connector reads `GET /personal_access_tokens/self`. When `expires_at` is
  fewer than `rotate_days_before` days away (default 30), it calls:

  ```
  POST /api/v4/personal_access_tokens/self/rotate   { "expires_at": "<today + token_lifetime_days>" }
  ```

  The answer carries the new token. GitLab **revokes the old token at once** *(verify the version that
  added it and the `expires_at` parameter; without the parameter the new token's expiry is GitLab's
  default, possibly a week, and the connector must then rotate on every cycle)*.
- The connector **keeps the new token in memory** and writes it back
  (`POST /v1/connector/credentials`, target `{ connection_id }`, compare-and-set on
  `credentials_version`). It retries with backoff until the write-back is stored. Meanwhile it
  uses the new token itself.
- **A compare-and-set conflict** (a person pasted a token meanwhile): the connector revokes the token it
  made (`DELETE /api/v4/personal_access_tokens/self`, authenticated with that token) and uses the stored
  one from its next assignment.
- **A server too old for self-rotation, or a refusal** (`403`, a missing scope): no rotation. The expiry
  warning stays, and the presence's row says why the token isn't renewed automatically.
- **A lost token.** If the host dies between the rotate and the stored write-back, the new token is lost
  and the old one is already revoked. The next poll answers `401`: the presence and the connection go
  `credential_dead`, and the owners are told to paste a new token (`P6-PINGS.md`).

## Receiving: the agent account's to-do list

GitLab already does the "who was tagged" work: every direct tag creates a **to-do** for the tagged user, in
every project that user can read. The connector polls, per presence:

```
GET /api/v4/todos?state=pending&per_page=100
```

on the integration's interval (default 30 s). For each to-do, by `action_name`:

| `action_name` | Event | Speaker |
|---|---|---|
| `mentioned`, `directly_addressed` | `message.posted`, `bot_mentioned: true` | the to-do's `author` |
| `assigned` | `item.addressed` `assigned` | `author` |
| `review_requested` | `item.addressed` `review_requested` | `author` |
| `approval_required` (the agent was made an approver) | `item.addressed` `approval_requested` | `author` |
| `review_submitted` (someone reviewed the agent's merge request) *(verify the name and the version)* | `item.addressed` `review_submitted` | `author` |
| `build_failed`, `unmergeable`, `merge_train_removed`, and any action without a person behind it | none in the first version (Q-P6-PING-6) | — |
| `marked` (the account added it itself), and every other action | ignored | — |

For each to-do the connector does three things:

1. It builds the event:
   - `event_id = "todo:<todo id>"`;
   - `space` from `project` (or `group`), with `kind` from its visibility;
   - `thread_ref` from `target_type` and `target.id`, for example `MergeRequest:<id>` (global ids, so a
     moved item keeps its thread);
   - `item` from `target` (title, web URL; `kind` `issue`, `merge_request`, `commit`, `work_item`);
   - `author` from `author` (`id`, `username` as the handle, `name`; `kind: "bot"` when GitLab says
     `bot`).
2. For mentions, it reads the comment for its full text and its discussion. The to-do's `target_url`
   names the note (`#note_<id>`). The connector reads it with `GET /projects/:id/<issues|merge_requests>/:iid/notes/:note_id`,
   and finds its discussion id for `reply_to` *(verify the cheapest call)*. A mention in a description has
   no note: `message_ref` is the item itself.
3. It sends the events to `POST /events`. **Only after** the core answers `accepted` or `duplicate` does
   it mark the to-do done (`POST /todos/:id/mark_as_done`). That gives at-least-once delivery: a crash
   between the two resends, and the core drops the duplicate. A `rejected` event is also marked done,
   because the core has decided.

An edit that adds a mention creates a new to-do in GitLab *(verify)*, so it arrives as a new event.
Mentions of a group the agent account belongs to are not direct tags and are ignored. GitLab files them
as `mentioned` with the group in the text *(verify)*; the connector keeps only to-dos whose comment
mentions the account's own `@username`.

**Doorbell (optional).** A project or group webhook on *Comments*, *Issues* and *Merge request* events
calls the connector's `doorbell` callback with `X-Gitlab-Token` set to the integration's generated secret.
The connector checks the secret in constant time, then polls the to-do lists of the presences on that
integration at once, at most once per 5 s per presence. The webhook body is not read beyond what routing
needs. Group webhooks are a paid-tier feature on GitLab. Without a doorbell, the delay is the poll
interval.

**Cost.** One request per presence per interval when nothing happened. GitLab's authenticated rate limit
is per user, and each presence is its own user, so agents don't share a budget.

## History (`history.read`)

- `thread_ref` → the item's notes: `GET …/notes?sort=asc&order_by=created_at`, without system notes
  (`system: true`). With `after`, only the notes after that one.
- The description comes first, as the item's own message: the author, and the description through the
  same untrusted-text handling.
- At most 20 entries (the core's cap). Notes in another discussion than the tag's are included, because
  the agent needs the whole picture of the item.
- Confidential notes and items: the agent's account reads them only if GitLab lets it. Its answer goes into
  the same item, where the same people can read it. The connector never moves text between items.

## Posting

- **The answer** (`message.post` with `reply_to`): a reply in the note's discussion,
  `POST /projects/:id/<issues|merge_requests>/:iid/discussions/:discussion_id/notes`. Without a
  discussion (a description tag, an `item.addressed` event, a commit), a new note on the item that starts
  with `@<speaker>`.
- **The receipt**: award emoji `eyes` on the note (`POST …/notes/:note_id/award_emoji`), or on the item
  for a description or `item.addressed`. It is removed when the answer posts (`13 §4.5`).
- **The refusal**: award emoji `speak_no_evil` (🙊) on the note, once.
- **Failure line, approval cards, frozen line**: notes, under the shared-space rules (`P6-PINGS.md`).
  An approval card gives no details and has no buttons.

## Rendering (`13 §4.6`, connector duty)

GitLab-flavoured Markdown is close to the neutral Markdown. Most of the text passes through as-is. What
the connector must change:

1. **Quick actions.** GitLab runs a line that starts with `/<command>` in a note as a command, with the
   author's rights. Outside code blocks and code spans, the connector prefixes every line that starts with
   `/` followed by a letter with a zero-width space (U+200B). GitLab then shows the line and doesn't run
   it *(verify on gitlab.cern.ch, and pick another escape if this one is stripped)*.
   - This covers `/merge`, `/approve`, `/close`, `/assign`, `/label`, `/award`, … in one rule. It also
     covers commands GitLab adds later.
   - Fixtures: every command in GitLab's list, at the start of a line, after list markers, inside block
     quotes, inside fenced code (unchanged), and mixed case.
2. **Mass mentions.** `@all`, `@here` and mentions of groups are defused, as Mattermost defuses
   `@channel` (D-271).
   - `@all` notifies every member of the project.
   - A group mention (`@group`, `@group/subgroup`) notifies its members. Without a cheap way to tell a user
     from a group, every `@name` the agent writes that is not the speaker, a person mentioned in the
     thread, or a user found by `user.lookup` is defused (wrapped in a code span).
3. **Mentions the agent reads** (D-460): `mentions[].handle` is `@username`, and `label` is the display
   name. Issue and merge request references (`#12`, `!34`, `group/project#5`) stay as text: GitLab links
   them.
4. **Links and images**: under D-227. No inline images from outside the server. The link scheme
   allowlist applies.

## Tests (spec batch writes `18 §GitLab connector`)

- A fake GitLab (recorded fixtures) for the to-do list, notes, discussions, award emoji and token reads.
- Conformance suite (`13 §7`) with the GitLab capabilities.
- The table of to-do actions, mark-done ordering after a crash, duplicates.
- Every quick-action and mass-mention rendering fixture above.
- Linking by SSO username, the 🙊 refusal, the echo guard on the agent's own app comments.
- `reply_to` limited to its thread link.
- An e2e journey on a throwaway GitLab container, if one fits the CI budget *(spec batch decides)*.
  Otherwise the fixtures are the gate.
