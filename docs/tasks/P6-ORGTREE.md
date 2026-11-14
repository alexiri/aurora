# P6-ORGTREE: Nested organizations (an org tree) — **proposal, spec batch (Opus) later — phase 6**

> **Status: proposal (2026-11-14).** Written ahead of phase 6 so the ideas are ready; nothing here is
> decided. No D-, C-, SEC- or migration numbers are taken yet: the spec batch takes them from
> PROGRESS's next-free note when the owner starts phase 6. Open questions are `Q-P6-ORG-n` at the end;
> the recommendations are the coordinator's.
>
> **Owner answers, 2026-11-15 (D-276):** (Q-P6-ORG-1) **yes**: members of sub-orgs are members of every
> parent. Membership is upward-closed and removal cascades down, as proposed below. Q-P6-ORG-2..8 are open.

## Goal

Owner, 2026-11-14: "org hierarchy: we should be able to create nested orgs. We could, for example, have
an org per department, with suborgs per group, etc."

At CERN that is *Instance → IT department → IT-CD group → IT-CD-CLI section*. Each level wants its own
admins, its own credentials (a department LiteLLM account, a group's GitLab connection), its own policy
and governance on top of the parent's, and visibility of its subtree's usage. A parent wants to set
floors that every child inherits and cannot loosen, the way the instance does for orgs today
(`08 §11.1`).

## Today (what the tree has to fit into)

- Tenancy is flat: `Instance → Organization → Workspace` (`04 §Tenancy model`). `organizations.slug` is
  globally unique (`03`); workspaces are unique per `(org_id, slug)`.
- Everything below hangs off an organization and must get a tree answer: members and roles
  (`owner > admin > member`, statuses `active/suspended/pending`), invitations, groups (P4-GRP-1) and SCIM
  (P4-GRP-2, seat cap `max_members`, `exclusive`), org policy (rules + uncredentialed default from the egress
  posture), governance (LLM, egress, BYO, agent-token age; tighten-only below the instance, `08 §11`), org
  secrets/connections/OAuth clients (visible to every workspace, used only through a grant or rule), org
  skills and skill libraries, app availability (`app_posture`), org API keys, channel integrations,
  self-hosted app registrations, retention windows, the attachment quota (`ATTACH_ORG_QUOTA_*`), previews
  switch, freeze, org audit log, `members_can_create_workspaces`, new-user placement (the default org,
  D-140), the `org` API rate-limit bucket (`05 §Conventions`), instance-admin delete (P5-ADMDEL).
- The policy evaluator already combines scopes by **strictest verdict** (`07 §Evaluation`, step 3): a
  workspace rule can never loosen an org rule. Governance is already "each level only tightens". Both
  generalise to N levels without changing their shape.

## Proposed model

### Shape

- `organizations.parent_org_id uuid null` (null = **root org**). Existing orgs become roots; nothing
  changes for an instance that never creates a child.
- A closure table `org_closure(ancestor_id, descendant_id, depth)` (depth 0 = self) maintained in the same
  transaction as create/move/delete, so "ancestors of X" and "subtree of X" are one indexed query on the
  access-law hot path. (A materialised path is the alternative; the closure table makes moves cheaper to
  reason about and needs no string prefix tricks.)
- Limits: `ORG_DEPTH_MAX` 4 levels (root + 3), `ORG_CHILDREN_MAX` 200 per org. Slugs stay globally unique,
  so every existing URL and API path keeps working; the dashboard shows the path as a breadcrumb.
- A sub-org is a full organization: its own workspaces, members, groups, policy, credentials. There is no
  "folder" object (alternative considered below).

### Membership and roles

- **Membership is upward-closed (owner, D-276(a)):** an active member of a sub-org is always a
  member of its parent. Adding someone to a child who is not in the parent adds them to every missing
  ancestor as `member` in the same transaction (only if the actor may add members there, or the ancestor
  allows it, else 403). Removing or suspending someone in an ancestor removes/suspends them in the whole
  subtree (one transaction, offboarding view covers the subtree, `04 §Fire-time authority`). Consequences:
  the root's seat cap and SCIM admission gate bound the whole tree; root groups can contain anyone in the
  tree; one removal at the department offboards a person everywhere.
- **Inherited admin authority:** an ancestor's `owner` or `admin` has **implicit `admin`** in every
  descendant (the same repair access org admins have over workspaces today). Not `owner`: owner-only acts
  of a child (delete it, transfer its ownership, enable its SCIM) stay with the child's owners **or** the
  parent's owners acting on the parent (create/move/delete children is a parent action). Effective org role
  = the strongest of the direct role, the group-derived role (P4-GRP-1) and the inherited role.
- Plain membership does **not** flow down: a department `member` sees no group's workspaces unless
  bound there (privacy between sibling groups). Q-P6-ORG-2 asks whether a child may opt in to "every
  member of the parent may use workspace X" (it can already do that with a parent group binding, below).
- The access law keeps its shape (`can_use`, `can_own`, `can_approve` in `aurora-db::authz`); "the user's
  org role" becomes "the effective org role in the workspace's org, ancestors included".

### Inheritance of everything else (one rule: down the tree, tighten-only)

| Thing | Proposed behaviour |
|---|---|
| Governance (LLM, egress, BYO, agent-token age) | `effective = instance ∩ root ∩ … ∩ org` for permissions and lists, `max(...)` for postures. Each level can only tighten; a parent tightening later tightens the subtree live (the existing §11.2 re-evaluation, run per descendant). "Enforced by *IT department*" in the UI. |
| Org policy | The evaluator receives one rule list per scope, **root first, then each descendant, then the workspace**; the final verdict is the strictest (unchanged step 3). A child can never loosen an ancestor rule. Uncredentialed default and egress allow-list: a child can only narrow. Simulation and effective views show which org a matching rule came from. |
| Credentials (secrets, connections, OAuth clients, LLM keys) | Not inherited by default. An org-scoped credential may be **shared with descendants** (new flag, default off, set by the owning org's admins; Q-P6-ORG-3). A shared credential is visible to descendant workspaces and usable through grants exactly as an org credential is for its own workspaces. Unsharing or moving a child out of the subtree revokes the grants that used it (impact preview first). Governance key-scope rules gain the level `ancestor`. |
| Groups | Groups of an ancestor can be bound to workspaces of descendants (they contain only tree members by the upward-closed rule). Group → org-role mapping stays per org. |
| SCIM / directory sync | Root orgs only (Q-P6-ORG-4); SCIM groups are bound anywhere in the tree. |
| App availability | Available in a child only if available in every ancestor (`restricted` narrows down the tree). |
| Skills, skill libraries | Org-tier skills of ancestors apply to descendant agents (as instance → org → workspace → agent tiers today; ancestor first). Libraries: shared down like credentials (opt-in). |
| Retention | Effective window = the shortest along the path; an ancestor lowering its ceiling clamps the subtree. |
| Attachment quota and every phase-6 usage limit | Usage of an org counts against **every ancestor** too; a parent sets each child's cap (≤ its own), the child can only lower it (`P6-LIMITS §Scopes`). |
| Freeze | Freezing an org freezes its subtree; lifting needs the authority that imposed it (unchanged rule, `04 §Freeze`). |
| Previews switch, members-can-create-workspaces | A child cannot turn on what an ancestor turned off. |
| Audit | A child's rows are readable by the child's admins and by every ancestor's admins (subtree audit view with an org filter). Security-critical events on a child also go to the parent's owners (Q-P6-ORG-5). |
| Channel integrations, self-hosted app registrations | Stay per org; a registration may be shared with descendants like a credential. |
| API rate limit `org` bucket | One bucket per org **and** each ancestor's bucket (a busy child cannot starve its siblings beyond the parent's budget); details in `P6-LIMITS`. |

### Lifecycle

- **Create a child:** `POST /v1/orgs/{org}/children` by the parent's `owner`/`admin`; optional org setting
  `members_can_create_suborgs` (default off). The creator becomes the child's owner; steps 2–4 of org creation
  (default workspace, empty policy generations, default agent) run as today. Root creation keeps
  `auth.org_creation`.
- **Move (reparent):** `POST /v1/orgs/{org}/move { parent_org_id | null }` — needs `owner` of the moved org
  **and** `owner`/`admin` of the new parent (or an instance admin for both); refuses cycles, depth overflow,
  and a member set that would break upward closure (unless `add_missing_members: true`). Dry-run returns the
  impact: governance that tightens, policies that start applying, shared credentials and grants that will be
  revoked, quotas that are exceeded. Security-critical audit on both sides.
- **Delete:** an org with children is refused (409 `org_has_children`); move or delete the children first.
  Instance-admin delete (P5-ADMDEL) may cascade with the subtree's slugs listed and typed confirmation of the
  root slug (Q-P6-ORG-6).
- **Placement:** the default org (D-140) may be any org in the tree. A follow-on (Q-P6-ORG-7): **placement by
  claim** — map an OIDC claim (e.g. CERN's department/group) to the org a new user lands in, extending the
  existing `role_mappings` to sub-orgs.

### Dashboard

- Org switcher shows the tree (indented, collapsible, search); breadcrumbs `IT › IT-CD › CLI` on org pages.
- Org settings gains **Sub-organizations** (list, create, move, delete; children's counts).
- Every inherited item is labelled "Inherited from *IT*" and read-only in the child; effective views (policy,
  governance, availability) show the contributing org per line.
- Follows `14 §Visual design` (tokens, shared primitives, both themes, 390 px).

## Alternative considered

**Org units (folders of workspaces inside one org).** Cheaper: no new tenancy level, only a grouping for
workspaces with per-unit admins. Rejected as the main design because the owner's example needs separate
admins, credentials, policy and governance per department and group, i.e. everything an org already has;
units would re-implement most of it. Could still come later for grouping workspaces inside a leaf org.

## Risks

- The access law and governance resolution are on hot paths (every request, every gateway resolve): the
  closure table and the existing caches with `org` invalidation must cover ancestors (an ancestor change
  invalidates the subtree).
- Upward-closed membership interacts with SCIM `exclusive`, seat caps, `pending` members and JIT; each path
  must add ancestors atomically or refuse.
- A move is the most dangerous operation (credential loss, policy change for running agents); dry-run first,
  audit, notify.

## Deliverables of the spec batch (when phase 6 starts)

Owning-doc changes: `04` (tenancy model, access law, roles, permission matrix, freeze, deletion, audit,
placement), `03` (`parent_org_id`, `org_closure`, sharing flags), `05` (children/move/tree routes, org object
gains `parent_org_id` and `path`, invalidation of subtrees), `07` (scope order, simulate/effective views),
`08 §11` (N-level governance, `ancestor` key scope), `10` (skills tiers, retention), `13` (channel
integrations), `14` (switcher, breadcrumbs, Sub-organizations page, inherited labels), `15`, `17` (new SEC
invariant: a descendant never loosens an ancestor; credentials never flow down without the share flag), `18`
(test plan), `API_CONTRACTS`. Then implementation briefs, roughly:

| Brief (proposed) | Scope | Size |
|---|---|---|
| P6-ORG-1 | Schema + closure table, create/move/delete children, upward-closed membership, effective role with inheritance, access law, caches/invalidation | L, Rust |
| P6-ORG-2 | N-level governance, policy scope chain in the gateway evaluator, simulate/effective views, retention and availability inheritance | L, Rust |
| P6-ORG-3 | Sharing credentials, libraries and registrations with descendants; grants across levels; revoke on unshare/move | M–L, Rust |
| P6-ORG-4 | Dashboard: tree switcher, breadcrumbs, Sub-organizations page, move dry-run dialog, inherited labels | L, Next.js |
| P6-ORG-5 | Subtree audit view, SCIM/groups across the tree, placement into sub-orgs (and by claim, if Q-P6-ORG-7 = yes) | M, both |

## Open questions for the owner

- **Q-P6-ORG-1** **Answered 2026-11-15 (D-276(a)): yes.** Membership upward-closed (a sub-org member is always
  a member of every ancestor; removal at a parent cascades down)? *(Was recommended: yes — it makes
  offboarding, SCIM and seat caps work for the whole tree.)*
- **Q-P6-ORG-2** Does plain membership flow down (department members automatically use group workspaces)?
  *Recommended: no*; a parent group bound in the child covers the cases that want it.
- **Q-P6-ORG-3** Credentials: shared down only by an explicit per-credential flag? *Recommended: yes, default off.*
- **Q-P6-ORG-4** SCIM at root orgs only? *Recommended: yes.*
- **Q-P6-ORG-5** Should a parent's owners be told (e-mail) of a child's security-critical events?
  *Recommended: audit view yes, e-mail opt-in.*
- **Q-P6-ORG-6** May an instance admin delete a whole subtree at once? *Recommended: yes, typed root slug, list shown.*
- **Q-P6-ORG-7** Placement by claim (OIDC department/group claim → sub-org) in phase 6? *Recommended: yes, as
  P6-ORG-5's second half; it is what makes the tree useful at CERN without manual sorting.*
- **Q-P6-ORG-8** Depth cap 4 enough (instance → department → group → section)?
