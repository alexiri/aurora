# Git skill libraries

(Phase 4, P4-GSL, D-148, D-149, D-154, SEC-83; any git URL since P5-SKLIB-1, D-448, SEC-148.) Normative text: [docs/10 §Git skill library](../10-agents-conversations.md), [docs/15 §Git skill library](../15-deployment-operations.md), [docs/06 §Library fetch](../06-gateway.md) (removed), [docs/09 §3.4 `library`](../09-app-plugins.md) (ignored). Every key and route below was checked against the code (`crates/aurora-config`, `services/aurora-api/src/domains/skill_libraries`).

An organization can add a git repository as a **skill library**: a catalogue of skills (`<folder>/SKILL.md` plus files) that people add to an agent, a workspace or the organization. The repository is a *source*, never a tier: nothing in it reaches an agent until a person adds a specific skill at a specific commit, and nothing a later push changes reaches an agent until a person approves that exact change. There is nothing to install: no new Service, Secret, NetworkPolicy or webhook, and **no GitHub or GitLab account or app connection**: any git server that speaks smart HTTP works (GitHub, GitLab, Gitea, Forgejo, Bitbucket, cgit, `git http-backend`).

## What you need

1. The API pods can reach the git host over https. The destination guard applies as for every API-side fetch: public addresses only. A server on a **private network** needs its address (or range) in `gateway.allowed_private`; without it the library cannot be read and says so ("The repository's address is not allowed").
2. An organization **owner or admin** opens **Agents › Skills › Libraries** (`/o/<org>/agents/skills/libraries`) and chooses **Add a library** (or `POST /v1/orgs/{org}/skill-libraries`). The dialog has two steps and saves nothing before the second:
   1. **Where is it?** Paste the repository's address (the browser page of the repository works too: a page inside a branch or folder fills the branch and folder), optionally a branch or tag and the folder that holds the skill folders, and, for a private repository, open **This repository is private** and enter the access token (and the user name when the host needs one). Only `https://` addresses are accepted. Choose **Check**.
   2. **What Aurora found.** The dialog shows the repository, the branch (or tag or commit) it read, the commit, how many skills can be added (and how many cannot), the first names, and says when it took the branch and folder from the address. Aurora reads the repository for this step and shows the reason if it cannot (`library_unreachable`), on the address when the address itself is refused. Choose **Add library** to save it (change the label first if you like), or **Back** to change something; nothing is lost going back. `POST /v1/orgs/{org}/skill-libraries/check` is the same read.

   Later, **Edit** changes the label, branch or tag, folder and the token (Replace, Remove, Add a token); the address is fixed (to use another repository, add a new library; a library that came from before addresses existed shows **Set address**). The library's **status** reads Active, Paused, "Can't be read" with the reason, "Needs an access token" or "Pending updates over budget". Under the table, **Shared with every organization** lists the instance's libraries (read-only, with **Browse**) when there are any.
3. For a **private** repository, a read-only access token (below). Aurora only ever reads, but a token that can write is more access than needed.

### Which token to create

| Host | Token | User name |
|------|-------|-----------|
| GitHub (github.com or Enterprise Server) | fine-grained personal access token limited to the skills repository, permission **Contents: read** | `x-access-token` (the default) |
| GitLab (gitlab.com or self-managed) | project or personal access token with the scope **`read_repository`** (a deploy token with `read_repository` works too) | `x-access-token` (the default) or `oauth2` |
| Gitea / Forgejo | access token with the **read** scope on repositories | any, e.g. `x-access-token` |
| Bitbucket (Cloud or Data Center) | repository access token with read access | `x-token-auth` |

Enter the user name in the "User name" field only when the host needs a particular one. The token is sealed in the instance's secret store, shown nowhere again (the library says only that one is saved and when), sent only to the repository's own address, and never put in a log, audit row or error. To change it, paste a new one (the repository is read with it first; if that fails the old one stays); to remove it, clear it (the repository is read without it first).

## Libraries for the whole installation (instance libraries)

(P5-SKLIB-2, D-449, SEC-149.) An **instance admin** can add a library that **every organization** sees beside its own
(`POST /v1/admin/skill-libraries`, `POST /v1/admin/skill-libraries/check`; at most 20). It is for a catalogue
the company wants everyone to draw from.

- **Where.** **Administration › What agents may do › Defaults › Skill libraries › Manage** (`/admin/skill-libraries`):
  the same table and the same two-step **Add a library** and **Edit** dialogs as the organization's page, with one more column,
  **Organizations using it**. People pick from it in **Add from library**, under the heading "Shared with every organization".

