# P6-PINGS-JIRA: The Jira connector (Data Center) — **proposal, spec batch (Opus) later — phase 6**

> **Status: proposal (2026-11-27), part of `P6-PINGS.md`** (read that first: the neutral model, linking,
> refusal, threads, setup and the security invariants). D-493 records the owner's answers. No other
> numbers are taken. This file is the Jira-specific part. It becomes `docs/27-jira-connector.md` and
> brief P6-PING-3 at spec time.
>
> **Jira Data Center first** (Q-P6-PING-8, D-495). Jira Cloud differs in auth, user ids
> (`accountId`, no usernames), e-mail privacy and text format (ADF); it would be a variant later.
> Jira API details below are as of Data Center 9/10. **The spec batch checks each one against the
> version CERN runs.** Items marked *(verify)* are the ones least certain.

## Manifest (proposed)

```json
{
  "package": { "id": "jira", "version": "1.0.0", "contract": 1, "publisher": "aurora" },
  "capabilities": {
    "threads": false, "edits": true, "buttons": false, "menus": false, "text_input": false,
    "reactions": false, "ephemeral": false, "native_status": false,
    "presence_dedicated": true, "presence_shared": false, "user_tokens": false,
    "email_verified": false, "commands": false, "history": true,
    "open_spaces": true,
    "addressed": ["assigned"],
    "files": { "max_bytes": 10485760, "max_count": 10 }
  },
  "integration_setup": { "fields": [ "server_url", "poll_seconds", "sso_username_autolink", "identity_provider", "webhook_doorbell" ] },
  "presence_setup": { "dedicated": { "fields": [ "token", "projects" ] } },
  "limits": { "message_max_chars": 30000, "writes_per_space_per_minute": 20 },
  "public_callbacks": ["doorbell"]
}
```

- **`threads: false`**: Jira comments are flat. A thread is the issue (`thread_ref` = the issue's numeric
  id, so a moved issue keeps its thread; the key, such as `OPS-123`, is the label).
- **`reactions: false`**: Data Center has no reactions on comments.
  - The seen-receipt is the `13 §4.5` fallback: a "Working…" comment, edited into the answer.
  - The 🙊 refusal becomes plain silence.
- **`addressed: ["assigned"]`**: Jira has no review or approver requests. Assignment is the only tag
  besides a mention.
- `server_url` includes the base path (`https://its.cern.ch/jira`), handled as in FIX-224.
- `email_verified: false`: Data Center often shows e-mail addresses, but linking uses the SSO username,
  which on Data Center is the Jira `name`.

## The token

- **A personal access token of the agent's own account** (Profile › Personal Access Tokens), sent as
  `Authorization: Bearer`. The agent account needs *Browse*, *Add comments* and *Edit own comments* in the
  projects it works in.
- `credentials.validate`: `GET /rest/api/2/myself` gives `name` (the handle and `platform_user_ref`),
  `key` and `displayName`.
- **Stored once** (D-494). The token is the credential of the Jira Data Center app connection the
  presence points at: the atlassian package's Jira DC personal-token connect, on the same instance.
- **Expiry.** A token may have an expiry, set when it is created or required by the admin. Its expiry
  can be read with `GET /rest/pat/latest/tokens` *(verify)*. When none can be read, the presence has no
  expiry warning and relies on `credential_dead`.

## Rotation (D-494)

Jira Data Center manages personal access tokens through `/rest/pat/latest/tokens`: create, list and
delete. **Whether a token may create its own successor is the open point** *(verify: some versions
accept only a password or session for these calls)*. When it may, rotation has no lost-token window,
because the old token stays valid until the new one is stored:

1. **Create.** When the expiry is fewer than `rotate_days_before` days away, the connector calls
   `POST /rest/pat/latest/tokens { "name": "aurora-<presence id>-<date>", "expirationDuration":
   <token_lifetime_days> }` with the current token. The answer carries the new token and its id.
2. **Write back.** `POST /v1/connector/credentials`, target `{ connection_id }`, compare-and-set on
   `credentials_version`.
3. **Delete the old token**, only after the write-back was stored: `DELETE /rest/pat/latest/tokens/<old
   id>`, called with the new token. On a compare-and-set conflict, the connector deletes the **new**
   token instead.

A token that never expires (allowed when the Jira admin sets no maximum) is not rotated. When the server
refuses token-authenticated creation, nothing is rotated. The expiry warning (`P6-PINGS.md`) is the
fallback, and the presence's row says that a person renews the token.

## Receiving

Jira has no to-do list like GitLab's. The connector finds tags itself, per presence.

