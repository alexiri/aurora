# Freeze: the emergency stop

(Phase 2, RFE-006, D-50.) Freeze is a **reversible** switch that stops one agent, every agent of a workspace or every agent of an organization from acting, while keeping every byte of state for investigation.
There is no configuration key: it is used through the API and the dashboard. The normative text is [docs/04 §Freeze](../04-identity-tenancy.md#freeze-phase-2-rfe-006-d-50).

## Who can freeze and unfreeze

| Scope | Freeze | Route |
|-------|--------|-------|
| One agent | workspace owner (or an org owner or admin) | `POST /v1/agents/{agent}/freeze` and `/unfreeze` |
| A workspace | the same | `POST /v1/workspaces/{ws}/freeze` and `/unfreeze` |
| An organization | the organization's owner | `POST /v1/orgs/{org}/freeze` and `/unfreeze` |
| An organization, as the operator | an **instance administrator** | `POST /v1/admin/orgs/{org}/freeze` and `/unfreeze` |

Every freeze needs a `reason` (1 to 500 characters). Freezing an already frozen scope keeps the original record. **Lifting needs at least the authority that froze**: a workspace owner cannot lift what an org admin froze, and an organization owner
cannot lift an instance-administrator freeze (403 `freeze_authority_insufficient`). An instance administrator's freeze does not give them access to the organization's data; it is a kill switch for an incident such as a compromised account.
The flags are independent: unfreezing an agent does not unfreeze its frozen workspace. An agent is frozen when it, its workspace or its organization is frozen.

## What stops, and what does not

Effective at the next gateway cache refresh (about 5 seconds):

- **Gateway.** Every credential of a frozen agent is refused (403 `frozen`), live tunnels and WebSockets are closed, and requests held for approval are answered 403 (their approvals become `abandoned`). Database-lane sessions end too.
- **Turns.** No turn starts (API: 423 `frozen`; chat channels answer once that the agent is paused). Queued turns are aborted and running turns are told to abort.
- **Sandboxes.** A running sandbox is parked. Its home volume is kept for investigation and the reconciler never destroys it.
- **Webhooks and schedules.** Webhooks answer 410 `frozen`; schedules do not fire (missed fires are not replayed after unfreezing).
- **BYO agents.** `GET /v1/agent/container-config` and `POST /v1/agent/context-credentials` answer 423 `frozen`.

**Not frozen:** the management plane. Owners and admins can still read transcripts, activity and audit, change policy and grants, rotate tokens and unfreeze.

## Operating it

- List frozen organizations: `GET /v1/admin/orgs?frozen=true` (instance administrator; id, slug, name and freeze state only).
- Every freeze and unfreeze is audited in the same transaction.
- To investigate, leave the agent frozen, read the transcript and the request log, then rotate its token or fix its policy before unfreezing.

Verify (on a test agent): freeze it with a reason; the response carries `frozen: { at, by_label, reason, authority, level }`; sending a message returns 423 `frozen`, and a gateway request with its credential returns 403 `frozen`; unfreeze and both work again.
