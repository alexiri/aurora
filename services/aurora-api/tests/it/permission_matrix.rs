#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! TEST-010: route x principal permission matrix, generated from `docs/04 §Permission matrix`.
//!
//! Every management route in `web/openapi.json` must appear either in [`ROWS`] (checked against
//! a principal matrix) or in [`EXEMPT`] with a reason (see `coverage_every_openapi_route_is_listed`).
//!
//! Outcome rules (docs/04 §Authorization, §Authentication):
//! - no credentials => 401
//! - invisible (non-member, suspended, unbound for workspace routes) => 404
//! - visible but insufficient role => 403 `forbidden`
//! - workspace key on another workspace => 403 `workspace_mismatch`
//! - workspace key on an org-level route => 403 `forbidden`
//! - session-only route with an API key => 403 `session_required`
//!
//! Write requests carry a minimal syntactically valid body (see `body_for`; `{}` where every
//! field is optional), so body extraction does not mask the authorization outcome. An
//! authorized request then normally fails *semantic* validation (404/409/422) or succeeds.
//! Authorization is what is asserted.
//! Rows whose path uses only seeded ids (`org`, `ws`, `agent`, `user`) also assert that
//! authorized principals are *not* denied. Other rows only assert the denial side.
//! DELETE rows on seeded ids run every principal against a fresh database so that one
//! principal's successful delete cannot change another principal's outcome.

use crate::common::*;
use aurora_db::repo::agent_helpers as helpers_repo;
use aurora_testkit::git_server::{Change, GitServer};
use http::Method;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use uuid::Uuid;

// ---- principals ------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum P {
    Anon,
    /// Org owner (session).
    Owner,
    /// Org admin (session).
    Admin,
    /// Org member bound to the workspace as `member`.
    WsMember,
    /// Org member bound to the workspace as `approver` (phase 2, RFE-012).
    WsApprover,
    /// Org member bound to the workspace as `owner`.
    WsOwner,
    /// Org member with no workspace binding.
    Unbound,
    /// Membership suspended (still has an owner binding).
    Suspended,
    /// (D-175, FIX-082) Membership `pending`: SCIM prepared it for an existing account and the
    /// person has not signed in since. Denied everywhere a non-member is, even with an owner
    /// binding and group-derived access in place.
    Pending,
    /// (FIX-083, D-130) Plain org member whose **group** maps to the org role `admin`: an
    /// effective admin everywhere, though `org_members.role` says `member`.
    GroupAdmin,
    /// (FIX-083) Org member with no direct binding whose group is bound to the workspace as
    /// `owner`: a group-only workspace owner, an ordinary member of the org.
    GroupWsOwner,
    /// (FIX-083, SEC-71) Membership suspended, but still in the admin-mapped and the
    /// workspace-owner groups: derives nothing from either.
    SuspendedGroupAdmin,
    /// No membership in the org.
    NonMember,
    /// Instance admin, non-member of the org.
    InstAdmin,
    /// Workspace key on the test workspace (owner = WsOwner).
    WsKeySame,
    /// Workspace key on the other workspace (owner = WsOwner).
    WsKeyOther,
    /// Org key of an org admin.
    OrgKeyAdmin,
    /// Org key of an org member bound as `member`.
    OrgKeyMember,
}

const ALL: [P; 18] = [
    P::Anon,
    P::Owner,
    P::Admin,
    P::WsMember,
    P::WsApprover,
    P::WsOwner,
    P::Unbound,
    P::Suspended,
    P::Pending,
    P::GroupAdmin,
    P::GroupWsOwner,
    P::SuspendedGroupAdmin,
    P::NonMember,
    P::InstAdmin,
    P::WsKeySame,
    P::WsKeyOther,
    P::OrgKeyAdmin,
    P::OrgKeyMember,
];

impl P {
    fn is_key(self) -> bool {
        matches!(
            self,
            P::WsKeySame | P::WsKeyOther | P::OrgKeyAdmin | P::OrgKeyMember
        )
    }
    fn is_ws_key(self) -> bool {
        matches!(self, P::WsKeySame | P::WsKeyOther)
    }
}

/// Workspace access for a principal as the access law resolves it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WsAccess {
    None,
    Member,
    Approver,
    Owner,
}

fn ws_access(p: P) -> WsAccess {
    match p {
        // (FIX-234, D-477, C-1160) An org role grants no workspace right: Admin, OrgKeyAdmin
        // and GroupAdmin hold `can_administer` alone. The Owner principal is the org's
        // creator, so the creation path bound them `owner` in their own right.
        P::WsOwner | P::WsKeySame | P::GroupWsOwner | P::Owner => WsAccess::Owner,
        P::WsApprover => WsAccess::Approver,
        P::WsMember | P::OrgKeyMember => WsAccess::Member,
        _ => WsAccess::None,
    }
}

/// Org role and status for a principal's *user*: `Some((role, active))`.
fn org_role(p: P) -> Option<(&'static str, bool)> {
    match p {
        P::Owner => Some(("owner", true)),
        P::Admin | P::OrgKeyAdmin | P::GroupAdmin => Some(("admin", true)),
        P::WsMember
        | P::WsApprover
        | P::WsOwner
        | P::Unbound
        | P::OrgKeyMember
        | P::GroupWsOwner => Some(("member", true)),
        P::Suspended | P::Pending | P::SuspendedGroupAdmin => Some(("member", false)),
        _ => None,
    }
}

// ---- gates -----------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum G {
    /// Any authenticated principal (session or key).
    Auth,
    /// Instance admin only (docs/04 instance-admin rows, org creation by default).
    Inst,
    /// Active org member (any role).
    Member,
    /// Org owner or admin.
    Admin,
    /// Org owner.
    Owner,
    /// Workspace visible: `can_use`.
    WsUse,
    /// Workspace approver or owner: `can_approve` (phase 2, RFE-012); a plain member gets
    /// 403 `approver_required`.
    WsApprove,
    /// Workspace owner: `can_own`.
    WsOwn,
    /// Org-scope credential metadata (I-27 amendment): org owner/admin, or an org member who
    /// is `can_own` on a workspace of the org (so org secrets/connections can be granted).
    OrgCred,
    /// (phase 4, P4-GSL) The skill-library browse audience: an **org-level** route, so the org
    /// owner/admin rule is unchanged (FIX-234 takes no org-level right away) — an administrator
    /// browses as admin even with no own access; a plain member needs `can_use` on at least one
    /// workspace of the org and is 403 without it.
    UseAny,
    /// `can_use`, but only with an API key: sessions get 403 `forbidden` before the workspace
    /// is looked at (`POST /v1/agents/{agent}/conversations` creates `api` conversations).
    WsUseKeyOnly,
    /// (FIX-234, C-1160) `can_administer`: an org owner/admin, or an own-right workspace
    /// owner. Exactly the C-1160 route list.
    Administer,
    /// (FIX-234, C-1160) `GET /workspaces/{ws}`: own access answers in full, an administrator
    /// without own access gets the metadata.
    WsUseOrAdminister,
}

#[derive(Clone, Copy)]
struct Row {
    m: &'static str,
    p: &'static str,
    g: G,
    /// Session-only route (keys get 403 `session_required`).
    so: bool,
}

const fn r(m: &'static str, p: &'static str, g: G, so: bool) -> Row {
    Row { m, p, g, so }
}

/// Routes that are not checked against the principal matrix, with reasons.
const EXEMPT: &[(&str, &str, &str)] = &[
    (
        "GET",
        "/v1/agents/{agent}/files",
        "P5-FILES-1 (C-898/C-900): session-cookie only like the terminal (a key is 401, not 403), 404 `terminal_disabled` while the terminal is off (this harness leaves it off) and a live sandbox + runner leg for anything else; the auth matrix is it/files.rs",
    ),
    (
        "GET",
        "/v1/agents/{agent}/files/text",
        "P5-FILES-1: see GET /files",
    ),
    (
        "GET",
        "/v1/agents/{agent}/terminal/shell",
        "FIX-218 (C-1145): the terminal's authority, session-cookie only (a key is 401 `unauthenticated`, not 403), an agent of another workspace 404, a BYO agent 422 `terminal_not_supported`; the auth matrix is it/terminal.rs shell_preference_authority",
    ),
    (
        "PUT",
        "/v1/agents/{agent}/terminal/shell",
        "FIX-218: see GET /terminal/shell",
    ),
    (
        "GET",
        "/v1/agents/{agent}/files/content",
        "P5-FILES-1: see GET /files",
    ),
    (
        "PUT",
        "/v1/agents/{agent}/files/content",
        "P5-FILES-1: see GET /files",
    ),
    (
        "POST",
        "/v1/agents/{agent}/files/ops",
        "P5-FILES-1: see GET /files",
    ),
    (
        "GET",
        "/v1/me/ssh-keys",
        "P4-SSH-1 (C-790): 404 `ssh_disabled` for everyone, before authentication, while `ssh.enabled` is off (this harness leaves it off); the auth matrix (session, own keys, service key, anonymous, session-only writes) is it/ssh.rs",
    ),
    ("POST", "/v1/me/ssh-keys", "P4-SSH-1: see GET /me/ssh-keys"),
    (
        "GET",
        "/v1/me/user-tokens",
        "P4-UTOK (C-853): a person's own tokens only (another person's is 404 on every route); writes are session-only; the matrix of who may do what is it/user_tokens.rs",
    ),
    (
        "POST",
        "/v1/me/user-tokens",
        "P4-UTOK: see GET /me/user-tokens",
    ),
    (
        "PATCH",
        "/v1/me/user-tokens/{}",
        "P4-UTOK: see GET /me/user-tokens",
    ),
    (
        "DELETE",
        "/v1/me/user-tokens/{}",
        "P4-UTOK: see GET /me/user-tokens",
    ),
    (
        "PUT",
        "/v1/me/user-tokens/{}/agents/{}",
        "P4-UTOK: see GET /me/user-tokens",
    ),
    (
        "DELETE",
        "/v1/me/user-tokens/{}/agents/{}",
        "P4-UTOK: see GET /me/user-tokens",
    ),
    (
        "GET",
        "/v1/me/user-token-approvals",
        "P4-UTOK (C-856): the caller's own proposed posts with their text (session only); nobody else's rows, it/user_tokens.rs only_the_owner_decides…",
    ),
    (
        "PATCH",
        "/v1/me/ssh-keys/{id}",
        "P4-SSH-1: see GET /me/ssh-keys",
    ),
    (
        "DELETE",
        "/v1/me/ssh-keys/{id}",
        "P4-SSH-1: see GET /me/ssh-keys",
    ),
    (
        "POST",
        "/v1/agents/{agent}/ssh-certificate",
        "P4-SSH-1 (C-791, SEC-93): see GET /me/ssh-keys; the mint's own matrix (session, auk inside/outside its workspace, auo, service key, agent token, anonymous, non-can_use, BYO, frozen, unregistered key) is it/ssh.rs ssh_cert_auth_matrix",
    ),
    ("GET", "/v1/ssh", "P4-SSH-1 (C-792): see GET /me/ssh-keys"),
    (
        "GET",
        "/v1/push/config",
        "P4-011B-3 (C-830): 404 `push_unavailable` for everyone, before authentication, while `push.enabled` is off (this harness leaves it off); the auth matrix (session and key allowed, anonymous 401) is it/web_push.rs routes_validate_limit_and_stay_session_only",
    ),
    (
        "GET",
        "/v1/me/push-subscriptions",
        "P4-011B-3 (C-831): see GET /push/config; session only (a key is 403), own rows only (another person's id is 404): it/web_push.rs",
    ),
    (
        "POST",
        "/v1/me/push-subscriptions",
        "P4-011B-3: see GET /me/push-subscriptions",
    ),
    (
        "PATCH",
        "/v1/me/push-subscriptions/{id}",
        "P4-011B-3: see GET /me/push-subscriptions",
    ),
    (
        "DELETE",
        "/v1/me/push-subscriptions/{id}",
        "P4-011B-3: see GET /me/push-subscriptions",
    ),
    (
        "POST",
        "/v1/me/push-subscriptions/{id}/test",
        "P4-011B-3: see GET /me/push-subscriptions; rate limit it/web_push.rs the_test_route_sends_the_test_payload_and_is_rate_limited",
    ),
    (
        "GET",
        "/v1/me/ssh-sessions",
        "P4-SSH-2 (C-793): 404 `ssh_disabled` for everyone while `ssh.enabled` is off (this harness leaves it off); the authority matrix (own list, `can_own` all with the person, `can_use` own only, stranger 404, disconnect by person or owner) is it/ssh_sessions.rs ssh_sessions_lists_and_disconnect_authority",
    ),
    (
        "GET",
        "/v1/agents/{agent}/ssh-sessions",
        "P4-SSH-2: see GET /me/ssh-sessions",
    ),
    (
        "DELETE",
        "/v1/ssh-sessions/{id}",
        "P4-SSH-2: see GET /me/ssh-sessions",
    ),
    (
        "GET",
        "/v1/admin/terminators",
        "P4-SSH-2 (C-793): instance admin only, 404 `ssh_disabled` while off; it/ssh_sessions.rs ssh_sessions_terminator_facts_and_admin_list",
    ),
    (
        "POST",
        "/v1/telemetry/traces",
        "FIX-146 D (C-732): any session may post browser spans, but the route is 404 for everyone \
         unless `telemetry.otlp_endpoint` resolves (resolved before auth), which this harness does \
         not set; 404/401/CSRF/API-key refusal and the sanitizer are in telemetry_ingest.rs",
    ),
    (
        "POST",
        "/v1/agents/{agent}/space-bindings",
        "FIX-127 (C-713): can_own of the agent's workspace, but the body names an integration and \
         an unseeded one is the specified 404 `integration_not_found`, which this matrix reads as \
         a denial; the denial side (member 403, outsider 404) is in channels_mgmt.rs",
    ),
    (
        "GET",
        "/v1/integrations/{id}/spaces",
        "FIX-127 (C-714): authorization resolves through the integration's org and the caller's \
         linked chat account (can_own in the org, 403 otherwise); covered by channels_mgmt.rs",
    ),
    (
        "DELETE",
        "/v1/scim/tokens/{id}",
        "authorization resolves through the token's org (owner only); an unknown id is an \
         idempotent 204 — covered by scim_auth.rs (FIX-081 lists the route)",
    ),
    (
        "GET",
        "/v1/public/avatars/{key}",
        "public capability: unguessable current avatar key in path (05 auth class P)",
    ),
    (
        "POST",
        "/v1/hooks/{token}",
        "public capability: token in path (05 auth class P)",
    ),
    (
        "GET",
        "/v1/instance",
        "public: instance bootstrap info (login page), served to anonymous callers",
    ),
    (
        "GET",
        "/v1/auth/oidc/{provider}/start",
        "public: OIDC sign-in redirect (the only sign-in; `reauth=1` needs a live session)",
    ),
    (
        "GET",
        "/v1/auth/oidc/{provider}/callback",
        "public: OIDC callback (state-based)",
    ),
    (
        "POST",
        "/v1/auth/cli",
        "public: CLI device flow start (pre-auth)",
    ),
    (
        "POST",
        "/v1/auth/cli/token",
        "public: CLI device flow poll (device_code)",
    ),
    (
        "GET",
        "/v1/invitations/preview",
        "public: token-based invitation preview",
    ),
    (
        "GET",
        "/v1/gateway/ca.pem",
        "public: CA certificate download",
    ),
    (
        "GET",
        "/v1/oauth/callback",
        "public: OAuth provider callback (state-based)",
    ),
    (
        "GET",
        "/v1/agent/container-config",
        "runner/agent token auth, not a user principal",
    ),
    (
        "POST",
        "/v1/agent/context-credentials",
        "runner/agent token auth, not a user principal",
    ),
    (
        "GET",
        "/v1/agents/{agent}/container-config",
        "runner/agent token auth, not a user principal",
    ),
    (
        "POST",
        "/v1/agents/{agent}/token",
        "runner/agent token minting, not a user principal",
    ),
    // (P4-GRP-1) Routes addressed by a group's own id: a group of another org or an org the
    // caller is not in answers 404 for everyone, so a seeded group is needed to see the matrix.
    // `api/it/groups.rs::permission_matrix_per_route` runs every principal against a real one.
    (
        "GET",
        "/v1/groups/{id}",
        "by group id; matrix proven in groups.rs with a seeded group",
    ),
    (
        "PATCH",
        "/v1/groups/{id}",
        "by group id; matrix proven in groups.rs with a seeded group",
    ),
    (
        "DELETE",
        "/v1/groups/{id}",
        "by group id; matrix proven in groups.rs with a seeded group",
    ),
    (
        "GET",
        "/v1/groups/{id}/members",
        "by group id; matrix proven in groups.rs with a seeded group",
    ),
    (
        "PUT",
        "/v1/groups/{id}/members/{user}",
        "by group id; matrix proven in groups.rs with a seeded group",
    ),
    (
        "DELETE",
        "/v1/groups/{id}/members/{user}",
        "by group id; matrix proven in groups.rs with a seeded group",
    ),
    (
        "POST",
        "/v1/groups/{id}/members:batch",
        "by group id; matrix proven in groups.rs with a seeded group",
    ),
    (
        "GET",
        "/v1/groups/{id}/workspaces",
        "by group id; matrix proven in groups.rs with a seeded group",
    ),
    (
        "GET",
        "/v1/groups/{id}/impact",
        "by group id; matrix proven in groups.rs with a seeded group",
    ),
    // (P4-PRV-1, D-144, SEC-80) Preview share routes: while the feature switch is off (the
    // harness default) every route answers 404 `previews_disabled` for every principal, and the
    // share-addressed routes need a seeded share id. The authority law (`can_use`, never an
    // agent credential) is proven per route in `api/it/previews.rs`.
    (
        "GET",
        "/v1/agents/{agent}/previews",
        "feature-switched; authority proven in previews.rs (can_use, SEC-80)",
    ),
    (
        "POST",
        "/v1/agents/{agent}/previews",
        "feature-switched; authority proven in previews.rs (can_use, SEC-80)",
    ),
    (
        "GET",
        "/v1/previews/{id}",
        "feature-switched, by share id; proven in previews.rs with a seeded share",
    ),
    (
        "PATCH",
        "/v1/previews/{id}",
        "feature-switched, by share id; proven in previews.rs with a seeded share",
    ),
    (
        "DELETE",
        "/v1/previews/{id}",
        "feature-switched, by share id; proven in previews.rs with a seeded share",
    ),
    (
        "POST",
        "/v1/previews/{id}/start",
        "feature-switched, by share id; proven in previews.rs with a seeded share",
    ),
];