- **Who does what.** Only an instance admin's session manages it: the address, branch, folder, the access token,
  Pause, Check now, Delete (an API key never does, and an organization's owner or admin gets "not found" for its id).
  The token is the instance admin's: organizations are told only that one is saved.
- **What organizations see.** Their owners and admins see it in their libraries list (marked as shared with every
  organization; a paused one shows as Paused); people who can use a workspace see it in the **Add from library**
  picker (only while active). They browse it as their organization and add skills at any tier with the authority
  that tier needs. Nothing from it reaches an agent until someone in an organization adds a skill and approves its updates.
- **Numbers.** An organization's page counts only its own skills and pending updates. The instance admin's list has totals and
  how many organizations use each library (never which skills).
- **One refresh for everyone.** The library is read once per pass; each skill gets its own proposed update, and each
  organization's open-proposal budget (4 MiB) is its own: an organization over budget gets no new proposals and its page says
  "Pending updates over budget", while the others are unaffected. Freeing space clears it at the next pass.
- **Deleting** detaches its skills in every organization (they keep their content) and deletes the token; the audit log
  has one instance entry and one `skill_library.delete` entry in each affected organization.
- An organization cannot hide an instance library (open question Q-SKLIB-1, built without it).

## Configuration

(Phase 5, SET-5.) The three keys below are edited on **Administration › What agents may do › Defaults › Skill libraries** and read at each refresh and fetch;
setting one under `config.skills.library.*` in Helm locks it.

| Key | Default | Meaning |
|-----|---------|---------|
| `skills.library.enabled` | `true` | Kill-switch. `false` hides the feature (every route answers 404 `skill_libraries_disabled`) and stops the refresh job; stored skills keep working. |
| `skills.library.refresh_interval_secs` | `900` | How often each library with added skills is checked. At least `300` (startup refuses a lower value, naming the key). |
| `skills.library.bytes_per_agent` | `4194304` | Library-sourced bytes per agent (an operator may only lower it, 1 to 4 194 304). Workspace (8 MiB / 50 skills) and organization (16 MiB / 100) budgets are fixed. |
| `gateway.allowed_private` | (empty) | Private addresses the API may reach, for libraries and addon sources on a private network. |

In Helm put the keys under `config.skills.library` and `config.gateway`.

## For library authors: scripts in a skill

A skill that ships a helper script should run it as `"$SKILLS_DIR/<skill name>/scripts/<file>"`
(mark the file executable in the repository). Every agent's shell, background process, terminal
and SSH session has `SKILLS_DIR` set to the skills folder of the agent's harness, so the same
skill works on every harness. A path with a harness folder in it (`~/.claude/skills/<name>/…`)
works on one harness only and stops working when the agent's harness is changed.

## What is stored, what is read

- **Read** (by the API itself, with the git smart-HTTP protocol, nothing cloned or written to disk): the repository's refs, and the files of the skills people browsed or added. A library with added skills costs **one** discovery and one ref listing per interval while the repository is unchanged, plus a tree and a file read after a push. Browsing costs the same plus one read of each skill's `SKILL.md` (at most 200), cached in memory for five minutes; members opening the picker at the same time share one read. A 429 or 503 from the git host is not counted as a failure: the library keeps its state and is tried again after the host's `Retry-After` (default 5 minutes, at most 1 hour). Self-hosted servers apply their own rate limits: raise the interval if needed. Every answer is capped (32 MiB of body, 64 MiB inflated, 500 object ids per request, 20 s per read) and every object's id is checked.
- **Stored**: only skills that were added (content, commit, blob ids, content hash), open proposed updates (at most 4 MiB per organization) and the sealed token. Symlinks, submodules, files over 160 KiB, texts over 190 KiB written as JSON (each text travels to the agent as one piece), instructions over 48 000 characters, skills over 300 000 characters in all, skill folders with more than 50 files and unusable names are refused. Binary files are stored and counted but not yet copied into the agent's home. A skill's oversize extra file is only found when the skill is read for adding, not in the list.
- **Never applied automatically.** A push creates a *proposed update*; a person with the tier's write authority (agent: can use the agent; workspace: workspace owner; organization: owner or admin) reads the **whole diff** in the dashboard and approves that exact content. There is no "track the branch" mode.

## Operating it

- **Libraries page**: each library's status and last error ("The repository needs an access token", "The access token was refused", "The repository was not found, or the token cannot read it", "The branch or tag was not found", an unreachable host), last check, skills in use per tier, pending updates, **Check now** (once a minute), Pause and Resume, Delete. A library in `error` leaves agents untouched; it is retried after 15 minutes, then 1 hour, then 6 hours, and recovers by itself. Add or replace the token when it was revoked.
- **Deleting a library** detaches its skills: they keep their content and become ordinary editable skills of their tier, and the sealed token is deleted. Deleting or privatising the repository changes nothing about stored skills either.
- **Metrics**: `aurora_skill_library_refresh_total{result}` (`unchanged`, `proposed`, `error`), `aurora_skill_library_pending_updates`, `aurora_skill_library_refresh_duration_seconds`. Alert on a sustained `result="error"` and on pending updates older than a week (nobody reviews: agents keep running the pinned skills, which is safe but stale).
- **Audit**: security-critical rows `skill_library.create|update|delete` (a token change is recorded as `token: set|replaced|removed`, never the value), `skill.library.add`, `skill.library.update_apply`, `skill.library.update_reject`, `skill.library.detach`; the job writes the best-effort system row `skill.library.update_proposed`.
- **Upgrade and rollback**: see [Upgrades › Skill libraries](upgrades.md#skill-libraries-by-address-p5-sklib-1). Rolling the API back leaves the tables and the pinned skills (ordinary skill rows) in place.

## Security notes

Everything from a repository (names, descriptions, bodies, paths, error text) is untrusted: it is sanitized on load, shown in the dashboard as data and never interpolated into platform-voiced text. The API builds every request from the stored, normalized address (never from request input at read time), only over https, through the destination guard, with redirects only during discovery (at most three, each https and guarded) and never carrying the token to another origin, and with size and time caps; packs are parsed in memory and no repository configuration, hook or submodule is ever read or run. A repository is a proposer, never a decider.
