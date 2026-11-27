# P6-PINGS: An agent's own accounts on GitLab and Jira wake it when it is tagged — **proposal, spec batch (Opus) later — phase 6**

> **Status: proposal (2026-11-27).** D-493..D-495 record the owner's answers. No C-, SEC- or migration
> numbers are taken yet: the spec batch takes them from PROGRESS's next-free note.
>
> **Owner answers, 2026-11-27: every question is answered** (table at the end).
> - Q-P6-PING-1..5: D-493.
> - Q-P6-PING-7: D-494. The token is stored only once and is rotated in the first version (§One token,
>   rotated).
> - Q-P6-PING-6 and 8..11: D-495, the recommendations accepted as written.
>
> This file covers what the two connectors share: the core and dashboard changes. The platform
> details are in `P6-PINGS-GITLAB.md` and `P6-PINGS-JIRA.md`.

## Goal

Owner, 2026-11-27: "if we start to use separate accounts for gitlab, jira, etc., it would be nice if
regular pings in the platform (ie, a `@agent fix this please` in a gitlab comment) would be able to reach
the agent somehow, so it wakes up and react."

An agent gets its own account on GitLab and on Jira, for example `@nightmanager`. When someone tags that
account, the agent wakes up and gets a turn. Tagging includes a mention in a comment or description, being
assigned an issue, being asked to review a merge request, or being added as an approver. The agent's answer
is posted back as a comment in the same place. A later tag in the same issue or merge request continues the
same conversation.

## Today

- **Agents already have outside accounts, but only for outgoing work.** An agent reaches GitLab or Jira
  through an app connection that the gateway injects (`09`, `24`). Nothing comes back the other way.
- **Agent webhooks can wake an agent, but they don't fit here** (`10 §Webhooks`). A GitLab project webhook
  fires on every comment, and the agent pays a model turn to ignore most of them. A webhook turn has no
  speaker: it runs on its creator's authority, so anyone who can comment could give the agent
  instructions. The verification kinds (`hmac_sha256`, `bearer`) don't match GitLab's `X-Gitlab-Token`.
  And each project needs its own webhook.
- **The channel layer already solves the hard parts for chat platforms** (`13`; Mattermost in `20`).
  It has:
  - a durable inbox, with duplicates dropped by platform event id;
  - dedicated bot presences, one per agent;
  - user links, from a platform account to an Aurora user;
  - speaker authorization and reach grants;
  - thread ↔ conversation links;
  - mention handling (FIX-219, D-460);
  - seen-receipts, and answers posted back to the thread;
  - managed connectors run by `aurora-connector-host` (§3.1), with no deployment step.

  The core contains no platform names (CI-enforced).

## Proposal: GitLab and Jira as channel connectors

Two new **built-in connector types**, `gitlab` and `jira`, run in `aurora-connector-host` like
`mattermost`. They speak connector contract v1 with the additions below. The core keeps no platform names.

### How the platforms map onto the neutral model

| Neutral | GitLab | Jira |
|---|---|---|
| Integration (org, once per server) | a GitLab server (`gitlab.cern.ch`) | a Jira server (`its.cern.ch/jira`) |
| Dedicated presence (one per agent) | the agent's own GitLab account | the agent's own Jira account |
| Shared presence | **not offered** (`presence_shared: false`) | not offered |
| Space | a project (a group, for group-level work items) | a project |
| Thread | an issue, a merge request, a commit, an epic or work item | an issue |
| `message_ref` | a comment (note), or the item itself for its description | a comment, or the issue for its description |
| Seen-receipt | 👀 award emoji | "Working…" comment, edited into the answer (no reactions on Jira Data Center) |

Every space is treated as **shared** (`channel`, or `private` when the project is not public), never as a
DM. The rules for shared spaces apply: D-192 cards without details, and attribution.

### What wakes the agent

The owner's answer to Q4 is "anything that tags the agent directly". That means:

- **A mention** in a comment or a description. This stays `message.posted` with `bot_mentioned: true`.
- **Being assigned, asked to review, or added as an approver.** These carry no message, so contract v1
  gains one **neutral inbound kind**:

  `item.addressed = { space, thread_ref, item: { ref, kind, title, url }, actor: { ref, kind, display_name,
  handle?, email?, email_verified? }, reason: "assigned" | "review_requested" | "approval_requested" |
  "review_submitted", message_ref? }`

  - `kind` is a short neutral word (`issue`, `merge_request`, `commit`, `work_item`).
  - `reason` is a closed list. A connector maps its platform's own actions onto it and drops the rest.
  - `review_submitted` means someone reviewed a merge request the agent authored.

  The core processes it like `message.posted`, steps 1–11 of `13 §5.1`, with the actor as the speaker. The
  turn text is one platform-voice line composed by the core, for example: `@ann assigned you issue "Disk
  full on node 12" (https://…)`. Every field in that line is sanitised as a label.
