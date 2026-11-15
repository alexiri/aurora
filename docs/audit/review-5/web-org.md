# REVIEW-5 findings — web-org (saved by the coordinator from the reviewer's hand-back)

## WEBO-1 (high) — SCIM settings "re-authenticate" notice has no way to re-authenticate (URL only in a hidden span)
- `web/src/components/org/directory-view.tsx:100-104`, `:318-320`, `:367-369`, `:170-174`, `:60-63` (form always sends all three fields); cf. `account/delete-view.tsx:64`. Server `scim/management.rs:161-165` (stricter than C-472: re-auth for any domain/exclusive change). Test `web/tests/directory.test.tsx:128-149`.
- docs/14 §Directory sync "turning it on opens the re-authentication step"; C-472.

## WEBO-2 (high) — skill-library browse reads only the first 50 skills; `next_cursor` never followed
- `web/src/components/skills/library.tsx:357`, `:419-425`; `web/src/lib/api/skill-libraries.ts:94-107`; `web/e2e/support/skill-libraries.ts:37`; `web/tests/skill-library.test.tsx:348-421`. C-571 (API_CONTRACTS:1001).

## WEBO-3 (medium) — SCIM token revoke immediate, no confirm (`directory-view.tsx:217`, `:253-260`; test `directory.test.tsx:173-186`); docs/14 "revoke with a confirm"

## WEBO-4 (medium, spec-defect + code) — unbinding a group from one workspace shows the impact of deleting the whole group
- `groups/group-detail.tsx:391`, `:416-422`; `settings/group-access.tsx:33`, `:74`; C-464 (API_CONTRACTS:1087) has only `remove_member`. Fix: add `unbind_workspace=<ws>` to C-464 + handler.

## WEBO-5 (medium, owner decision) — Admin Runners page lacks provision / token / delete / replicas / capacity (docs/14:770-776)
- `admin/runners-view.tsx:49-92`, `:62`.

## WEBO-6 (medium) — lists silently truncated at the first page
- `lib/api/queries.ts:48-56` `useWorkspaces` (server default 50, `services/aurora-api/src/http/pagination.rs:11`; group-detail.tsx:299, 309); `lib/api/account.ts:98-110` `useOrgMembers` (limit 200, no cursor, no search; group-detail.tsx:130-140, 164-178); `account.ts:78-91` `useAccess` (ignores `next_cursor`/`derived_cursor`; access-tab.tsx:212; group-access.tsx:183); `lib/api/apps.ts:156-165` `useConnections`; `lib/api/freeze.ts:87-91` `useAdminOrgs` (placement-view.tsx:70-74, 182-186). `groups.ts:47-59` `fetchAllPages` is the pattern.

## WEBO-7 (medium) — directory-managed members' role stays editable (`org/members-view.tsx:49-52`, `:83-91`, `:97-103`); docs/14; C-477 server allows it (decide 409 `managed_by_scim`?)

## WEBO-8 (low) — group PATCH 409 shows the policy message; revision never refreshed (`lib/api/errors.ts:70`; `group-detail.tsx:47-51`, `:78-82`)
## WEBO-9 (low) — Delete offered for SCIM-managed groups the server refuses (`group-detail.tsx:435-443`; C-461)
## WEBO-10 (low) — start-diagnostics query fires with an empty agent id (`admin/runners-view.tsx:95-100`, `:218`)
## WEBO-11 (low) — Directory sync page vs docs/14: nav visible to admins and with scim off (`app-shell.tsx:284-287`, `:309-312`; `org-gate.tsx:24`; `directory-view.tsx:308-316`); token form always rendered (`:147-175`); 5 errors not 20 (`:282`; C-479); copy dialog; 14-day banner (`:249`); base-URL copy button (`:338-347`); per-IdP setup guide absent
## WEBO-12 (low) — one-off styling: `sm:mt-[1.625rem]` (group-detail.tsx:75; group-access.tsx:125), `ml-[1.625rem]` (automations/owner-controls.tsx:57), dialog widths `w-[min(…)]` (skills/library.tsx:162, :391; policy/rule-editor.tsx:235; agents/grant-editor.tsx:68; admin/runners-view.tsx:103; credentials/give-access.tsx:329), `tracking-[0.06em]` (placement-view.tsx:338), `grid-cols-[16rem_1fr]` (skills-view.tsx:77), `max-w-[…]` (runners-view.tsx:133-134, 197), native checkbox (skills-view.tsx:229, :301) vs `CheckboxField`, `max-w-[1160px]` repeated ~40 pages
## WEBO-13 (low, test-gap) — tests/mocks encode the gaps above
## WEBO-14 (low) — SCIM token kept in client state / mutation cache after shown (`directory-view.tsx:125`, `:138-140`, `:176-197`; `lib/api/scim.ts:126-133`)

## Not covered
- full test bodies; credentials/grant-editor/approvals logic beyond the diff; systematic wrapper-vs-openapi diff; 390 px / both themes visual checks.
