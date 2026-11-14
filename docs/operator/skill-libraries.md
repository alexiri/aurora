# Git skill libraries

(Phase 4, P4-GSL, D-148, D-149, D-154, SEC-83, SEC-84.) Normative text: [docs/10 §Git skill library](../10-agents-conversations.md), [docs/06 §Library fetch](../06-gateway.md), [docs/09 §3.4 `library`](../09-app-plugins.md), [docs/15 §Git skill library](../15-deployment-operations.md). Every key and route below was checked against the code (`crates/aurora-config`, `services/aurora-api/src/domains/skill_libraries`, `services/aurora-gateway/src/library_fetch.rs`).

An organization can connect a git repository as a **skill library**: a catalogue of skills (`<folder>/SKILL.md` plus files) that people add to an agent, a workspace or the organization. The repository is a *source*, never a tier: nothing in it reaches an agent until a person adds a specific skill at a specific commit, and nothing a later push changes reaches an agent until a person approves that exact change. There is nothing to install: no new Service, Secret, NetworkPolicy or webhook.

## What you need

1. The built-in **GitHub** (1.1.0 or later, both the github.com and the Enterprise Server app) or **GitLab** (1.2.0 or later) package, approved by an instance admin. The upgrade that adds the `library` block shows in **Admin › Plugins** as a widening ("can serve a skill library: reads repository files from `<host>`") and needs the usual re-approval, like a new host claim. A package without the block cannot serve a library.
2. The gateway can reach the git host's API as it already does for agents (the host is one of the app's claimed hosts), and the API can reach the gateway's internal port 4103. Outside Kubernetes set `api.gateway_internal_url` (default `http://aurora-gateway:4103`; `http://127.0.0.1:4103` in `deploy/dev/aurora.dev.toml`). The internal port must stay unreachable from sandboxes (SEC-09).
3. An organization **owner or admin** connects the git host as an **organization-level** app connection with a **read-only** credential (GitHub: a fine-grained token limited to the skills repository with *Contents: read*; GitLab: *read_repository*). Aurora only ever sends `GET` requests, but a token that can write is more access than needed.

Then **Skills › Skill libraries › Connect a repository** (or `POST /v1/orgs/{org}/skill-libraries`). Creation reads the repository once and fails with the reason if it cannot (`library_unreachable`: wrong name, a token that cannot see the repository, a host that is unreachable from the gateway).

## Configuration

| Key | Default | Meaning |
|-----|---------|---------|
| `skills.library.enabled` | `true` | Kill-switch. `false` hides the feature (every route answers 404 `skill_libraries_disabled`) and stops the refresh job; stored skills keep working. |
| `skills.library.refresh_interval_secs` | `900` | How often each library with added skills is checked. At least `300` (startup refuses a lower value, naming the key). |
| `skills.library.bytes_per_agent` | `1048576` | Library-sourced bytes per agent (an operator may only lower it, 1 to 1 048 576). Workspace (2 MiB / 50 skills) and organization (4 MiB / 100) budgets are fixed. |
| `api.gateway_internal_url` | `http://aurora-gateway:4103` | Where the API reaches the gateway's internal listener. |

In Helm put the keys under `config.skills.library` and `config.api`.

## What is stored, what is read

- **Read** (through the gateway, which holds the credential; the API never does): repository metadata and the files of the skills people browsed or added. Nothing is cloned. A library with added skills costs **one** call per interval while the repository is unchanged, plus a tree listing and the changed files after a push. Browsing costs a ref resolution, a tree and one read of each skill's `SKILL.md` (at most 200), cached in memory for five minutes; members opening the picker at the same time share one read. A 429 or 503 from the git host is not counted as a failure: the library keeps its state and is tried again after the host's `Retry-After` (default 5 minutes, at most 1 hour). Self-hosted instances apply their own rate limits: raise the interval if needed. The gateway limits itself to 20 fetches per second per replica and 5 per organization.
- **Stored**: only skills that were added (content, commit, blob ids, content hash) and open proposed updates (at most 4 MiB per organization). Symlinks, submodules, files over 64 KiB, skill folders with more than 50 files and unusable names are refused.
- **Never applied automatically.** A push creates a *proposed update*; a person with the tier's write authority (agent: can use the agent; workspace: workspace owner; organization: owner or admin) reads the **whole diff** in the dashboard and approves that exact content. There is no "track the branch" mode.

## Operating it

- **Libraries page**: each library's status and last error (`Error` with a revoked token or `reauth_required`, a moved repository, an unreachable host), last check, skills in use per tier, pending updates, **Check now** (once a minute), Pause and Resume, Delete. A library in `error` leaves agents untouched; it is retried after 15 minutes, then 1 hour, then 6 hours, and recovers by itself. **Reconnect** the connection under Credentials when the token was revoked.
- **Deleting a library** detaches its skills: they keep their content and become ordinary editable skills of their tier. Removing the package support, deleting the connection or deleting the repository changes nothing about stored skills either.
- **Metrics**: `aurora_skill_library_refresh_total{result}` (`unchanged`, `proposed`, `error`), `aurora_skill_library_pending_updates`, `aurora_skill_library_refresh_duration_seconds`, `aurora_gateway_library_fetch_total{step,status}`. Alert on a sustained `result="error"` and on pending updates older than a week (nobody reviews: agents keep running the pinned skills, which is safe but stale).
- **Audit**: security-critical rows `skill_library.create|update|delete`, `skill.library.add`, `skill.library.update_apply`, `skill.library.update_reject`, `skill.library.detach`; the job writes the best-effort system row `skill.library.update_proposed`.
- **Upgrade and rollback**: the migration `20261127000001_skill_libraries.sql` is additive. Rolling the API back leaves the tables and the pinned skills (ordinary skill rows) in place and the library features disappear.

## Security notes

Everything from a repository (names, descriptions, bodies, paths, error text) is untrusted: it is sanitized on load, shown in the dashboard as data and never interpolated into platform-voiced text. The gateway builds each request from the approved package's `library` templates with parameters that matched their patterns (no free-form path, query, host, header or redirect), only for an organization-level, active connection of the named organization, through the guarded resolver, with size and time caps. A repository is a proposer, never a decider.
