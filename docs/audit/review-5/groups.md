# REVIEW-5 findings — groups (saved by the coordinator from the reviewer's hand-back)

## GRP-1 (high, security, CONFIRMED; a FIX-063 test encodes it) — a reactivated member regains IdP-linked group access the IdP removed while the membership was suspended
- `crates/aurora-db/src/repo/groups.rs:467-476` (sync DELETE only in orgs where the user is active), `:430-436` (doc "survive until reactivation"); `services/aurora-api/src/domains/tenancy/service.rs:407-421` (status update, no re-sync); `scim/deprovision.rs:60-70` (`reactivate_membership`). Test `services/aurora-api/tests/it/groups_idp.rs:478` asserts `members == 1`.
- Spec: SEC-71 "a removal is effective for the next request"; D-156 "an absent claim removes every IdP-linked membership". Root cause in docs/04 §IdP-linked groups Sync wording "of each org they are an active member of".
- Fix: on any transition of the org membership out of `active` (dashboard suspend/remove, SCIM suspend/delete) delete the user's `source='idp'` rows in that org in the same tx + `notify_member_changed`. Amend docs/04, C-462.

## GRP-2 (medium, unwired) — IdP sign-in sync doesn't enforce the 5,000-members-per-group cap
- `repo/groups.rs:493-516`, limit `:17`; manual `groups/service.rs:429`; SCIM `scim/resource_groups.rs:430,524`. D-130(d), docs/04:623, docs/02:288.

## GRP-3 (medium, fail-open after config change) — IdP-linked memberships survive removal of a provider's `group_claim`
- `oidc/service.rs:650` (`if p.group_claim.is_some()`); `repo/groups.rs:555-563` (`idp_linked_providers` "orphan warning at startup" has no caller).

## GRP-4 (medium) — `my_role` in org and workspace lists shows the direct role, not the effective role
- `tenancy/routes.rs:362-368` (`workspaces::roles_of`, `repo/workspaces.rs:188-198`); `repo/orgs.rs:22,57,74`; consumed by `web/src/lib/api/freeze.ts:31`.

## GRP-5 (low) — `GET /v1/workspaces/{ws}/access` omits group-mapped org admins from implicit rows (`repo/workspaces.rs:230-236`, `:251`; docs/05 I-27, :509)

## GRP-6 (low, PLAUSIBLE, spec privacy) — any workspace owner can list any org member's groups via `GET /v1/orgs/{org}/groups?member=` (`groups/routes.rs:55-70`; `http/auth.rs:144-157`; C-460 API_CONTRACTS:1083)

## GRP-7 (low, PLAUSIBLE) — group PATCH/DELETE skip the org lock; racing sign-in sync hits FK violation and fails the sign-in (`groups/service.rs` ~238-244, ~296-300; `scim/resource_groups.rs:805-812`; `oidc/service.rs:651-655`)

## GRP-8 (low, test-gap) — permission matrix never runs a group-derived principal; nine `/v1/groups/{id}*` routes in EXEMPT (`tests/it/groups.rs:56-75`, `:348-455`; `permission_matrix.rs:243-287`)

## Not covered
- gateway `decide.rs` principal use on every path; dashboard group pages; IdP role-claim × group org-role mapping (`oidc/roles.rs`); audit contents.