- **Not in the first version (Q-P6-PING-6, D-495): to-dos without a person behind them.** Examples are a failed
  pipeline or a merge conflict on the agent's own merge request. There is no speaker whose access can be
  checked.

A connector declares the kinds it sends with a new capability, `addressed: ["assigned", …]`. A connector
without it never sends `item.addressed`.

### Who may talk to the agent (speaker, link, refusal)

1. **Echo guard** (unchanged): anything the agent's own account wrote is ignored. That includes the comments
   it posts through its app connection. Other agents' accounts aren't linked to people, so they are
   strangers (rule 4). Two agents can't wake each other in a loop.
2. **Linking by SSO username** (owner, Q1: "SSO usernames match across all apps"). This is a new
   integration opt-in, `sso_username_autolink`, set by an org owner or admin like
   `verified_email_autolink` (`13 §5.9`). For an unlinked author, the core links the platform account to an
   Aurora user when **all** of these hold:
   - the integration has the opt-in;
   - the platform username equals that user's SSO username for the identity provider the integration names
     (case-insensitive, exact);
   - the user is an **active member of the integration's organization**.

   Aurora stores only `sub` today (`03 §user_identities`). So the identity row gains `username`, written at
   every sign-in from the provider's configured claim (`preferred_username` by default, or `{claim:…}`
   such as `cern_upn`). The link is recorded as `method = sso_username` and audited `user_link.create`
   (`via: sso_username`), in the security-critical set like `verified_email`. The setting's text warns that
   it is only safe when every account on the platform is SSO-provisioned. A local account named like a
   person would otherwise speak as that person.

   The sign-in-link fallback (`13 §5.1` step 4) is **not used** on these platforms. It would post a link
   token in a public comment, and they have no DMs.
3. **Authorization** (unchanged): `can_use` on the agent's workspace, or an approved reach grant.
4. **Strangers get silence** (owner, Q2). An author who can't be linked, or a linked person without
   access, gets no reply. On a platform with `reactions`, the core adds one 🙊 reaction (neutral name
   `speak_no_evil`) to the message and stops.

   Q-P6-PING-9 (D-495): a linked organization member without access still gets the usual
   `pending` reach grant, with the owners told in the dashboard and by e-mail and push. Nothing is said on
   the platform. An unlinked author creates nothing.
5. **Frozen agent** (`13 §5.1` step 2a): unchanged. The one rate-limited platform-voice line is posted.

### Threads, context and the answer

- **One conversation per item.** The thread link key is the item, so every tag in the same issue or merge
  request continues the same `group` conversation (`13 §5.1` step 7).
- **Context comes on every tag, not only the first.** Between two tags people write comments the agent
  hasn't seen. `history.read` gains an optional `after` (a `message_ref`). On each turn the core includes,
  as the existing untrusted "earlier messages" block:
  - the item's title and description, on the first turn in a thread;
  - the comments since the last message the agent answered, at most 20, oldest first.

  A connector returns the description as the item's first message.
- **The answer goes where the tag was.** `message.post` gains an optional `reply_to` (a `message_ref`).
  A connector with `threads` replies inside the discussion of that comment. Without it, or for a tag in a
  description, the reply is a new comment that starts with a mention of the speaker.
- **The agent knows where its answer goes.** Its "where you talk" fragment lists each presence, for example:

  > `@nightmanager` on gitlab.cern.ch: people mention you, assign you issues and ask you to review merge
  > requests. Your answer to such a message is posted there as a comment for you. Don't also post it
  > yourself.

  The turn context says which item the message came from. Agent-facing text cites no internal ids (D-483).
- **Posting rules.** Answers, attachments, splitting and the failure line are unchanged (`13 §5.2`).
  Approval cards follow D-192 in these spaces: title, agent, rule, expiry and a dashboard link, with
  no buttons and no `approve <code>` (`commands: false`). Approvers are reached by e-mail and push as today.
  No narration cards are posted in these spaces (Q-P6-PING-10, D-495): a tracker comment is not a chat
  line, and the steps are on the dashboard.