/// Route table. `p` is the OpenAPI path template (placeholder names are normalized).
const ROWS: &[Row] = &[
    // ---- instance admin ----
    // (phase 2, RFE-006) operator kill-switch for organizations.
    r("GET", "/v1/admin/orgs", G::Inst, false),
    // (phase 5, P5-ADMDEL, C-733) metadata and deletes; the deletes need a recent re-auth.
    r("GET", "/v1/admin/orgs/{org}", G::Inst, false),
    r("GET", "/v1/admin/orgs/{org}/workspaces", G::Inst, false),
    r("DELETE", "/v1/admin/orgs/{org}", G::Inst, false),
    r("DELETE", "/v1/admin/workspaces/{ws}", G::Inst, false),
    r("POST", "/v1/admin/orgs/{org}/freeze", G::Inst, false),
    r("POST", "/v1/admin/orgs/{org}/unfreeze", G::Inst, false),
    r("GET", "/v1/admin/host-claims", G::Inst, false),
    r("PUT", "/v1/admin/host-claims/{group_key}", G::Inst, false),
    // (P5-SKLIB-2, C-1128, SEC-149) instance skill libraries: instance admins only, never an org owner
    // or an API key
    r("GET", "/v1/admin/skill-libraries", G::Inst, false),
    r("POST", "/v1/admin/skill-libraries", G::Inst, false),
    r("POST", "/v1/admin/skill-libraries/check", G::Inst, false),
    r("GET", "/v1/admin/app-instances", G::Inst, false),
    r("POST", "/v1/admin/app-instances", G::Inst, false),
    r(
        "POST",
        "/v1/admin/app-instances/{id}/promote",
        G::Inst,
        false,
    ),
    r("GET", "/v1/admin/oauth-clients", G::Inst, false),
    r("POST", "/v1/admin/oauth-clients", G::Inst, false),
    r("GET", "/v1/admin/plugins", G::Inst, false),
    r("POST", "/v1/admin/plugins", G::Inst, false),
    r("GET", "/v1/admin/plugins/{id}", G::Inst, false),
    r(
        "GET",
        "/v1/admin/plugins/{id}/apps/{app_id}/skills/{name}/file",
        G::Inst,
        false,
    ),
    r(
        "GET",
        "/v1/admin/plugins/{id}/apps/{app_id}/operations",
        G::Inst,
        false,
    ),
    r("DELETE", "/v1/admin/plugins/{id}", G::Inst, false),
    r("POST", "/v1/admin/plugins/{id}/approve", G::Inst, false),
    r("POST", "/v1/admin/plugins/{id}/disable", G::Inst, false),
    r("POST", "/v1/admin/plugins/{id}/enable", G::Inst, false),
    r("POST", "/v1/admin/plugins/{id}/reject", G::Inst, false),
    r("GET", "/v1/admin/registry/snapshot", G::Inst, false),
    r("GET", "/v1/admin/relays", G::Inst, false),
    r("GET", "/v1/admin/runners", G::Inst, false),
    r("POST", "/v1/admin/runners", G::Inst, false),
    r("DELETE", "/v1/admin/runners/{id}", G::Inst, false),
    r("POST", "/v1/admin/runners/{id}/token", G::Inst, false),
    r(
        "GET",
        "/v1/admin/runners/{id}/start-failures",
        G::Inst,
        false,
    ),
    r(
        "GET",
        "/v1/admin/agents/{agent}/start-failures",
        G::Inst,
        false,
    ),
    r("GET", "/v1/admin/connectors", G::Inst, false),
    r("POST", "/v1/admin/connectors", G::Inst, false),
    r("DELETE", "/v1/admin/connectors/{id}", G::Inst, false),
    r("PATCH", "/v1/admin/connectors/{id}", G::Inst, false),
    r("GET", "/v1/admin/connector-types", G::Inst, false),
    r("GET", "/v1/admin/overview", G::Inst, false),
    // (FIX-167 Part B, C-1022) what needs an instance admin's attention
    r("GET", "/v1/admin/attention", G::Inst, false),
    r("POST", "/v1/admin/connectors/{id}/approve", G::Inst, false),
    r("POST", "/v1/admin/connectors/{id}/token", G::Inst, false),
    r("GET", "/v1/admin/connector-host-tokens", G::Inst, false),
    r("POST", "/v1/admin/connector-host-tokens", G::Inst, false),
    r(
        "DELETE",
        "/v1/admin/connector-host-tokens/{id}",
        G::Inst,
        false,
    ),
    r("GET", "/v1/admin/trusted-keys", G::Inst, false),
    r("POST", "/v1/admin/trusted-keys", G::Inst, false),
    r("DELETE", "/v1/admin/trusted-keys/{id}", G::Inst, false),
    // (P5-ASRC, C-590..C-602) addon sources: instance administrators only
    r("GET", "/v1/admin/addon-sources", G::Inst, false),
    r("POST", "/v1/admin/addon-sources", G::Inst, false),
    r("POST", "/v1/admin/addon-sources/preview", G::Inst, false),
    r("POST", "/v1/admin/addon-sources/check-all", G::Inst, false),
    r("GET", "/v1/admin/addon-sources/{id}", G::Inst, false),
    r("PATCH", "/v1/admin/addon-sources/{id}", G::Inst, false),
    r("DELETE", "/v1/admin/addon-sources/{id}", G::Inst, false),
    r("POST", "/v1/admin/addon-sources/{id}/check", G::Inst, false),
    r(
        "POST",
        "/v1/admin/addon-sources/{id}/confirm-key",
        G::Inst,
        false,
    ),
    // (P6-TPL-2, C-1107/C-1108) agent template sources and review: instance administrators
    // only, session only (`InstanceAdmin` is a `SessionCaller`; an API key is 403
    // `session_required` like the settings routes)
    r("GET", "/v1/admin/agent-template-sources", G::Inst, true),
    r("POST", "/v1/admin/agent-template-sources", G::Inst, true),
    r(
        "POST",
        "/v1/admin/agent-template-sources/preview",
        G::Inst,
        true,
    ),
    r(
        "POST",
        "/v1/admin/agent-template-sources/check-all",
        G::Inst,
        true,
    ),
    r(
        "GET",
        "/v1/admin/agent-template-sources/{id}",
        G::Inst,
        true,
    ),
    r(
        "PATCH",
        "/v1/admin/agent-template-sources/{id}",
        G::Inst,
        true,
    ),
    r(
        "DELETE",
        "/v1/admin/agent-template-sources/{id}",
        G::Inst,
        true,
    ),
    r(
        "POST",
        "/v1/admin/agent-template-sources/{id}/check",
        G::Inst,
        true,
    ),
    r("GET", "/v1/admin/agent-templates", G::Inst, true),
    r(
        "GET",
        "/v1/admin/agent-templates/{template_id}/versions/{version}",
        G::Inst,
        true,
    ),
    r(
        "GET",
        "/v1/admin/agent-templates/{template_id}/versions/{version}/avatar",
        G::Inst,
        true,
    ),
    r(
        "POST",
        "/v1/admin/agent-templates/{template_id}/versions/{version}/approve",
        G::Inst,
        true,
    ),
    r(
        "POST",
        "/v1/admin/agent-templates/{template_id}/versions/{version}/reject",
        G::Inst,
        true,
    ),
    r(
        "PATCH",
        "/v1/admin/agent-templates/{template_id}",
        G::Inst,
        true,
    ),
    r("GET", "/v1/admin/audit", G::Inst, false),
    r("GET", "/v1/admin/usage", G::Inst, false),
    r("GET", "/v1/admin/users", G::Inst, false),
    r("PATCH", "/v1/admin/users/{id}", G::Inst, false),
    r("GET", "/v1/admin/storage", G::Inst, false),
    r("POST", "/v1/admin/storage/refresh", G::Inst, false),
    // (P4-IDP-1, C-511) read-only role mapping table
    // (P4-GRP-1, C-460..C-469, SEC-71) directory groups: the org-addressed and the
    // workspace-addressed routes; the by-group-id routes need a seeded group and are proven in
    // `groups.rs` (see EXEMPT).
    r("GET", "/v1/orgs/{org}/groups", G::OrgCred, false),
    r("POST", "/v1/orgs/{org}/groups", G::Admin, false),
    r(
        "PUT",
        "/v1/workspaces/{ws}/access/groups/{group}",
        G::Administer,
        false,
    ),
    r(
        "DELETE",
        "/v1/workspaces/{ws}/access/groups/{group}",
        G::Administer,
        false,
    ),
    // (P4-ONB-1, C-520..C-522, SEC-79) new-user placement
    r("GET", "/v1/admin/placement", G::Inst, false),
    r("PUT", "/v1/admin/placement", G::Inst, false),
    r("GET", "/v1/admin/placement/preview", G::Inst, false),
    r("GET", "/v1/admin/placement/choices", G::Inst, false),
    // (P5-UX-6, C-922) give the starter grants to people who signed in before
    r(
        "POST",
        "/v1/admin/placement/starter-grants/backfill",
        G::Inst,
        false,
    ),
    // (phase 5, SET-1) instance settings and (FIX-124) secret stores: instance administrators only; session-only (an API key gets 403 `session_required`)
    r("GET", "/v1/admin/settings", G::Inst, true),
    r("GET", "/v1/admin/settings/export", G::Inst, true),
    r("POST", "/v1/admin/settings/import", G::Inst, true),
    r("GET", "/v1/admin/settings/governance/orgs", G::Inst, true),
    r("GET", "/v1/admin/settings/{section}", G::Inst, true),
    r("PUT", "/v1/admin/settings/{section}", G::Inst, true),
    r(
        "POST",
        "/v1/admin/settings/{section}/preview",
        G::Inst,
        true,
    ),
    r(
        "POST",
        "/v1/admin/settings/llm/endpoint-test",
        G::Inst,
        true,
    ),
    r(
        "POST",
        "/v1/admin/settings/sign_in/provider-test",
        G::Inst,
        true,
    ),
    r("POST", "/v1/admin/settings/email/test", G::Inst, true),
    r("GET", "/v1/admin/llm-endpoint-warnings", G::Inst, false),
    r("GET", "/v1/admin/secret-stores", G::Inst, true),
    r("PUT", "/v1/admin/secret-stores/{id}", G::Inst, true),
    r("DELETE", "/v1/admin/secret-stores/{id}", G::Inst, true),
    r(
        "PUT",
        "/v1/admin/secret-stores/{id}/reference-scopes",
        G::Inst,
        true,
    ),
    r("PUT", "/v1/admin/secret-store-default", G::Inst, true),
    r("POST", "/v1/admin/secret-store-test", G::Inst, true),
    r("POST", "/v1/admin/secret-stores/migrations", G::Inst, true),
    r(
        "GET",
        "/v1/admin/secret-stores/migrations/current",
        G::Inst,
        true,
    ),
    // (P4-ONB-2, C-533) setup wizard status
    r("GET", "/v1/admin/setup", G::Inst, false),
    r("POST", "/v1/orgs", G::Inst, false),
    // ---- any authenticated principal ----
    // (P4-ONB-2, C-530) the caller's own onboarding progress
    r("GET", "/v1/me/onboarding", G::Auth, false),
    r("PATCH", "/v1/me/onboarding", G::Auth, false),
    // I-27: instance governance is config without secrets, readable by any signed-in user
    r("GET", "/v1/governance/instance", G::Auth, false),
    r("GET", "/v1/me", G::Auth, false),
    // (P5-REFINE-6, C-1089) the chat platforms a person can link.
    r("GET", "/v1/me/chat-platforms", G::Auth, false),
    r("PATCH", "/v1/me", G::Auth, false),
    r("DELETE", "/v1/me", G::Auth, true),
    r("GET", "/v1/me/api-keys", G::Auth, false),
    r("POST", "/v1/auth/logout", G::Auth, false),
    r("POST", "/v1/auth/sessions/revoke-others", G::Auth, true),
    r("POST", "/v1/auth/cli/approve", G::Auth, true),
    r("GET", "/v1/auth/cli/pending", G::Auth, true),
    r("POST", "/v1/invitations/accept", G::Auth, true),
    r("GET", "/v1/orgs", G::Auth, false),
    r("GET", "/v1/resolve", G::Auth, false),
    r("GET", "/v1/secret-stores", G::Auth, false),
    r("GET", "/v1/llm/providers", G::Auth, false),
    r("GET", "/v1/harnesses", G::Auth, false),
    r("GET", "/v1/apps", G::Auth, false),
    r("GET", "/v1/apps/{app}", G::Auth, false),
    r("GET", "/v1/apps/{app}/guide", G::Auth, false),
    r("GET", "/v1/apps/{app}/icon", G::Auth, false),
    // (P5-UX-2, C-931, C-955) Fired with `?workspace_id=` (the workspace view: `can_own`); the
    // organization view (owner/admin) is covered by it/app_setup.rs.
    r("GET", "/v1/apps/{app}/setup", G::WsOwn, false),
    r("GET", "/v1/apps/{app}/connect-plan", G::WsOwn, false),
    // (P5-POLISH-4, C-1030, SEC-135) Ask an admin to set an app up: workspace owners ask, anyone with the
    // workspace may withdraw their own ask, owners and admins read and decline.
    r(
        "POST",
        "/v1/workspaces/{ws}/apps/{app}/setup-request",
        G::WsOwn,
        false,
    ),
    r(
        "DELETE",
        "/v1/workspaces/{ws}/apps/{app}/setup-request",
        G::WsUse,
        false,
    ),
    // (P5-APIGAP-2, C-1047) The same with a subject: any member of the workspace asks, lists their own, withdraws.
    r("POST", "/v1/workspaces/{ws}/requests", G::WsUse, false),
    r("GET", "/v1/workspaces/{ws}/requests", G::WsUse, false),
    r(
        "DELETE",
        "/v1/workspaces/{ws}/requests/{id}",
        G::WsUse,
        false,
    ),
    r("GET", "/v1/orgs/{org}/app-setup-requests", G::Admin, false),
    r(
        "POST",
        "/v1/orgs/{org}/app-setup-requests/{id}/decline",
        G::Admin,
        false,
    ),
    // (P5-REFINE-8, C-1080..C-1082) One attention feed per level, the rail counts, and Administration's
    // requests.
    r("GET", "/v1/workspaces/{ws}/attention", G::WsUse, false),
    r("GET", "/v1/orgs/{org}/attention", G::Admin, false),
    r("GET", "/v1/attention/counts", G::Auth, false),
    r("GET", "/v1/admin/requests", G::Inst, false),
    r("POST", "/v1/admin/requests/{id}/decline", G::Inst, false),
    r("GET", "/v1/connectors", G::Auth, false),
    r("GET", "/v1/connectors/{id}/icon", G::Auth, false),
    r("POST", "/v1/apps/{app}/connect", G::Auth, false),
    r("GET", "/v1/approvals/pending", G::Auth, false),
    r("GET", "/v1/approvals/stream", G::Auth, false),
    r("DELETE", "/v1/api-keys/{id}", G::Auth, false),
    // (FIX-140, C-720) Revoke the presenting key; a session gets 400 inside the handler.
    r("DELETE", "/v1/me/api-key", G::Auth, false),
    // ---- org-level ----
    r("GET", "/v1/orgs/{org}", G::Member, false),
    r("PATCH", "/v1/orgs/{org}", G::Owner, false),
    r("DELETE", "/v1/orgs/{org}", G::Owner, false),
    // (FIX-081, C-471/C-472) Directory-sync management: org owner only, session only.
    r("GET", "/v1/orgs/{org}/scim", G::Owner, true),
    r("PUT", "/v1/orgs/{org}/scim", G::Owner, true),
    r("GET", "/v1/orgs/{org}/scim/status", G::Owner, true),
    r("GET", "/v1/orgs/{org}/scim/tokens", G::Owner, true),
    r("POST", "/v1/orgs/{org}/scim/tokens", G::Owner, true),
    // (phase 2, RFE-006) freeze / unfreeze: org owner; workspace and agent: `can_own`.
    r("POST", "/v1/orgs/{org}/freeze", G::Owner, false),
    r("POST", "/v1/orgs/{org}/unfreeze", G::Owner, false),
    r("POST", "/v1/workspaces/{ws}/freeze", G::Administer, false),
    r("POST", "/v1/workspaces/{ws}/unfreeze", G::Administer, false),
    r("POST", "/v1/agents/{agent}/freeze", G::Administer, false),
    r("POST", "/v1/agents/{agent}/unfreeze", G::Administer, false),
    r(
        "GET",
        "/v1/orgs/{org}/governance/effective",
        G::Member,
        false,
    ),
    r("GET", "/v1/orgs/{org}/workspaces", G::Member, false),
    r("POST", "/v1/orgs/{org}/workspaces", G::Member, false),
    r("GET", "/v1/orgs/{org}/workspace-overrides", G::Admin, false),
    r("GET", "/v1/orgs/{org}/api-keys", G::Admin, false),
    r("POST", "/v1/orgs/{org}/api-keys", G::Admin, true),
    r("GET", "/v1/orgs/{org}/app-availability", G::Admin, false),
    r("PUT", "/v1/orgs/{org}/app-availability", G::Admin, false),
    r("GET", "/v1/orgs/{org}/audit", G::Admin, false),
    r("GET", "/v1/orgs/{org}/connections", G::OrgCred, false),
    // (P5-UX-4, C-917) the organization's accounts list: owners and admins.
    r("GET", "/v1/orgs/{org}/accounts", G::Admin, false),
    r("GET", "/v1/orgs/{org}/governance", G::Admin, false),
    r("PUT", "/v1/orgs/{org}/governance", G::Admin, false),
    r("GET", "/v1/orgs/{org}/invitations", G::Admin, false),
    r("POST", "/v1/orgs/{org}/invitations", G::Admin, false),
    r("DELETE", "/v1/invitations/{id}", G::Admin, false),
    // (P5-REFINE-6, C-1069, SEC-140) re-issue the link / resend: owners and admins of the
    // invitation's organization (the full matrix is it/invitations.rs sec140_*).
    r("POST", "/v1/invitations/{id}/link", G::Admin, false),
    r("POST", "/v1/invitations/{id}/resend", G::Admin, false),
    // (P5-REFINE-6, C-1090) the organization's agents: owners and admins.
    r("GET", "/v1/orgs/{org}/agents", G::Admin, false),
    r("GET", "/v1/orgs/{org}/members", G::Admin, false),
    r("PATCH", "/v1/orgs/{org}/members/{user}", G::Admin, false),
    r("DELETE", "/v1/orgs/{org}/members/{user}", G::Admin, false),
    r(
        "GET",
        "/v1/orgs/{org}/members/{user}/offboarding",
        G::Admin,
        false,
    ),
    // (phase 2, D-74) self-hosted app instances: any member lists, owners/admins register
    r("GET", "/v1/orgs/{org}/app-instances", G::Member, false),
    r("POST", "/v1/orgs/{org}/app-instances", G::Admin, false),
    r("PATCH", "/v1/app-instances/{id}", G::Admin, false),
    r("DELETE", "/v1/app-instances/{id}", G::Admin, false),
    r("GET", "/v1/orgs/{org}/oauth-clients", G::Admin, false),
    r("POST", "/v1/orgs/{org}/oauth-clients", G::Admin, false),
    r("PATCH", "/v1/oauth-clients/{id}", G::Admin, false),
    r("DELETE", "/v1/oauth-clients/{id}", G::Admin, false),
    // (P5-REFINE-6, C-1070) Test a sign-in app: the callers of PATCH.
    r("POST", "/v1/oauth-clients/{id}/test", G::Admin, false),
    r("GET", "/v1/orgs/{org}/policy", G::Admin, false),
    r("PUT", "/v1/orgs/{org}/policy/draft", G::Admin, false),
    r("GET", "/v1/orgs/{org}/policy/generations", G::Admin, false),
    r("POST", "/v1/orgs/{org}/policy/publish", G::Admin, false),
    r("POST", "/v1/orgs/{org}/policy/rollback", G::Admin, false),
    r("GET", "/v1/orgs/{org}/policy/warnings", G::Admin, false),
    r("GET", "/v1/orgs/{org}/secrets", G::OrgCred, false),
    r("POST", "/v1/orgs/{org}/secrets", G::Admin, false),
    // ---- workspace-level ----
    r("GET", "/v1/workspaces/{ws}", G::WsUseOrAdminister, false),
    r("PATCH", "/v1/workspaces/{ws}", G::WsOwn, false),
    r("DELETE", "/v1/workspaces/{ws}", G::Administer, false),
    r("GET", "/v1/workspaces/{ws}/access", G::Administer, false),
    // (P5-UX-5, C-919, C-920, SEC-128; FIX-234 C-1160) Adding people by e-mail:
    // `can_administer`.
    r(
        "POST",
        "/v1/workspaces/{ws}/access/by-email",
        G::Administer,
        false,
    ),
    r(
        "GET",
        "/v1/workspaces/{ws}/access/pending",
        G::Administer,
        false,
    ),
    r(
        "DELETE",
        "/v1/workspaces/{ws}/access/pending/{id}",
        G::Administer,
        false,
    ),
    r(
        "PUT",
        "/v1/workspaces/{ws}/access/{user}",
        G::Administer,
        false,
    ),
    // (P5-APIGAP-4, C-1051) suspend/restore; the body restores (a no-op on the active victim).
    r(
        "PATCH",
        "/v1/workspaces/{ws}/access/{user}",
        G::Administer,
        false,
    ),
    r(
        "DELETE",
        "/v1/workspaces/{ws}/access/{user}",
        G::Administer,
        false,
    ),
    r("GET", "/v1/workspaces/{ws}/activity", G::WsUse, false),
    r("GET", "/v1/workspaces/{ws}/usage", G::Administer, false),
    // (P5-CHAT-4, C-727) message search: workspace member; visibility is applied inside the query.
    r(
        "GET",
        "/v1/workspaces/{ws}/messages/search",
        G::WsUse,
        false,
    ),
    r(
        "GET",
        "/v1/workspaces/{ws}/members/{user}/offboarding",
        G::Administer,
        false,
    ),
    r(
        "POST",
        "/v1/workspaces/{ws}/automations/reassign",
        G::Administer,
        false,
    ),
    r("GET", "/v1/workspaces/{ws}/agents", G::WsUse, false),
    r("POST", "/v1/workspaces/{ws}/agents", G::WsUse, false),
    // (phase 3, RFE-013, C-313) agent-token lists: workspace owner / org owner or admin.
    r(
        "GET",
        "/v1/workspaces/{ws}/agent-tokens",
        G::Administer,
        false,
    ),
    r("GET", "/v1/orgs/{org}/agent-tokens", G::Admin, false),
    r("GET", "/v1/workspaces/{ws}/api-keys", G::WsOwn, false),
    r("POST", "/v1/workspaces/{ws}/api-keys", G::WsOwn, true),
    r("GET", "/v1/workspaces/{ws}/approvals", G::WsApprove, false),
    r("GET", "/v1/workspaces/{ws}/audit", G::WsOwn, false),
    r("GET", "/v1/workspaces/{ws}/connections", G::WsOwn, false),
    // (P5-UX-4, C-917) the workspace's accounts list: every member sees it.
    r("GET", "/v1/workspaces/{ws}/accounts", G::WsUse, false),
    r("GET", "/v1/workspaces/{ws}/oauth-clients", G::WsOwn, false),
    r("POST", "/v1/workspaces/{ws}/oauth-clients", G::WsOwn, false),
    r("GET", "/v1/workspaces/{ws}/policy", G::WsOwn, false),
    // (FIX-164, D-365) The published rules, read-only, for any member of the workspace.
    r(
        "GET",
        "/v1/workspaces/{ws}/internet-access",
        G::WsUse,
        false,
    ),
    r("PUT", "/v1/workspaces/{ws}/policy/draft", G::WsOwn, false),
    r(
        "GET",
        "/v1/workspaces/{ws}/policy/generations",
        G::WsOwn,
        false,
    ),
    r(
        "POST",
        "/v1/workspaces/{ws}/policy/publish",
        G::WsOwn,
        false,
    ),
    r(
        "POST",
        "/v1/workspaces/{ws}/policy/rollback",
        G::WsOwn,
        false,
    ),
    r(
        "POST",
        "/v1/workspaces/{ws}/policy/simulate",
        G::WsOwn,
        false,
    ),
    // (P4-011B-2, C-829) The live approval limits: any signed-in caller.
    r("GET", "/v1/policy/approval-limits", G::Auth, false),
    r(
        "GET",
        "/v1/workspaces/{ws}/policy/warnings",
        G::WsOwn,
        false,
    ),
    r("GET", "/v1/workspaces/{ws}/secrets", G::WsOwn, false),
    r("POST", "/v1/workspaces/{ws}/secrets", G::WsOwn, false),
    // ---- agents (agent id resolves to its workspace) ----
    r("GET", "/v1/agents/{agent}", G::WsUse, false),
    r("GET", "/v1/agents/{agent}/avatar", G::WsUse, false),
    r("PUT", "/v1/agents/{agent}/avatar", G::WsUse, false),
    r("DELETE", "/v1/agents/{agent}/avatar", G::WsUse, false),
    r("PATCH", "/v1/agents/{agent}", G::WsUse, false),
    // (phase 2, RFE-025) the agent's own source list / require-context flag: `can_own`.
    r("PUT", "/v1/agents/{agent}/byo", G::WsOwn, false),
    r("DELETE", "/v1/agents/{agent}", G::WsUse, false),
    r("GET", "/v1/agents/{agent}/conversations", G::WsUse, false),
    r(
        "POST",
        "/v1/agents/{agent}/conversations",
        G::WsUseKeyOnly,
        false,
    ),
    r(
        "POST",
        "/v1/agents/{agent}/conversations/direct",
        G::WsUse,
        true,
    ),
    // (P5-REFINE-7, C-1073) a fresh direct chat: session only, like `direct`.
    r(
        "POST",
        "/v1/agents/{agent}/conversations/direct/new",
        G::WsUse,
        true,
    ),
    // (P5-REFINE-7, C-1074) the automation folders of the conversation list.
    r(
        "GET",
        "/v1/agents/{agent}/conversation-folders",
        G::WsUse,
        false,
    ),
    // (P5-REFINE-7, C-1077) the schedule preview saves nothing.
    r(
        "POST",
        "/v1/agents/{agent}/schedules/preview",
        G::WsUse,
        false,
    ),
    r("GET", "/v1/agents/{agent}/effective", G::WsUse, false),
    // (P5-UX-10, C-934) what exactly an agent can do in an app
    r(
        "GET",
        "/v1/agents/{agent}/apps/{app}/access",
        G::WsUse,
        false,
    ),
    r("GET", "/v1/agents/{agent}/schedules", G::WsUse, false),
    r("POST", "/v1/agents/{agent}/schedules", G::WsUse, false),
    r("GET", "/v1/agents/{agent}/webhooks", G::WsUse, false),
    r("POST", "/v1/agents/{agent}/webhooks", G::WsUse, false),
    r("DELETE", "/v1/schedules/{id}", G::WsUse, false),
    r("GET", "/v1/schedules/{id}", G::WsUse, false),
    r("PATCH", "/v1/schedules/{id}", G::WsUse, false),
    r("PUT", "/v1/schedules/{id}/creator", G::Administer, false),
    r("POST", "/v1/schedules/{id}/run-now", G::WsUse, false),
    r("GET", "/v1/schedules/{id}/runs", G::WsUse, false),
    r("DELETE", "/v1/webhooks/{id}", G::WsUse, false),
    r("GET", "/v1/webhooks/{id}", G::WsUse, false),
    r("PATCH", "/v1/webhooks/{id}", G::WsUse, false),
    r("GET", "/v1/webhooks/{id}/deliveries", G::WsUse, false),
    r("PUT", "/v1/webhooks/{id}/creator", G::Administer, false),
    r("POST", "/v1/webhooks/{id}/rotate", G::WsUse, false),
    // (P5-REFINE-7, C-1078) same callers as `rotate`.
    r("POST", "/v1/webhooks/{id}/rotate-secret", G::WsUse, false),
    r("GET", "/v1/agents/{agent}/memories", G::WsUse, false),
    r("POST", "/v1/agents/{agent}/memories", G::WsUse, false),
    r("GET", "/v1/agents/{agent}/memories/search", G::WsUse, false),
    r("GET", "/v1/agents/{agent}/memories/{id}", G::WsUse, false),
    r("PUT", "/v1/agents/{agent}/memories/{id}", G::WsUse, false),
    // (P6-MEMLOCK, C-1148) The lock route has the same workspace access as the others.
    r(
        "PUT",
        "/v1/agents/{agent}/memories/{id}/locked",
        G::WsUse,
        false,
    ),
    r(
        "DELETE",
        "/v1/agents/{agent}/memories/{id}",
        G::WsUse,
        false,
    ),
    r(
        "POST",
        "/v1/agents/{agent}/memories/{id}/restore",
        G::WsUse,
        false,
    ),
    r(
        "GET",
        "/v1/agents/{agent}/memories/{id}/revisions",
        G::WsUse,
        false,
    ),
    r(
        "POST",
        "/v1/agents/{agent}/memories/{id}/revisions/{rev}/redact",
        G::WsUse,
        false,
    ),
    // (P6-TPL-6, C-1143) The template helper files: list, read and remove; the same workspace
    // access as the agent's other read/write routes.
    r("GET", "/v1/agents/{agent}/helpers", G::WsUse, false),
    r("GET", "/v1/agents/{agent}/helpers/content", G::WsUse, false),
    r("DELETE", "/v1/agents/{agent}/helpers", G::WsUse, false),
    r("GET", "/v1/agents/{agent}/skills", G::WsUse, false),
    r("POST", "/v1/agents/{agent}/skills", G::WsUse, false),
    // (P6-TPL-3, C-1112) the template provenance: status, apply, detach; `can_use`
    r("GET", "/v1/agents/{agent}/template", G::WsUse, false),
    r("POST", "/v1/agents/{agent}/template/apply", G::WsUse, false),
    r("DELETE", "/v1/agents/{agent}/template", G::WsUse, false),
    r(
        "GET",
        "/v1/agents/{agent}/skills/effective",
        G::WsUse,
        false,
    ),
    r("GET", "/v1/workspaces/{ws}/skills", G::WsUse, false),
    r("POST", "/v1/workspaces/{ws}/skills", G::WsOwn, false),
    r("GET", "/v1/orgs/{org}/skills", G::Member, false),
    r("POST", "/v1/orgs/{org}/skills", G::Admin, false),
    // skills addressed by their own id: owner scope unknown, only authentication is asserted
    r("GET", "/v1/skills/{id}", G::Auth, false),
    r("PATCH", "/v1/skills/{id}", G::Auth, false),
    r("DELETE", "/v1/skills/{id}", G::Auth, false),
    r("GET", "/v1/skills/{id}/files/{path}", G::Auth, false),
    r("PUT", "/v1/skills/{id}/files/{path}", G::Auth, false),
    r("DELETE", "/v1/skills/{id}/files/{path}", G::Auth, false),
    // (phase 4, P4-GSL) git skill libraries: management by org owner/admin, browse by anyone who
    // can use a workspace of the org; the update review follows the skill's tier authority
    // (unknown owner scope here: authentication only) and approve/reject/detach are session-only
    r("GET", "/v1/orgs/{org}/skill-libraries", G::UseAny, false),
    r("POST", "/v1/orgs/{org}/skill-libraries", G::Admin, false),
    // (P5-SKLIB-1, C-1124) the check before saving: the same authority as adding a library
    r(
        "POST",
        "/v1/orgs/{org}/skill-libraries/check",
        G::Admin,
        false,
    ),
    r("GET", "/v1/skill-libraries/{id}", G::Auth, false),
    r("PATCH", "/v1/skill-libraries/{id}", G::Auth, false),
    r("DELETE", "/v1/skill-libraries/{id}", G::Auth, false),
    r("POST", "/v1/skill-libraries/{id}/refresh", G::Auth, false),
    r("GET", "/v1/skill-libraries/{id}/skills", G::Auth, false),
    r(
        "GET",
        "/v1/skill-libraries/{id}/skills/{path}",
        G::Auth,
        false,
    ),
    r("GET", "/v1/skills/{id}/library", G::Auth, false),
    r(
        "POST",
        "/v1/skills/{id}/library/updates/{uid}/approve",
        G::Auth,
        true,
    ),
    r(
        "POST",
        "/v1/skills/{id}/library/updates/{uid}/reject",
        G::Auth,
        true,
    ),
    r("POST", "/v1/skills/{id}/library/detach", G::Auth, true),
    r(
        "GET",
        "/v1/orgs/{org}/skill-library-updates",
        G::Admin,
        false,
    ),
    r(
        "GET",
        "/v1/workspaces/{ws}/skill-library-updates",
        G::WsOwn,
        false,
    ),
    // (P6-TPL-2, C-1109) organization template switches: org owner/admin, session or key; a
    // non-member (an instance admin included) is invisible (404), no IA bypass
    r("GET", "/v1/orgs/{org}/agent-templates", G::Admin, false),
    r(
        "PUT",
        "/v1/orgs/{org}/agent-templates/{template_id}",
        G::Admin,
        false,
    ),
    // (P6-TPL-2, C-1110) the workspace catalogue: `can_use` on the workspace
    r(
        "GET",
        "/v1/workspaces/{ws}/agent-templates",
        G::WsUse,
        false,
    ),
    r(
        "GET",
        "/v1/workspaces/{ws}/agent-templates/{template_id}",
        G::WsUse,
        false,
    ),
    r(
        "GET",
        "/v1/workspaces/{ws}/agent-templates/{template_id}/avatar",
        G::WsUse,
        false,
    ),
    // (P6-TPL-3, C-1111) create from a template: `can_use` on the workspace; session or key
    r(
        "POST",
        "/v1/workspaces/{ws}/agents/from-template",
        G::WsUse,
        false,
    ),
    r(
        "GET",
        "/v1/agents/{agent}/skill-library-updates",
        G::WsUse,
        false,
    ),
    r("GET", "/v1/agents/{agent}/sandbox", G::WsUse, false),
    r("POST", "/v1/agents/{agent}/sandbox/retry", G::WsUse, false),
    r("POST", "/v1/agents/{agent}/sandbox/wake", G::WsUse, false),
    // (P5-REFINE-4, C-1060) Sleep now: the wake rule (can use the agent).
    r("POST", "/v1/agents/{agent}/sandbox/sleep", G::WsUse, false),
    r("GET", "/v1/agents/{agent}/start-failures", G::WsUse, false),
    r("POST", "/v1/agents/{agent}/restart", G::WsUse, false),
    r("GET", "/v1/agents/{agent}/models", G::WsUse, false),
    r("GET", "/v1/agents/{agent}/model-readiness", G::WsOwn, false),
    r(
        "GET",
        "/v1/agents/{agent}/answer-readiness",
        G::WsUse,
        false,
    ),
    r("GET", "/v1/agents/{agent}/grants", G::WsOwn, false),
    r(
        "PUT",
        "/v1/agents/{agent}/grants/connections/{conn}",
        G::WsOwn,
        false,
    ),
    r(
        "PUT",
        "/v1/agents/{agent}/grants/apps/{app}",
        G::WsOwn,
        false,
    ),
    r(
        "DELETE",
        "/v1/agents/{agent}/grants/connections/{conn}",
        G::Administer,
        false,
    ),
    r(
        "PUT",
        "/v1/agents/{agent}/grants/secrets/{secret}",
        G::WsOwn,
        false,
    ),
    r(
        "DELETE",
        "/v1/agents/{agent}/grants/secrets/{secret}",
        G::WsOwn,
        false,
    ),
    // ---- resources addressed by their own id (not seeded: denial side only) ----
    r("GET", "/v1/conversations/{id}", G::WsUse, false),
    // (P5-REFINE-7, C-1073) rename: the per-conversation decision (the direct user, `can_own`
    // for other kinds, 409 for run threads) is it/conversations.rs; the matrix asserts the
    // visibility floor.
    r("PATCH", "/v1/conversations/{id}", G::WsUse, false),
    r("GET", "/v1/conversations/{id}/events", G::WsUse, false),
    r("GET", "/v1/conversations/{id}/stream", G::WsUse, false),
    r("POST", "/v1/conversations/{id}/messages", G::WsUse, false),
    r(
        "POST",
        "/v1/conversations/{id}/attachments",
        G::WsUse,
        false,
    ),
    r("GET", "/v1/attachments/{id}/content", G::WsUse, false),
    r("DELETE", "/v1/attachments/{id}", G::WsUse, false),
    r("POST", "/v1/turns/{id}/abort", G::WsUse, false),
    r("POST", "/v1/turns/{id}/send-now", G::WsUse, false),
    // (FIX-225, C-1154, C-1155, SEC-152) Stop authority is `can_use` plus `can_see_conversation`: a
    // follow-up's route answers 404 to anyone who cannot see its conversation. The finer rules (only
    // the author edits, an owner of a direct conversation cancels, an agent token is 401, the 409 and
    // 423 states) are it/followups.rs.
    r("GET", "/v1/conversations/{id}/followups", G::WsUse, false),
    r("POST", "/v1/followups/{id}/steer", G::WsUse, false),
    r("POST", "/v1/followups/{id}/cancel", G::WsUse, false),
    r("PATCH", "/v1/followups/{id}", G::WsUse, false),
    r("POST", "/v1/approvals/{id}/decision", G::WsApprove, false),
    r(
        "POST",
        "/v1/approvals/batches/{batch_id}/decision",
        G::WsApprove,
        false,
    ),
    r("GET", "/v1/connections/{id}", G::WsOwn, false),
    r("PATCH", "/v1/connections/{id}", G::WsOwn, false),
    r("DELETE", "/v1/connections/{id}", G::WsOwn, false),
    // (P5-WEBHOOK-2, C-909) An outgoing webhook's configuration: `can_own` of the workspace (owner
    // or admin for an organization's); the preview and the presets need a signed-in caller.
    r("GET", "/v1/connections/{id}/webhook", G::WsOwn, false),
    r("PUT", "/v1/connections/{id}/webhook", G::WsOwn, false),
    r("POST", "/v1/connections/{id}/webhook/test", G::WsOwn, false),
    r("POST", "/v1/apps/webhook/webhook-preview", G::Auth, false),
    r("GET", "/v1/apps/{app}/webhook-presets", G::Auth, false),
    // (FIX-234, C-1160) An organization's account: grants, removal and the limit preview are
    // administered by its org owners and admins; a workspace's account needs own `can_own`.
    // `{id}` is unseeded here — the org-scope credential block below proves the seeded rows.
    r("GET", "/v1/connections/{id}/grants", G::Administer, false),
    // (P5-UX-11, C-990) who a lower limit would change: workspace `can_own`, or org owner/admin
    // for an organization's account (a workspace's account has no limit: 400).
    r(
        "GET",
        "/v1/connections/{id}/limit-preview",
        G::Administer,
        false,
    ),
    // (FIX-020) credential-side bulk grants: the credential is addressed by its own id (the
    // per-agent `can_own` is proven in grants_bulk.rs); org scope is covered by the org block.
    r("POST", "/v1/connections/{id}/grants", G::Administer, false),
    r("GET", "/v1/secrets/{id}/grants", G::WsOwn, false),
    r("POST", "/v1/secrets/{id}/grants", G::WsOwn, false),
    r("GET", "/v1/secrets/{id}", G::WsOwn, false),
    r("PATCH", "/v1/secrets/{id}", G::WsOwn, false),
    r("DELETE", "/v1/secrets/{id}", G::WsOwn, false),
    r("POST", "/v1/secrets/{id}/accept", G::WsOwn, false),
    r("POST", "/v1/secrets/{id}/reject", G::WsOwn, false),
    r("POST", "/v1/secrets/{id}/verify", G::WsOwn, false),
    // (P5-UX-14, C-943, SEC-129) Test of an account: `can_own` of its workspace (owner/admin for an organization's).
    r("POST", "/v1/accounts/{type}/{id}/test", G::WsOwn, false),
    r("PUT", "/v1/secrets/{id}/value", G::WsOwn, false),
    r("GET", "/v1/secrets/{id}/models", G::WsOwn, false),
    // BE-065 (M5): action approvals, agent links, peer tasks, app-turn cap reset.
    r(
        "GET",
        "/v1/workspaces/{ws}/action-approvals",
        G::WsApprove,
        false,
    ),
    r(
        "POST",
        "/v1/action-approvals/{id}/decision",
        G::WsApprove,
        false,
    ),
    // P5-UX-17 (C-984): agents' requests for access; dismissing is `can_own`.
    r(
        "GET",
        "/v1/workspaces/{ws}/access-requests",
        G::WsUse,
        false,
    ),
    r("POST", "/v1/access-requests/{id}/dismiss", G::WsOwn, false),
    r("GET", "/v1/agents/{agent}/links", G::WsUse, false),
    r("PUT", "/v1/agent-links/{id}/policy", G::WsUse, false),
    // P5-REFINE-10 (C-1085, SEC-142): people link agents. `{agent}` needs `can_use` (the body
    // links it to itself: 409 `link_self` for everyone allowed); the other side's rules are in
    // `agent_links_people.rs`.
    r("GET", "/v1/agents/{agent}/linkable", G::WsUse, false),
    r("POST", "/v1/agents/{agent}/links", G::WsUse, false),
    r("PUT", "/v1/agent-links/{id}/direction", G::WsUse, false),
    r("DELETE", "/v1/agent-links/{id}", G::WsUse, false),
    // P4-EVAL-1 (C-861, C-863, C-866): `can_use` everywhere (a judge change needs `can_own`
    // inside the service; the matrix body leaves the judge alone); no one else learns the agent exists.
    r("GET", "/v1/agents/{agent}/eval-questions", G::WsUse, false),
    r("POST", "/v1/agents/{agent}/eval-questions", G::WsUse, false),
    r("GET", "/v1/eval-questions/{id}", G::WsUse, false),
    r("PATCH", "/v1/eval-questions/{id}", G::WsUse, false),
    r("DELETE", "/v1/eval-questions/{id}", G::WsUse, false),
    r("GET", "/v1/agents/{agent}/eval-settings", G::WsUse, false),
    r("PUT", "/v1/agents/{agent}/eval-settings", G::WsUse, false),
    r(
        "GET",
        "/v1/agents/{agent}/eval-fingerprint",
        G::WsUse,
        false,
    ),
    // P4-EVAL-2 (C-864, C-870): runs and schedules are `can_use`; deleting a run needs `can_own`
    // inside the service (decided after the run is found, so the floor here is `can_use`).
    r("GET", "/v1/agents/{agent}/eval-runs", G::WsUse, false),
    r("POST", "/v1/agents/{agent}/eval-runs", G::WsUse, false),
    r("GET", "/v1/eval-runs/{id}", G::WsUse, false),
    r("DELETE", "/v1/eval-runs/{id}", G::WsUse, false),
    r("POST", "/v1/eval-runs/{id}/cancel", G::WsUse, false),
    r("GET", "/v1/agents/{agent}/eval-schedules", G::WsUse, false),
    r("POST", "/v1/agents/{agent}/eval-schedules", G::WsUse, false),
    r("GET", "/v1/eval-schedules/{id}", G::WsUse, false),
    r("PATCH", "/v1/eval-schedules/{id}", G::WsUse, false),
    r("DELETE", "/v1/eval-schedules/{id}", G::WsUse, false),
    r("GET", "/v1/agents/{agent}/peer-tasks", G::WsUse, false),
    // (P5-REFINE-9, C-1083, C-1084, SEC-143) the agent timeline: `can_use`; its stream is
    // session-only. Rows of conversations the reader may not see: tests/it/timeline.rs.
    r("GET", "/v1/agents/{agent}/timeline", G::WsUse, false),
    r("GET", "/v1/agents/{agent}/timeline/stream", G::WsUse, true),
    r("POST", "/v1/conversations/{id}/continue", G::WsUse, false),
    // P2-010 (RFE-010): the per-conversation decision (direct user / turn author / `can_own`) needs
    // a real conversation and is covered by tests/it/conversation_lifecycle.rs; the matrix asserts
    // the visibility floor (`can_use`).
    r("POST", "/v1/conversations/{id}/archive", G::WsUse, false),
    r("POST", "/v1/conversations/{id}/unarchive", G::WsUse, false),
    r("DELETE", "/v1/conversations/{id}", G::WsUse, false),
    r(
        "POST",
        "/v1/conversations/{id}/events/{seq}/redact",
        G::WsUse,
        false,
    ),
    r("POST", "/v1/turns/{id}/redact", G::WsUse, false),
    r("GET", "/v1/conversations/{id}/export", G::WsUse, false),
    r("GET", "/v1/orgs/{org}/retention", G::Admin, false),
    r("PUT", "/v1/orgs/{org}/retention", G::Admin, false),
    r("GET", "/v1/orgs/{org}/storage", G::Admin, false),
    r("GET", "/v1/orgs/{org}/usage", G::Admin, false),
    r("POST", "/v1/orgs/{org}/storage/refresh", G::Admin, false),
    // BE-074 (M6): channel management.
    r(
        "GET",
        "/v1/orgs/{org}/channel-integrations",
        G::Admin,
        false,
    ),
    r(
        "POST",
        "/v1/orgs/{org}/channel-integrations",
        G::Admin,
        false,
    ),
    r("GET", "/v1/channel-integrations/{id}", G::Admin, false),
    r("PATCH", "/v1/channel-integrations/{id}", G::Admin, false),
    r("DELETE", "/v1/channel-integrations/{id}", G::Admin, false),
    r(
        "GET",
        "/v1/channel-integrations/{id}/user-links",
        G::Admin,
        false,
    ),
    r(
        "POST",
        "/v1/channel-integrations/{id}/user-links",
        G::Admin,
        false,
    ),
    r("DELETE", "/v1/user-links/{id}", G::Admin, false),
    // FIX-127 (C-711..C-713).
    r(
        "POST",
        "/v1/orgs/{org}/channel-integrations:probe",
        G::Admin,
        false,
    ),
    r("GET", "/v1/connector-availability", G::Auth, false),
    r("GET", "/v1/agents/{agent}/presences", G::WsUse, false),
    r(
        "GET",
        "/v1/agents/{agent}/presence-options",
        G::WsUse,
        false,
    ),
    r("POST", "/v1/agents/{agent}/presences", G::WsUse, false),
    r("DELETE", "/v1/presences/{id}", G::WsUse, false),
    r("POST", "/v1/presences/{id}/reattach", G::WsUse, false),
    r("GET", "/v1/agents/{agent}/space-bindings", G::WsOwn, false),
    r("PATCH", "/v1/space-bindings/{id}", G::WsOwn, false),
    r("DELETE", "/v1/space-bindings/{id}", G::WsOwn, false),
    r("GET", "/v1/me/channel-links", G::Auth, true),
    r("DELETE", "/v1/me/channel-links/{id}", G::Auth, true),
    r("GET", "/v1/channel-links/preview", G::Auth, true),
    r("POST", "/v1/channel-links/confirm", G::Auth, true),
    r("GET", "/v1/me/dm-defaults", G::Auth, true),
    r("PUT", "/v1/me/dm-defaults", G::Auth, true),
    r("GET", "/v1/me/dm-default-options", G::Auth, true),
    r("GET", "/v1/agents/{agent}/reach", G::WsUse, false),
    r("POST", "/v1/reach-grants/{id}/decision", G::WsOwn, false),
];