**Mentions: poll the presence's projects.** Jira Data Center has no JQL function for "mentions me", and a
text search for the mention markup is unreliable. So the person attaching the presence names its
**projects** (`presence_setup.projects`: project keys, at most 50; the dialog lists the projects the token
can browse). Every interval (default 30 s), the connector runs:

```
POST /rest/api/2/search
jql = project in (<keys>) AND updated >= "<cursor − 2 min>" ORDER BY updated ASC
fields = summary, assignee, updated ; expand = changelog (only for the assignment check below)
```

- For each issue it reads the comments newer than its per-issue cursor,
  `GET /rest/api/2/issue/<id>/comment?orderBy=created`.
- It keeps those whose body mentions the account, `[~<name>]` (the account's own name, case-insensitive),
  plus a description edited to add the mention (seen in the changelog).
- Each becomes `message.posted`, `bot_mentioned: true`, `event_id = "comment:<id>"`. A description
  mention uses `"desc:<issue id>:<changelog id>"`.
- The cursor is stored by the connector per presence. The core's duplicate check makes overlap
  harmless.

**Assignments: anywhere.** This one doesn't need the project list:

```
jql = assignee = currentUser() AND updated >= "<cursor − 2 min>"
```

with `expand=changelog`. An `assignee` change to the account, made by a person, becomes `item.addressed`
`assigned`, with the actor taken from the changelog's `author` and
`event_id = "assign:<issue id>:<changelog id>"`. A change made by the account itself is ignored.

**Doorbell (optional, needs a Jira admin).** A system webhook (Data Center: *Administration › System ›
WebHooks*) on *comment created*, *comment updated* and *issue updated*. It is filtered by JQL to the
presences' projects and calls the connector's `doorbell` callback. It carries the integration's generated
secret in the URL's query or path, because Data Center webhooks can't send a custom header in every
version *(verify; newer versions may sign deliveries)*. On a ring, the connector polls the affected
presences at once, at most once per 5 s each. The body is never trusted (`P6-PINGS.md`).

**Cost.** Per presence per interval: one search per side (projects, assignments), plus one comment read
per updated issue. The 50-project cap and the cursor margin bound it. The spec batch sets a per-presence
request budget and what happens when it is exceeded: back off the interval and tell the agent's owners.

## History (`history.read`)

- The issue's description first, then its comments, oldest first. With `after`, only the comments after
  that one. At most 20.
- Comments with restricted visibility (a group or role) are read only if the account may see them. When
  the tagging comment was restricted, **the answer copies the same visibility** (Q-P6-PING-11, D-495), so
  an answer never shows more people than the question did. Otherwise the answer has no restriction.

## Posting

- **The answer**: a new comment (`POST /rest/api/2/issue/<id>/comment`) that starts with `[~<speaker>]`.
  When there was a "Working…" receipt comment, that comment is edited into the answer instead
  (`PUT …/comment/<id>`).
- **Failure line, approval cards, frozen line**: comments, under the shared-space rules (`P6-PINGS.md`).
  An approval card gives no details and has no buttons.
- **Refusal**: nothing at all.

## Rendering (`13 §4.6`, connector duty)

Jira Data Center stores comments as **wiki markup**, not Markdown. The connector converts the neutral
Markdown into it in Rust, using the same test vectors as the Jira app's `md2jira` helper (FIX-228), so
both write Jira text the same way. What the connector must also guarantee:

1. **Mentions.** An agent's `@name` becomes `[~name]` only for:
   - the speaker;
   - a person mentioned in the issue's thread;
   - a user `user.lookup` finds.

   Anything else stays as plain text.
2. **No macros.** Wiki macros (`{html}`, `{include}`, `{jira}`, `{panel}` with untrusted parameters, and
   any `{name…}` the spec batch doesn't allow) are escaped (`\{`). Agent text can't embed or fetch
   anything. Fenced code becomes `{code}` / `{noformat}`, with its own content escaped so it can't close
   the macro early.
3. **Mentions the agent reads** (D-460): `[~name]` in incoming text becomes `@name` with the display name as
   `label`. Issue keys (`OPS-123`) stay as text.
4. **Links**: `[label|url]` under D-227's link rules. No images from outside the server.

## Tests (spec batch writes `18 §Jira connector`)

- A fake Jira Data Center (recorded fixtures) for search with changelog, comments, `myself`, and the
  personal-token read.
- Conformance suite (`13 §7`) with the Jira capabilities.
- Mention detection (in a comment, after an edit, in the description, the own-name check), the
  assignment changelog, cursors and overlap.
- The wiki-markup conversion against the shared vectors, plus every macro-escaping fixture.
- Restricted-visibility answers.
- Linking by SSO username, silence for strangers, the echo guard.