- **Approval of the answer itself.** The owner answered Q3: "nothing new is needed". Note what that covers:
  - The posted answer is the conversation's reply, as on Mattermost. It is not an app tool call, so the
    app's approval rules don't hold it back.
  - What the agent does through its app connection keeps its approval rules: comments it writes itself,
    pushes, merges.
  - A person who connects an agent to a public project accepts that its answers appear there.

### Setting it up (a person, never the agent)

- **Integration (org owner or admin, once per server).** Settings are the server URL (with a base path,
  like FIX-224), `sso_username_autolink` with its identity provider, and the platform settings in each
  platform brief. Each connector's `credentials.validate` probes the server.
- **Presence (owner, Q5: `can_own` on the agent's workspace).** On the agent's "where it talks" section,
  the person picks the integration and pastes **the agent account's token**.
  - `credentials.validate` reads the account (`GET /user`, `GET /myself`). The presence stores its handle
    and `platform_user_ref` and goes `active`.
  - Raising attach from `can_use` to `can_own` is declared neutrally: a connector whose spaces anyone on
    the platform can write in declares `capabilities.open_spaces: true`, and attaching such a presence
    needs `can_own`. Detach keeps `can_use`.
  - `presence.provision` is not supported: a person creates the platform account. Deprovision does
    nothing on the platform: the person revokes the token there.
- **One token, stored once** (owner, Q7, D-494). See §One token, rotated, below.
- **The agent never sees the token, and can't attach itself.** It holds no credentials (gateway custody),
  and attaching widens who can give it work: a person's decision. When asked, the agent can say what is
  missing. The dashboard link to its "where it talks" section is in its fragment.

### One token, rotated (owner, Q7, D-494)

Owner, 2026-11-27: "Store the token only once and rotate it in the first version."

**The presence has no credential of its own. It points at an app connection.**
- The token is the credential of one app connection: the GitLab app's or the Jira Data Center app's
  *Personal access token* connect method, on the integration's server. The presence stores that
  connection's id (`connection_id`), not a token.
- The attach dialog accepts two things:
  - a pasted token. This creates the connection in the agent's workspace (scope `workspace`) through the
    app's existing token connect, in the same transaction as the presence;
  - an existing token-kind connection to the same server that the person may grant.

  Either way, the account it reads must be the account the presence is for. `credentials.validate` and
  the app's account check must read the same user.
- **The agent's app access is a separate choice, in the same dialog.** The dialog offers the usual grant
  levels (`read` / `ask` / `act`, or none), through the existing connect-and-grant path. The presence
  receives tags and posts answers whatever the agent's grant is. The grant governs only what the agent
  does through the app itself.
- **One credential, two readers.**
  - The gateway injects it for the agent's app calls, as today.
  - The connector host receives it decrypted in its assignment, like a presence credential today
    (`13 §3.1`).
  - The host receives a connection's token only while an **active presence** on an integration of a type
    its token is bound to points at it, and only when the connection's server equals the integration's
    server.
  - Only token-kind connections (a personal access token) can back a presence. An OAuth connection
    can't: the gateway refreshes OAuth tokens, and a credential has one writer.
- **Deleting the connection** while a presence points at it is refused (409 `connection_in_use`, naming
  the agent and the server); the person detaches first. Detaching leaves the connection and its grants as
  they are.

**Rotation (the connector is the one writer of the token).**
- The connector rotates its own platform credentials, as `13 §1` already says. Once a day per presence,
  it reads the token's expiry. When the expiry is fewer than `rotate_days_before` days away (integration
  setting, default 30), the connector:
  1. makes a new token valid for `token_lifetime_days` (integration setting, default 365, capped by what
     the server allows);
  2. writes it back through `POST /v1/connector/credentials`. That route gains the target
     `{ connection_id }`, with a compare-and-set on the connection's `credentials_version`.
- **The write-back** (API):
  - takes the connection's credential lock (the one OAuth refresh uses, `08`);
  - stores the new value and raises `credentials_version`;
  - sends `aurora_gateway_invalidate { scope: "connection", id }`, so every gateway drops the old value;
  - records `connection.credential_rotate` (actor: the connector; ids only), in the security-critical
    set;
  - and the next host assignment carries the new value.
- **A compare-and-set conflict** means a person pasted a new token meanwhile. Theirs wins: the connector
  revokes the token it just made and uses the stored one.
- **The gateway after a rotation.** Requests already in flight when the platform revoked the old token
  get `401`. The gateway re-reads a connection whose `credentials_version` moved since it was cached and
  retries that request once. A `401` means the platform did not act on it, so the retry is safe.