// ---- expectations ----------------------------------------------------------

#[derive(Debug)]
enum Want {
    /// Authorized: must not be 401/403/404 (seeded rows).
    Allow,
    /// Authenticated: anything except 401.
    AllowAuth,
    /// Denied with one of these statuses; optional error code.
    Deny(&'static [u16], Option<&'static str>),
}

fn want(g: G, so: bool, p: P) -> Want {
    use Want::*;
    if p == P::Anon {
        return Deny(&[401], None);
    }
    if so && p.is_key() {
        return Deny(&[403], Some("session_required"));
    }
    match g {
        G::Auth => AllowAuth,
        G::Inst => {
            if p == P::InstAdmin {
                Allow
            } else {
                Deny(&[401, 403, 404], None)
            }
        }
        G::UseAny => {
            if p.is_ws_key() {
                return Deny(&[403], Some("forbidden"));
            }
            // (FIX-234, C-1160) An org-level browse route: the org owner/admin rule is
            // unchanged (the brief keeps `skill_libraries/access.rs` as is), so an
            // administrator still browses as admin without own access. A plain member needs
            // own use of a workspace of the org.
            match org_role(p) {
                Some((role, true)) => {
                    if role != "member" || ws_access(p) != WsAccess::None {
                        Allow
                    } else {
                        Deny(&[403], Some("forbidden"))
                    }
                }
                _ => Deny(&[404], None),
            }
        }
        // (FIX-234, C-1160) The administer routes: org owner/admin or own-right workspace
        // owner; everyone else finds nothing (the route refuses before saying whether the
        // workspace exists). A workspace key acts with its own right, so the same workspace's
        // key passes, another's mismatches.
        G::Administer | G::WsUseOrAdminister => {
            if p == P::WsKeyOther {
                return Deny(&[403], Some("workspace_mismatch"));
            }
            if p == P::WsKeySame {
                return Allow;
            }
            match org_role(p) {
                Some((role, true)) => {
                    let admin = role != "member";
                    let own = ws_access(p);
                    let ok = match g {
                        G::Administer => admin || own == WsAccess::Owner,
                        _ => admin || own != WsAccess::None,
                    };
                    if ok { Allow } else { Deny(&[404], None) }
                }
                _ => Deny(&[404], None),
            }
        }
        G::Member | G::Admin | G::Owner | G::OrgCred => {
            if p.is_ws_key() {
                return Deny(&[403], Some("forbidden"));
            }
            match org_role(p) {
                Some((role, true)) => {
                    let ok = match g {
                        G::Member => true,
                        G::Admin => role != "member",
                        G::OrgCred => role != "member" || ws_access(p) == WsAccess::Owner,
                        _ => role == "owner",
                    };
                    if ok {
                        Allow
                    } else {
                        Deny(&[403], Some("forbidden"))
                    }
                }
                // suspended, non-member, instance admin, unknown => invisible
                _ => Deny(&[404], None),
            }
        }
        G::WsUse | G::WsApprove | G::WsOwn | G::WsUseKeyOnly => {
            if matches!(g, G::WsUseKeyOnly) && !p.is_key() {
                return Deny(&[403], Some("forbidden"));
            }
            if p == P::WsKeyOther {
                return Deny(&[403], Some("workspace_mismatch"));
            }
            match ws_access(p) {
                WsAccess::None => {
                    // (FIX-234, C-1160) An administrator without own access is told they are
                    // not one of the workspace's people (never the content); anyone else
                    // finds nothing.
                    match org_role(p) {
                        Some((role, true)) if role != "member" => {
                            Deny(&[403], Some("workspace_access_required"))
                        }
                        _ => Deny(&[404], None),
                    }
                }
                WsAccess::Member if matches!(g, G::WsOwn) => Deny(&[403], Some("forbidden")),
                WsAccess::Approver if matches!(g, G::WsOwn) => Deny(&[403], Some("forbidden")),
                WsAccess::Member | WsAccess::Approver if matches!(g, G::WsApprove) => {
                    if ws_access(p) == WsAccess::Approver {
                        Allow
                    } else {
                        Deny(&[403], Some("approver_required"))
                    }
                }
                _ => Allow,
            }
        }
    }
}

// ---- seeding and requests --------------------------------------------------

#[derive(Clone)]
enum Auth {
    Anon,
    Cookie(String),
    Bearer(String),
}

struct Fx {
    org: Uuid,
    ws: Uuid,
    agent: Uuid,
    /// Active member bound to `ws` as member, used as the `{user}` target of DELETE/PATCH.
    victim: Uuid,
    auth: HashMap<P, Auth>,
}

/// Seed one org with every principal. Uses the API to create the org so the default agent exists.
async fn seed(h: &Harness, n: u32) -> Fx {
    // Bootstrap through the real org-creation route, then demote the creator so that the
    // org owner principal is not an instance admin.
    let boot = h.user(&format!("boot{n}@x.io"), true).await;
    let (_, boot_tok) = h.good_session(boot).await;
    let c = cookie(&boot_tok);
    let (s, _, v) = h
        .send(
            Method::POST,
            "/v1/orgs",
            &[("cookie", c.as_str()), ("origin", ORIGIN)],
            Some(json!({"name": "Matrix"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let org: Uuid = v["org"]["id"].as_str().unwrap().parse().unwrap();
    let ws: Uuid = v["workspace"]["id"].as_str().unwrap().parse().unwrap();
    sqlx::query("UPDATE users SET is_instance_admin=false WHERE id=$1")
        .bind(boot)
        .execute(h.pool())
        .await
        .unwrap();
    // fresh token after the flag change (auth results are cached per token)
    let (_, owner_tok) = h.good_session(boot).await;
    let agent: Uuid = sqlx::query_scalar("SELECT id FROM agents WHERE workspace_id=$1 LIMIT 1")
        .bind(ws)
        .fetch_one(h.pool())
        .await
        .unwrap();
    // (P6-TPL-3) The agent took the matrix template (stamped the way creation does), so
    // GET/POST/DELETE `/template` have their real outcomes instead of a plain 404.
    sqlx::query("UPDATE agents SET template_id='mattpl', template_name='Matrix', template_version='1.0.0' WHERE id=$1")
        .bind(agent)
        .execute(h.pool())
        .await
        .unwrap();
    // The seeded agent has an avatar so `GET .../avatar` has a 200 outcome to compare against.
    sqlx::query("INSERT INTO blobs (key, content) VALUES ($1, $2)")
        .bind(format!("matrix-avatar-{n}"))
        .bind(&b"png"[..])
        .execute(h.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE agents SET avatar_blob_key=$2, avatar_public_key=$2 WHERE id=$1")
        .bind(agent)
        .bind(format!("matrix-avatar-{n}"))
        .execute(h.pool())
        .await
        .unwrap();
    // (P6-TPL-6) A helper file, so `GET .../helpers/content?path=matrix.sh` (probed in `fire`)
    // has a 200 outcome for an authorized principal to compare against instead of a 404.
    let mut conn = h.pool().acquire().await.unwrap();
    helpers_repo::insert(
        &mut conn,
        agent,
        "matrix.sh",
        b"matrix",
        false,
        &hex::encode(Sha256::digest(b"matrix")),
    )
    .await
    .unwrap();
    drop(conn);
    let ws2 = h.workspace(org).await;

    let adm = h.user(&format!("adm{n}@x.io"), false).await;
    let m = h.user(&format!("m{n}@x.io"), false).await;
    let wo = h.user(&format!("wo{n}@x.io"), false).await;
    let u = h.user(&format!("u{n}@x.io"), false).await;
    let s_user = h.user(&format!("s{n}@x.io"), false).await;
    let pend = h.user(&format!("pend{n}@x.io"), false).await;
    let g_adm = h.user(&format!("gadm{n}@x.io"), false).await;
    let g_wso = h.user(&format!("gwso{n}@x.io"), false).await;
    let g_susp = h.user(&format!("gsusp{n}@x.io"), false).await;
    let nm = h.user(&format!("nm{n}@x.io"), false).await;
    let ia = h.user(&format!("ia{n}@x.io"), true).await;
    let victim = h.user(&format!("victim{n}@x.io"), false).await;
    let ap = h.user(&format!("ap{n}@x.io"), false).await;

    h.member(org, adm, "admin", "active").await;
    h.member(org, m, "member", "active").await;
    h.member(org, wo, "member", "active").await;
    h.member(org, ap, "member", "active").await;
    h.bind_ws(ws, ap, "approver").await;
    h.member(org, u, "member", "active").await;
    h.member(org, s_user, "member", "suspended").await;
    h.member(org, pend, "member", "pending").await;
    h.member(org, g_adm, "member", "active").await;
    h.member(org, g_wso, "member", "active").await;
    h.member(org, g_susp, "member", "suspended").await;
    // Two groups (FIX-083, D-130): one maps to the org role `admin`, one is bound to the
    // workspace as `owner`. The suspended member sits in both and must derive nothing.
    let grp_admin = Uuid::now_v7();
    let grp_ws = Uuid::now_v7();
    for (id, name, role) in [
        (grp_admin, "matrix-admins", Some("admin")),
        (grp_ws, "matrix-ws", None),
    ] {
        sqlx::query("INSERT INTO groups (id, org_id, name, org_role) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(org)
            .bind(format!("{name}-{n}"))
            .bind(role)
            .execute(h.pool())
            .await
            .unwrap();
    }
    sqlx::query(
        "INSERT INTO group_workspace_access (group_id, workspace_id, org_id, role) VALUES ($1, $2, $3, 'owner')",
    )
    .bind(grp_ws)
    .bind(ws)
    .bind(org)
    .execute(h.pool())
    .await
    .unwrap();
    for (grp, user) in [
        (grp_admin, g_adm),
        (grp_admin, g_susp),
        (grp_ws, g_wso),
        (grp_ws, g_susp),
    ] {
        sqlx::query("INSERT INTO group_members (group_id, org_id, user_id) VALUES ($1, $2, $3)")
            .bind(grp)
            .bind(org)
            .bind(user)
            .execute(h.pool())
            .await
            .unwrap();
    }
    h.member(org, victim, "member", "active").await;
    h.bind_ws(ws, m, "member").await;
    h.bind_ws(ws, wo, "owner").await;
    h.bind_ws(ws, s_user, "owner").await;
    // The pending member holds an owner binding too: it must not be honoured.
    h.bind_ws(ws, pend, "owner").await;
    h.bind_ws(ws, victim, "member").await;

    let mut auth = HashMap::new();
    auth.insert(P::Anon, Auth::Anon);
    auth.insert(P::Owner, Auth::Cookie(owner_tok));
    auth.insert(P::Admin, Auth::Cookie(h.good_session(adm).await.1));
    auth.insert(P::WsMember, Auth::Cookie(h.good_session(m).await.1));
    auth.insert(P::WsApprover, Auth::Cookie(h.good_session(ap).await.1));
    auth.insert(P::WsOwner, Auth::Cookie(h.good_session(wo).await.1));
    auth.insert(P::Unbound, Auth::Cookie(h.good_session(u).await.1));
    auth.insert(P::Suspended, Auth::Cookie(h.good_session(s_user).await.1));
    auth.insert(P::Pending, Auth::Cookie(h.good_session(pend).await.1));
    auth.insert(P::GroupAdmin, Auth::Cookie(h.good_session(g_adm).await.1));
    auth.insert(P::GroupWsOwner, Auth::Cookie(h.good_session(g_wso).await.1));
    auth.insert(
        P::SuspendedGroupAdmin,
        Auth::Cookie(h.good_session(g_susp).await.1),
    );
    auth.insert(P::NonMember, Auth::Cookie(h.good_session(nm).await.1));
    auth.insert(P::InstAdmin, Auth::Cookie(h.good_session(ia).await.1));
    auth.insert(
        P::WsKeySame,
        Auth::Bearer(h.api_key(wo, org, Some(ws), false).await.1),
    );
    auth.insert(
        P::WsKeyOther,
        Auth::Bearer(h.api_key(wo, org, Some(ws2), false).await.1),
    );
    auth.insert(
        P::OrgKeyAdmin,
        Auth::Bearer(h.api_key(adm, org, None, false).await.1),
    );
    auth.insert(
        P::OrgKeyMember,
        Auth::Bearer(h.api_key(m, org, None, false).await.1),
    );

    Fx {
        org,
        ws,
        agent,
        victim,
        auth,
    }
}

/// Placeholder names in a path template.
fn placeholders(path: &str) -> Vec<&str> {
    path.split('{')
        .skip(1)
        .filter_map(|s| s.split('}').next())
        .collect()
}

/// Paths whose placeholders are all seeded ids (`org`, `ws`, `agent`, `user`).
fn seeded(path: &str) -> bool {
    placeholders(path)
        .iter()
        .all(|n| matches!(*n, "org" | "ws" | "agent" | "user"))
}

fn fill(path: &str, fx: &Fx) -> String {
    let mut out = String::new();
    let mut rest = path;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let j = rest[i..].find('}').unwrap() + i;
        let v = match &rest[i + 1..j] {
            "org" => fx.org.to_string(),
            "ws" => fx.ws.to_string(),
            "agent" => fx.agent.to_string(),
            "user" => fx.victim.to_string(),
            "rev" | "seq" => "1".to_owned(),
            "type" => "connection".to_owned(),
            _ => Uuid::now_v7().to_string(),
        };
        out.push_str(&v);
        rest = &rest[j + 1..];
    }
    out.push_str(rest);
    out
}

/// Normalize an OpenAPI path so that placeholder names do not matter.
fn norm(path: &str) -> String {
    let mut out = String::new();
    let mut rest = path;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let j = rest[i..].find('}').unwrap() + i;
        out.push_str("{}");
        rest = &rest[j + 1..];
    }
    out.push_str(rest);
    out
}

/// Minimal syntactically valid body for a write row, so that body extraction succeeds and the
/// authorization outcome (not a 400/422) is what is observed. Routes whose request type has only
/// optional fields accept `{}`. Values need not be *semantically* acceptable: the handler may
/// still answer 4xx (404 unknown id, 409, 422) after authorization, which counts as allowed.
fn body_for(method: &str, path: &str, fx: &Fx) -> Value {
    match (method, path) {
        // (P5-UX-5) one exact address; an authorized owner gets a 200, anyone else a refusal before the body is read.
        ("POST", "/v1/workspaces/{ws}/access/by-email") => {
            json!({"email": "matrix@example.test", "role": "member"})
        }
        // (P5-APIGAP-2, C-1047) a subject; any member of the workspace may ask.
        ("POST", "/v1/workspaces/{ws}/requests") => json!({"subject": "ai_key"}),
        ("PUT", "/v1/admin/host-claims/{group_key}") => {
            json!({"winner_package_id": "x", "winner_app_id": "x", "fingerprint": "x"})
        }
        (
            "POST",
            "/v1/admin/oauth-clients"
            | "/v1/orgs/{org}/oauth-clients"
            | "/v1/workspaces/{ws}/oauth-clients",
        ) => json!({"package_id": "x", "client_key": "x", "fields": {}}),
        ("POST", "/v1/admin/app-instances" | "/v1/orgs/{org}/app-instances") => {
            json!({"app_id": "x", "host": "x.example.test", "label": "x"})
        }
        // wrong confirmation text: an authorized caller gets a validation error, nothing is deleted
        (
            "DELETE",
            "/v1/orgs/{org}"
            | "/v1/workspaces/{ws}"
            | "/v1/admin/orgs/{org}"
            | "/v1/admin/workspaces/{ws}",
        ) => json!({"confirm": "not-the-slug"}),
        (
            "POST",
            "/v1/orgs/{org}/freeze"
            | "/v1/workspaces/{ws}/freeze"
            | "/v1/agents/{agent}/freeze"
            | "/v1/admin/orgs/{org}/freeze",
        ) => json!({"reason": "matrix"}),
        ("PATCH", "/v1/followups/{id}") => json!({"text": "matrix"}),
        ("POST", "/v1/orgs") => json!({"name": "Matrix second org"}),
        ("POST", "/v1/orgs/{org}/scim/tokens") => json!({"name": "matrix"}),
        ("POST", "/v1/orgs/{org}/groups") => json!({"name": "matrix group"}),
        ("PUT", "/v1/workspaces/{ws}/access/groups/{group}") => json!({"role": "member"}),
        ("POST", "/v1/admin/runners") => json!({"name": "matrix-runner"}),
        ("PUT", "/v1/admin/settings/{section}" | "/v1/admin/secret-stores/{id}") => {
            json!({"generation": 0, "values": {}, "store": {}})
        }
        ("POST", "/v1/admin/settings/{section}/preview") => json!({"generation": 0, "values": {}}),
        ("POST", "/v1/admin/settings/import") => json!({"document": "{}", "format": "json"}),
        ("PUT", "/v1/admin/secret-store-default") => json!({"generation": 0, "store_id": "x"}),
        ("PUT", "/v1/admin/secret-stores/{id}/reference-scopes") => {
            json!({"generation": 0, "scopes": []})
        }
        ("POST", "/v1/admin/secret-store-test") => json!({"id": "x", "store": {}}),
        ("POST", "/v1/admin/secret-stores/migrations") => {
            json!({"target": "inline", "dry_run": true})
        }
        ("POST", "/v1/admin/trusted-keys") => {
            json!({"publisher": "x", "name": "x", "public_key": "x"})
        }
        ("POST", "/v1/admin/addon-sources" | "/v1/admin/addon-sources/preview") => {
            json!({"url": "https://matrix.example/repo"})
        }
        ("PATCH", "/v1/admin/addon-sources/{id}") => json!({"update_interval": "1d"}),
        // (P6-TPL-2, C-1107/C-1108) the add runs a preview git read and the first check; an
        // authorized instance admin gets the address's own 422, anyone else the denial
        (
            "POST",
            "/v1/admin/agent-template-sources" | "/v1/admin/agent-template-sources/preview",
        ) => json!({"url": "https://git.example/a/b"}),
        ("PATCH", "/v1/admin/agent-template-sources/{id}") => json!({"update_interval": "1d"}),
        // approve is bound to the content hash; an unknown template id answers 404 first
        ("POST", "/v1/admin/agent-templates/{template_id}/versions/{version}/approve") => {
            json!({"content_sha256": "0".repeat(64)})
        }
        ("PATCH", "/v1/admin/agent-templates/{template_id}") => json!({"offered": false}),
        ("PUT", "/v1/orgs/{org}/agent-templates/{template_id}") => json!({"enabled": false}),
        ("POST", "/v1/admin/addon-sources/{id}/confirm-key") => {
            json!({"new_key_fingerprint": "sha256:000000000000"})
        }
        ("POST", "/v1/agents/{agent}/conversations") => json!({"external_ref": "matrix"}),
        ("PUT", "/v1/agents/{agent}/grants/connections/{conn}") => json!({"mode": "full"}),
        ("PUT", "/v1/agents/{agent}/grants/apps/{app}") => json!({"mode": "full"}),
        ("POST", "/v1/secrets/{id}/grants") => json!({"agent_ids": [Uuid::now_v7()]}),
        ("POST", "/v1/connections/{id}/grants") => {
            json!({"agent_ids": [Uuid::now_v7()], "mode": "full"})
        }
        ("POST", "/v1/agents/{agent}/memories") => json!({"key": "k", "content": "c"}),
        ("PUT", "/v1/agents/{agent}/memories/{id}") => json!({"content": "c"}),
        // (P6-MEMLOCK, C-1148) The matrix calls the routes that need a body; the lock route's
        // authorization does not depend on the value.
        ("PUT", "/v1/agents/{agent}/memories/{id}/locked") => json!({"locked": false}),
        ("POST", "/v1/agents/{agent}/memories/{id}/restore") => json!({"revision": 1}),
        ("POST", "/v1/agents/{agent}/schedules") => {
            json!({"name": "s", "prompt": "p", "cron": "0 * * * *"})
        }
        (
            "POST",
            "/v1/agents/{agent}/skills" | "/v1/orgs/{org}/skills" | "/v1/workspaces/{ws}/skills",
        ) => {
            json!({"name": "s", "description": "d", "body": "b"})
        }
        ("POST", "/v1/orgs/{org}/skill-libraries") => {
            json!({"label": "matrix", "url": "https://git.example/a/b"})
        }
        ("POST", "/v1/orgs/{org}/skill-libraries/check" | "/v1/admin/skill-libraries/check") => {
            json!({"url": "https://git.example/a/b"})
        }
        ("POST", "/v1/admin/skill-libraries") => {
            json!({"label": "matrix", "url": "https://git.example/a/b"})
        }
        ("POST", "/v1/skills/{id}/library/updates/{uid}/approve") => {
            json!({"content_sha256": "0".repeat(64)})
        }
        ("POST", "/v1/agents/{agent}/webhooks") => json!({"name": "w", "instructions": "i"}),
        // (P6-TPL-3) The matrix template (seeded below); the stale hash gives an authorized
        // principal the semantic 409, the denial side is refused before the plan is built.
        ("POST", "/v1/workspaces/{ws}/agents/from-template") => {
            json!({
                "template_id": "mattpl", "version": "1.0.0",
                "content_sha256": "0".repeat(64), "name": "Matrix agent",
            })
        }
        ("POST", "/v1/agents/{agent}/template/apply") => {
            json!({"version": "1.0.0", "content_sha256": "0".repeat(64), "parts": []})
        }
        ("POST", "/v1/agents/{agent}/schedules/preview") => json!({"cron": "*/5 * * * *"}),
        ("PUT", "/v1/agents/{agent}/byo") => {
            json!({"source_cidrs": null, "require_context": false})
        }
        ("POST", "/v1/approvals/{id}/decision") => json!({"decision": "deny"}),
        ("POST", "/v1/approvals/batches/{batch_id}/decision") => {
            json!({"decision": "deny", "agent_id": fx.agent})
        }
        ("POST", "/v1/conversations/{id}/messages") => json!({"text": "hi"}),
        ("POST", "/v1/orgs/{org}/api-keys" | "/v1/workspaces/{ws}/api-keys") => {
            json!({"name": "k"})
        }
        ("PUT", "/v1/orgs/{org}/app-availability") => json!({"posture": "open", "allow": []}),
        ("PUT", "/v1/orgs/{org}/governance") => json!({"document": {}, "revision": 0}),
        ("POST", "/v1/orgs/{org}/invitations") => json!({"email": "inv@x.io", "role": "member"}),
        ("PUT", "/v1/orgs/{org}/policy/draft" | "/v1/workspaces/{ws}/policy/draft") => {
            json!({"revision": 0, "default_action": "none", "rules": []})
        }
        ("POST", "/v1/orgs/{org}/policy/publish" | "/v1/workspaces/{ws}/policy/publish") => {
            json!({"revision": 0})
        }
        ("POST", "/v1/orgs/{org}/policy/rollback" | "/v1/workspaces/{ws}/policy/rollback") => {
            json!({"generation": 1, "expect_revision": 0})
        }
        ("POST", "/v1/orgs/{org}/secrets" | "/v1/workspaces/{ws}/secrets") => {
            json!({"name": "matrix-secret", "kind": "generic"})
        }
        ("POST", "/v1/orgs/{org}/workspaces") => json!({"name": "Matrix extra"}),
        ("POST", "/v1/secrets/{id}/accept") => json!({"value": "v"}),
        ("PUT", "/v1/workspaces/{ws}/access/{user}") => json!({"role": "member"}),
        ("PATCH", "/v1/workspaces/{ws}/access/{user}") => json!({"status": "active"}),
        ("PUT", "/v1/schedules/{id}/creator" | "/v1/webhooks/{id}/creator") => {
            json!({"user_id": fx.victim})
        }
        ("POST", "/v1/workspaces/{ws}/automations/reassign") => {
            json!({"from_user_id": fx.victim, "to": "workspace"})
        }
        ("POST", "/v1/workspaces/{ws}/agents") => json!({"name": "Matrix agent", "kind": "byo"}),
        ("POST", "/v1/workspaces/{ws}/policy/simulate") => {
            json!({"agent_id": fx.agent, "method": "GET", "url": "https://example.com/"})
        }
        ("POST", "/v1/orgs/{org}/channel-integrations") => {
            json!({"connector_id": "matrix-none", "config": {}, "credentials": {"token": "x"}})
        }
        ("POST", "/v1/orgs/{org}/channel-integrations:probe") => {
            json!({"connector_id": "matrix-none", "config": {}, "credentials": {"token": "x"}})
        }
        ("PATCH", "/v1/channel-integrations/{id}") => json!({"status": "disabled"}),
        ("POST", "/v1/channel-integrations/{id}/user-links") => {
            json!({"platform_user_ref": "U1", "user_id": fx.victim})
        }
        ("POST", "/v1/agents/{agent}/presences") => {
            json!({"integration_id": Uuid::now_v7(), "mode": "shared"})
        }
        ("POST", "/v1/channel-links/confirm") => {
            json!({"token": "aui_matrix", "platform_user_ref": "U1"})
        }
        ("PUT", "/v1/me/dm-defaults") => {
            json!({"integration_id": Uuid::now_v7(), "agent_id": fx.agent})
        }
        ("POST", "/v1/reach-grants/{id}/decision") => json!({"decision": "block", "revision": 0}),
        ("POST", "/v1/action-approvals/{id}/decision") => json!({"decision": "deny"}),
        ("PUT", "/v1/agent-links/{id}/policy") => {
            json!({"agent_id": fx.agent, "policy": "ask", "revision": 0})
        }
        ("POST", "/v1/agents/{agent}/links") => {
            json!({"peer_agent_id": fx.agent, "direction": "both"})
        }
        ("PUT", "/v1/agent-links/{id}/direction") => {
            json!({"agent_id": fx.agent, "direction": "both", "revision": 0})
        }
        ("POST", "/v1/agents/{agent}/eval-questions") => {
            json!({"question": "q", "expected_kind": "judged", "criteria": "c"})
        }
        ("PUT", "/v1/agents/{agent}/eval-settings") => {
            json!({"judge": null, "regression_notify": true})
        }
        ("POST", "/v1/agents/{agent}/eval-schedules") => {
            json!({"name": "n", "cron": "0 * * * *", "timezone": "UTC"})
        }
        _ => json!({}),
    }
}

async fn fire(h: &Harness, fx: &Fx, row: &Row, p: P) -> (u16, Value) {
    let mut uri = fill(row.p, fx);
    if row.p.ends_with("/memories/search") || row.p.ends_with("/messages/search") {
        uri.push_str("?q=matrix");
    }
    if row.p == "/v1/apps/{app}/setup" || row.p == "/v1/apps/{app}/connect-plan" {
        uri.push_str(&format!("?workspace_id={}", fx.ws));
    }
    if row.p == "/v1/channel-links/preview" {
        uri.push_str("?token=aui_matrix");
    }
    // (P6-TPL-6) The content route's `path` is required (400 `invalid_request` from the query
    // extractor before authorization), so probe with a seeded helper's path like the search
    // routes are probed with `?q=`.
    if row.p == "/v1/agents/{agent}/helpers/content" {
        uri.push_str("?path=matrix.sh");
    }
    let method = Method::from_bytes(row.m.as_bytes()).unwrap();
    let body = (matches!(method, Method::POST | Method::PUT | Method::PATCH)
        || (method == Method::DELETE && matches!(row.p, "/v1/orgs/{org}" | "/v1/workspaces/{ws}")))
    .then(|| body_for(row.m, row.p, fx));
    let c;
    let b;
    let headers: Vec<(&str, &str)> = match &fx.auth[&p] {
        Auth::Anon => vec![],
        Auth::Cookie(t) => {
            c = cookie(t);
            vec![("cookie", c.as_str()), ("origin", ORIGIN)]
        }
        Auth::Bearer(t) => {
            b = bearer(t);
            vec![("authorization", b.as_str())]
        }
    };
    // SSE routes never finish their body once authorized: a timeout means the stream opened.
    match tokio::time::timeout(
        std::time::Duration::from_secs(3),
        h.send(method, &uri, &headers, body),
    )
    .await
    {
        Ok((s, _, v)) => (s.as_u16(), v),
        Err(_) => (200, Value::Null),
    }
}

/// Returns a failure description, or `None` when the outcome matches the expectation.
fn check(
    row: &Row,
    p: P,
    status: u16,
    body: &Value,
    inconclusive: &mut BTreeSet<String>,
) -> Option<String> {
    let f = check_inner(row, p, status, body)?;
    // Body extraction can run before authorization, so a 400/422 on a denial-side request says
    // nothing about the denial side. The row stays listed (with the bodies from `body_for`).
    if matches!(status, 400 | 422) && f.contains("expected [") {
        eprintln!("INCONCLUSIVE (validation before authz) {f}");
        inconclusive.insert(format!("{} {}", row.m, row.p));
        return None;
    }
    Some(f)
}

fn check_inner(row: &Row, p: P, status: u16, body: &Value) -> Option<String> {
    let seeded = seeded(row.p);
    let actual = body["error"]["code"].as_str().unwrap_or("");
    let w = want(row.g, row.so, p);
    let fail = |msg: String| Some(format!("{} {} as {:?}: {}", row.m, row.p, p, msg));
    if status >= 500 {
        return fail(format!("server error {status}"));
    }
    match w {
        Want::AllowAuth => {
            if status == 401 {
                return fail("401 for an authenticated principal".into());
            }
            None
        }
        Want::Allow => {
            if seeded && matches!(status, 401 | 403 | 404) {
                return fail(format!("denied ({status} {actual}), expected allowed"));
            }
            if !seeded && status == 401 {
                return fail("401 for an authenticated principal".into());
            }
            None
        }
        Want::Deny(set, code) => {
            // Unseeded ids resolve to 404 for everyone, so only the denial class is comparable.
            let loose = !seeded && code != Some("session_required");
            let ok_status = if loose {
                matches!(status, 401 | 403 | 404)
            } else {
                set.contains(&status)
            };
            if !ok_status {
                return fail(format!(
                    "status {status} ({actual}), expected {set:?}{}",
                    code.map(|c| format!(" {c}")).unwrap_or_default()
                ));
            }
            if let Some(c) = code
                && (seeded || c == "session_required")
                && actual != c
            {
                return fail(format!("status {status} code '{actual}', expected '{c}'"));
            }
            None
        }
    }
}

// ---- tests -----------------------------------------------------------------

#[tokio::test]
async fn coverage_every_openapi_route_is_listed() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/openapi.json");
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut spec: Vec<(String, String)> = Vec::new();
    for (p, item) in doc["paths"].as_object().unwrap() {
        for m in item.as_object().unwrap().keys() {
            if matches!(m.as_str(), "get" | "post" | "put" | "patch" | "delete") {
                spec.push((m.to_uppercase(), norm(p)));
            }
        }
    }
    let mut listed: Vec<(String, String)> =
        ROWS.iter().map(|r| (r.m.to_owned(), norm(r.p))).collect();
    listed.extend(EXEMPT.iter().map(|(m, p, _)| (m.to_string(), norm(p))));

    let missing: Vec<_> = spec.iter().filter(|s| !listed.contains(s)).collect();
    let stale: Vec<_> = listed.iter().filter(|s| !spec.contains(s)).collect();
    assert!(
        missing.is_empty(),
        "routes missing from matrix or EXEMPT: {missing:?}"
    );
    assert!(stale.is_empty(), "matrix rows not in openapi: {stale:?}");
    let mut dups = listed.clone();
    dups.sort();
    dups.dedup();
    assert_eq!(dups.len(), listed.len(), "duplicate matrix rows");
    assert!(EXEMPT.iter().all(|(_, _, why)| !why.is_empty()));
}

/// Rows whose allowed outcome may change state for later principals: each principal runs
/// against its own freshly seeded database.
fn isolated(row: &Row) -> bool {
    (row.m == "DELETE" && (seeded(row.p) || row.p == "/v1/me"))
        || row.p == "/v1/auth/logout"
        // a successful create makes the creator an owner of the new workspace, which changes
        // what they may read at org level (I-27 amendment) for every later row
        || (row.m == "POST" && row.p == "/v1/orgs/{org}/workspaces")
        // a successful freeze changes later principals' outcomes (and unfreeze needs authority)
        || (row.m == "POST" && (row.p.ends_with("/freeze") || row.p.ends_with("/unfreeze")))
}

#[tokio::test]
async fn permission_matrix_routes_by_principal() {
    let mut failures: Vec<String> = Vec::new();
    let mut inconclusive: BTreeSet<String> = BTreeSet::new();
    // One shared database for all non-isolated rows (creating a migrated database is costly).
    // Agent creation reads the gateway CA to build the container config (503 without it).
    let ca_dir = std::env::temp_dir().join(format!("aurora-ca-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&ca_dir).unwrap();
    std::fs::write(ca_dir.join("ca.pem"), "-----BEGIN CERTIFICATE-----\nX\n").unwrap();
    let ca = ca_dir.to_string_lossy().into_owned();
    // The matrix asserts the access law; the BYO posture that further restricts creating BYO
    // agents (RFE-025, default `owners_only`) is covered in `byo.rs`, so the floor is `enabled`.
    let mut shared = Harness::with_config(move |c| {
        c.gateway_ca_dir = ca;
        c.governance.byo.minimum_posture = aurora_config::ByoPosture::Enabled;
    })
    .await;
    // The API activates an (empty) registry snapshot at startup; the harness does not, and
    // `GET /v1/admin/registry/snapshot` is 404 without one.
    aurora_api::domains::plugins::registry::ensure_initial(&shared.state)
        .await
        .unwrap();
    // (P6-TPL-3) A git server and one approved template (`mattpl@1.0.0`): the create route's
    // plan is built after authorization, so an authorized principal must not 404 on it.
    let git = GitServer::start().await;
    let state = shared.state.clone().with_connect_override(&git.base());
    shared.app = crate::common::test_router(state.clone());
    shared.state = state;
    let tpl_files = [
        (
            "mattpl/template.yaml".to_owned(),
            "format: 1\nid: mattpl\nversion: 1.0.0\nname: Matrix\nsummary: The matrix one.\n"
                .to_owned()
                .into_bytes(),
        ),
        (
            "mattpl/agent.yaml".to_owned(),
            b"format: 1\nname: T\npurpose: monitor\nmodel: default\nmay_message_first: false\n"
                .to_vec(),
        ),
        (
            "mattpl/instructions.md".to_owned(),
            b"Matrix instructions.\n".to_vec(),
        ),
        (
            "mattpl/README.md".to_owned(),
            b"# mattpl\nMatrix.\n".to_vec(),
        ),
    ];
    git.add_repo("acme/tpl");
    let mut tpl_changes: Vec<Change> = vec![Change::Put(
        "aurora-templates.yaml".into(),
        b"api: 1\nrepo:\n  name: Matrix templates\n  description: Test repo\n\
         packages:\n  url: https://git.example/pkg/{id}-{version}.zip\n"
            .to_vec(),
    )];
    tpl_changes.extend(
        tpl_files
            .iter()
            .map(|(p, b)| Change::Put(p.clone(), b.clone())),
    );
    git.commit("acme/tpl", "main", &tpl_changes);
    git.serve_file(
        "pkg/mattpl-1.0.0.zip",
        aurora_template::pack(
            "mattpl",
            &tpl_files
                .iter()
                .map(|(p, b)| aurora_template::TreeFile {
                    path: p.clone(),
                    bytes: b.clone(),
                    mode: 0o100644,
                })
                .collect::<Vec<_>>(),
        ),
    );
    {
        let ta = shared.user("matrix-tpl@x.io", true).await;
        let (_, ta_tok) = shared.good_session(ta).await;
        let tc = cookie(&ta_tok);
        let (s, _, v) = shared
            .send(
                Method::POST,
                "/v1/admin/agent-template-sources",
                &[("cookie", tc.as_str()), ("origin", ORIGIN)],
                Some(json!({"url": "https://git.example/acme/tpl"})),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        let src: Uuid = v["id"].as_str().unwrap().parse().unwrap();
        let (s, _, v) = shared
            .send(
                Method::POST,
                &format!("/v1/admin/agent-template-sources/{src}/check"),
                &[("cookie", tc.as_str()), ("origin", ORIGIN)],
                None,
            )
            .await;
        assert_eq!(s, 200, "{v}");
        let (s, _, v) = shared
            .send(
                Method::GET,
                "/v1/admin/agent-templates/mattpl/versions/1.0.0",
                &[("cookie", tc.as_str()), ("origin", ORIGIN)],
                None,
            )
            .await;
        assert_eq!(s, 200, "{v}");
        let sha = v["content_sha256"].as_str().unwrap().to_owned();
        let (s, _, v) = shared
            .send(
                Method::POST,
                "/v1/admin/agent-templates/mattpl/versions/1.0.0/approve",
                &[("cookie", tc.as_str()), ("origin", ORIGIN)],
                Some(json!({"content_sha256": sha})),
            )
            .await;
        assert_eq!(s, 200, "{v}");
    }
    let shared_fx = seed(&shared, 0).await;
    let mut n = 0u32;
    for row in ROWS {
        if isolated(row) {
            for p in ALL {
                n += 1;
                let fx = seed(&shared, n).await;
                let (status, body) = fire(&shared, &fx, row, p).await;
                failures.extend(check(row, p, status, &body, &mut inconclusive));
            }
        } else {
            for p in ALL {
                let (status, body) = fire(&shared, &shared_fx, row, p).await;
                failures.extend(check(row, p, status, &body, &mut inconclusive));
            }
        }
        eprintln!("checked {} {}", row.m, row.p);
    }
    eprintln!(
        "INCONCLUSIVE rows: {} {:?}",
        inconclusive.len(),
        inconclusive
    );
    assert!(
        failures.is_empty(),
        "{} permission-matrix mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
    // (FIX-101) An INCONCLUSIVE row proves nothing about its denial side (the request failed validation before
    // authorization ran), so it fails the test unless it is on the explicit list below. Fix the row by giving
    // `body_for` a valid minimal body; do not grow the list to make a run pass.
    let unexplained: Vec<&String> = inconclusive
        .iter()
        .filter(|r| {
            !ACCEPTED_INCONCLUSIVE
                .iter()
                .any(|(row, _)| *row == r.as_str())
        })
        .collect();
    assert!(
        unexplained.is_empty(),
        "{} permission-matrix rows are INCONCLUSIVE (validation before authorization) and not on ACCEPTED_INCONCLUSIVE: {unexplained:?}",
        unexplained.len()
    );
    let stale: Vec<&str> = ACCEPTED_INCONCLUSIVE
        .iter()
        .map(|(row, _)| *row)
        .filter(|row| !inconclusive.contains(*row))
        .collect();
    assert!(
        stale.is_empty(),
        "ACCEPTED_INCONCLUSIVE lists rows that are conclusive now (remove them): {stale:?}"
    );
}

/// Rows allowed to stay INCONCLUSIVE, as `("<METHOD> <path template>", "why a valid body cannot be built")`.
/// Empty at FIX-101: all 315 routes answer their denial side with a valid minimal body (checked on
/// 2026-11-14: `INCONCLUSIVE rows: 0`). The list may only shrink.
const ACCEPTED_INCONCLUSIVE: &[(&str, &str)] = &[];

/// Org-scope single-resource reads (I-27 amendment): rows for `/v1/secrets/{id}` etc. use
/// unseeded ids and only exercise workspace scope, so seed an org secret and an org connection
/// and check the same principals against [`G::OrgCred`]. Writes stay owner/admin (rows above).
#[tokio::test]
async fn org_scope_credential_reads_by_principal() {
    let h = Harness::new().await;
    let fx = seed(&h, 0).await;
    let (s, _, v) = h
        .send(
            Method::POST,
            &format!("/v1/orgs/{}/secrets", fx.org),
            &[
                (
                    "cookie",
                    cookie(match &fx.auth[&P::Owner] {
                        Auth::Cookie(t) => t,
                        _ => unreachable!(),
                    })
                    .as_str(),
                ),
                ("origin", ORIGIN),
            ],
            Some(json!({"name": "org-key", "kind": "generic", "host_pattern": "api.example.com", "value": "v"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let secret: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let conn = Uuid::now_v7();
    sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref) VALUES ($1,$2,NULL,'acme_app','acme','1.0.0','c','active','x')")
        .bind(conn)
        .bind(fx.org)
        .execute(h.pool())
        .await
        .unwrap();
    let mut failures = Vec::new();
    for (path, g) in [
        (format!("/v1/secrets/{secret}"), G::OrgCred),
        (format!("/v1/secrets/{secret}/models"), G::OrgCred),
        (format!("/v1/connections/{conn}"), G::OrgCred),
        // (FIX-020) readable like the credential; the list is filtered to workspaces the caller owns
        (format!("/v1/connections/{conn}/grants"), G::OrgCred),
        (format!("/v1/secrets/{secret}/grants"), G::OrgCred),
    ] {
        for p in ALL {
            let (status, _, body) = {
                let (c, b);
                let headers: Vec<(&str, &str)> = match &fx.auth[&p] {
                    Auth::Anon => vec![],
                    Auth::Cookie(t) => {
                        c = cookie(t);
                        vec![("cookie", c.as_str()), ("origin", ORIGIN)]
                    }
                    Auth::Bearer(t) => {
                        b = bearer(t);
                        vec![("authorization", b.as_str())]
                    }
                };
                h.send(Method::GET, &path, &headers, None).await
            };
            let status = status.as_u16();
            let actual = body["error"]["code"].as_str().unwrap_or("");
            let msg = |m: String| failures_push(&path, p, m);
            match want(g, false, p) {
                Want::Deny(set, code) => {
                    if !set.contains(&status) || code.is_some_and(|c| c != actual) {
                        failures.push(msg(format!(
                            "status {status} '{actual}', expected {set:?} {code:?}"
                        )));
                    }
                }
                // The secret/connection exist; models may fail upstream (4xx/5xx other than
                // auth) for a generic secret, so only the auth outcome is asserted.
                _ => {
                    if matches!(status, 401 | 403 | 404) {
                        failures.push(msg(format!(
                            "denied ({status} '{actual}'), expected allowed"
                        )));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn failures_push(path: &str, p: P, m: String) -> String {
    format!("GET {path} as {p:?}: {m}")
}