- **Each platform's own way** is in its file:
  - GitLab's rotate call revokes the old token at once, so the connector keeps the new token in memory
    and retries the write-back until it is stored;
  - Jira, if its API allows, creates the new token first and deletes the old one only after the
    write-back.

  If the host dies between GitLab's rotate and the write-back, the new token is lost. The presence and
  the connection both go `credential_dead`, the owners are told, and a person pastes a new token. The
  window is one request long and happens at most once a year per account.
- **When rotation isn't possible or fails**, the expiry warning remains: an attention item and an e-mail
  14 days before the expiry, and `credential_dead` after it. The connection's row says "Renewed
  automatically, next on <date>" or "Expires on <date>; renew by pasting a new token".

### Receiving: poll first, webhooks as a doorbell

Both connectors **poll** with each presence's token, on an interval set on the integration (default 30 s,
minimum 15 s). So no platform admin is needed, and the agent's account sees exactly what it can read.

An optional webhook (a `public_callbacks` entry, verified with a secret the core generates) is only a
**doorbell**: when it rings, the connector polls that presence at once. The webhook's body is never trusted
as the event. A forged delivery can only cause an early poll, which the connector rate-limits. The event
the core receives is always one the connector read back with the agent's own token.

### Rendering: what an agent's comment must never do

The connector escapes agent text for its platform (`13 §4.6`) under the D-227 and D-271 rules. On these
platforms, defusing means more than broadcasts: a comment can **act**.

- **GitLab quick actions** (`/merge`, `/close`, `/assign`, `/approve`, … at the start of a line) run with
  the agent account's rights. Every quick-action line is neutralised.
- **Mass mentions** (`@all`, group mentions, Jira's mentions of many users) are defused.

The platform briefs give the exact rules and their fixtures. An answer shaped by a prompt injection in an
issue must not be able to merge, close, reassign or ping a whole group.

## Security (SEC invariants at spec time)

1. **The core authorizes, never the platform** (`13 §6` invariant 1). Speaker = the platform author, linked
   by SSO username only under the audited opt-in, or by an existing link. Unlinked authors and people
   without access reach nothing. They get 🙊 or silence.
2. **Untrusted text stays data.** Comments, descriptions, titles and labels arrive fenced and labelled as
   untrusted, and every field of the composed `item.addressed` line is sanitised.
3. **An agent's comment can't act.** Quick actions and mass mentions are neutralised by the connector, with
   fixtures for each rule. An answer reaches only the item it answers (`reply_to` must belong to the thread
   link).
4. **Webhooks are a doorbell.** Their bodies never become events.
5. **Credential custody** (D-494).
   - The token is stored once, as the app connection's credential.
   - The connector host receives it only for a token-kind connection an active presence points at, on the
     same server, for a type its host token is bound to.
   - The connector is the token's only writer. Each write-back is a compare-and-set, under the
     connection's credential lock, audited in the critical set, and followed by a gateway invalidation.
   - The token is never shown to the agent, logged, or put in an operation payload beyond the existing
     sealed-reference rules.
6. **Attach needs `can_own`** for `open_spaces` connectors, and the opt-in for username links needs an org
   owner or admin. Both are audited, with the link in the critical set.
7. **No loops, bounded cost.** Echo guard and strangers' silence. Per-presence poll budgets, the existing
   inbox rate limits and app-turn caps apply.

## Rejected

- **Agent webhooks pointed at the platform.** They have no speaker, fire on every comment, and need
  per-project setup (§Today).
- **A "watch" block in each app manifest** (polling like skill libraries, D-149). It would rebuild
  speaker linking, reach, threads, receipts and posting a second time, outside the channel layer that
  already has them.
- **The agent configuring its own presence or holding its token.** That would put a credential in the
  model's context and widen the agent's reach without a person deciding.
- **The sign-in link in a public comment.** Anyone who clicks first would confirm a link for that
  platform account.
- **Trusting webhook bodies.** That would need per-platform signature schemes the platforms don't all
  have (Jira Data Center), for no gain over reading the event back.
- **Two sealed copies of the token, one for the presence and one for the app connection, with no
  rotation** (the first recommendation for Q7). The owner chose one stored token, rotated (D-494). Two
  copies can't both survive a rotation that revokes the old token.
- **An OAuth connection behind a presence.** The gateway refreshes OAuth tokens, so a rotating connector
  would be a second writer.

## Deliverables of the spec batch (when phase 6 starts)

- The answers are D-493..D-495. SEC invariants (§Security). C-numbers for:
  - `item.addressed` and the `addressed` capability;
  - `history.read` `after` and `message.post` `reply_to`;
  - the `open_spaces` capability and the `can_own` attach rule;
  - `sso_username_autolink` and the identity `username`;
  - the presence's `connection_id`, the host's access to that connection's token, the write-back target
    `{ connection_id }`, `connection.credential_rotate`, 409 `connection_in_use`, the gateway's re-read
    and one retry, and the rotation and expiry settings;
  - the 🙊 refusal reaction;
  - the two connector manifests.
- `13` (the additions above, §5.1 steps 4–5 and 7–8, §5.7, §5.9, §4.5), `04` (the link method, attach
  matrix, audit), `03` (`user_identities.username`, the presence's `connection_id` and the token's expiry
  under §Planned changes, with their migrations), `05` and `API_CONTRACTS`, `08` (the connector as the
  token's writer, the gateway's re-read), `14` (the attach dialog: a pasted token or an existing token
  account, and the agent's app access; integration setup; the renewal line and the expiry warning), `15` (connector host types), `17`, `18`. Two new platform files,
  `docs/26-gitlab-connector.md` and `docs/27-jira-connector.md`, like `20`.
- Proposed briefs:
  - **P6-PING-1** (Rust, core: the contract additions, linking by username, the refusal reaction, the
    context and reply rules, the attach role, the presence's connection, the rotation write-back and the
    gateway's re-read, expiry; L);
  - **P6-PING-2** (Rust: the GitLab connector, `P6-PINGS-GITLAB.md`; L);
  - **P6-PING-3** (Rust: the Jira Data Center connector, `P6-PINGS-JIRA.md`; L);
  - **P6-PING-4** (Next.js: the integration and attach dialogs, the token or an existing account with
    the app access, the renewal line and the expiry attention; M).

  The order is 1 first, then 2 and 4 beside each other, then 3.

## Questions for the owner (all answered 2026-11-27: D-493, D-494, D-495)

| # | Question | Answer |
|---|---|---|
| Q-P6-PING-1 | How does a platform author become an Aurora user, given there are no DMs for the sign-in link? | **Answered (D-493(a)):** "SSO usernames match across all apps". Link by SSO username, as an audited opt-in per integration. |
| Q-P6-PING-2 | What does a stranger get? | **Answered (D-493(b)):** silence, with a 🙊 reaction on the message where the platform has reactions. |
| Q-P6-PING-3 | Does an answer in a public project need something new (only private projects, or approval)? | **Answered (D-493(c)):** no. It is the person's decision when they connect the agent. Generally only shared agents are connected to public projects, and app approval rules already cover what the agent posts itself. |
| Q-P6-PING-4 | What wakes the agent? | **Answered (D-493(d)):** anything that tags the agent directly: mentions, assignments, review requests, approver requests, reviews of its merge requests. |
| Q-P6-PING-5 | Who may attach such a presence? | **Answered (D-493(e)):** `can_own`. |
| Q-P6-PING-6 | To-dos with no person behind them (a failed pipeline or a conflict on the agent's own merge request): do they wake it? | **Answered (D-495(a)): not in the first version.** No speaker means no access check. The agent can watch its own pipelines with a schedule or a watch. Revisit with a "creator" rule like schedules have. |
| Q-P6-PING-7 | One token for the presence and the app connection? | **Answered (D-494):** "Store the token only once and rotate it in the first version." The presence points at a token-kind app connection, and the connector rotates the token and writes it back (§One token, rotated). |
| Q-P6-PING-8 | Jira Cloud too? | **Answered (D-495(b)): Jira Data Center first** (what CERN runs). Cloud has other auth, account ids instead of usernames, hidden e-mails and ADF text. It would be the same connector type with a variant, in a follow-up. |
| Q-P6-PING-9 | A linked organization member without access: silent, but still a reach request to the owners? | **Answered (D-495(c)): yes.** Nothing on the platform, a `pending` reach grant and the owners told, as on Mattermost. Unlinked authors create nothing. |
| Q-P6-PING-10 | Narration cards in these spaces? | **Answered (D-495(d)): no.** The answer only; the steps are on the dashboard. |
| Q-P6-PING-11 | Jira: a tag in a comment visible only to a group or role. Is the answer restricted the same way? | **Answered (D-495(e)): yes**, the answer copies the tagging comment's visibility, so it never shows more people than the question did (`P6-PINGS-JIRA.md` §History). |
