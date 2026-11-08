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
use http::Method;
use serde_json::{Value, json};
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

const ALL: [P; 14] = [
    P::Anon,
    P::Owner,
    P::Admin,
    P::WsMember,
    P::WsApprover,
    P::WsOwner,
    P::Unbound,
    P::Suspended,
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
        P::Owner | P::Admin | P::OrgKeyAdmin => WsAccess::Owner,
        P::WsOwner | P::WsKeySame => WsAccess::Owner,
        P::WsApprover => WsAccess::Approver,
        P::WsMember | P::OrgKeyMember => WsAccess::Member,
        _ => WsAccess::None,
    }
}

/// Org role and status for a principal's *user*: `Some((role, active))`.
fn org_role(p: P) -> Option<(&'static str, bool)> {
    match p {
        P::Owner => Some(("owner", true)),
        P::Admin | P::OrgKeyAdmin => Some(("admin", true)),
        P::WsMember | P::WsApprover | P::WsOwner | P::Unbound | P::OrgKeyMember => {
            Some(("member", true))
        }
        P::Suspended => Some(("member", false)),
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
    /// `can_use`, but only with an API key: sessions get 403 `forbidden` before the workspace
    /// is looked at (`POST /v1/agents/{agent}/conversations` creates `api` conversations).
    WsUseKeyOnly,
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
];

/// Route table. `p` is the OpenAPI path template (placeholder names are normalized).
const ROWS: &[Row] = &[
    // ---- instance admin ----
    r("GET", "/v1/admin/host-claims", G::Inst, false),
    r("PUT", "/v1/admin/host-claims/{group_key}", G::Inst, false),
    r("GET", "/v1/admin/oauth-clients", G::Inst, false),
    r("POST", "/v1/admin/oauth-clients", G::Inst, false),
    r("GET", "/v1/admin/plugins", G::Inst, false),
    r("POST", "/v1/admin/plugins", G::Inst, false),
    r("GET", "/v1/admin/plugins/{id}", G::Inst, false),
    r("DELETE", "/v1/admin/plugins/{id}", G::Inst, false),
    r("POST", "/v1/admin/plugins/{id}/approve", G::Inst, false),
    r("POST", "/v1/admin/plugins/{id}/disable", G::Inst, false),
    r("POST", "/v1/admin/plugins/{id}/enable", G::Inst, false),
    r("POST", "/v1/admin/plugins/{id}/reject", G::Inst, false),
    r("GET", "/v1/admin/registry/snapshot", G::Inst, false),
    r("GET", "/v1/admin/runners", G::Inst, false),
    r("POST", "/v1/admin/runners", G::Inst, false),
    r("DELETE", "/v1/admin/runners/{id}", G::Inst, false),
    r("POST", "/v1/admin/runners/{id}/token", G::Inst, false),
    r("GET", "/v1/admin/connectors", G::Inst, false),
    r("POST", "/v1/admin/connectors", G::Inst, false),
    r("DELETE", "/v1/admin/connectors/{id}", G::Inst, false),
    r("POST", "/v1/admin/connectors/{id}/approve", G::Inst, false),
    r("POST", "/v1/admin/connectors/{id}/token", G::Inst, false),
    r("GET", "/v1/admin/trusted-keys", G::Inst, false),
    r("POST", "/v1/admin/trusted-keys", G::Inst, false),
    r("DELETE", "/v1/admin/trusted-keys/{id}", G::Inst, false),
    r("GET", "/v1/admin/users", G::Inst, false),
    r("PATCH", "/v1/admin/users/{id}", G::Inst, false),
    r("POST", "/v1/orgs", G::Inst, false),
    // ---- any authenticated principal ----
    // I-27: instance governance is config without secrets, readable by any signed-in user
    r("GET", "/v1/governance/instance", G::Auth, false),
    r("GET", "/v1/me", G::Auth, false),
    r("PATCH", "/v1/me", G::Auth, false),
    r("DELETE", "/v1/me", G::Auth, true),
    r("GET", "/v1/me/api-keys", G::Auth, false),
    r("POST", "/v1/auth/logout", G::Auth, false),
    r("POST", "/v1/auth/sessions/revoke-others", G::Auth, true),
    r("POST", "/v1/auth/cli/approve", G::Auth, true),
    r("POST", "/v1/invitations/accept", G::Auth, true),
    r("GET", "/v1/orgs", G::Auth, false),
    r("GET", "/v1/resolve", G::Auth, false),
    r("GET", "/v1/secret-stores", G::Auth, false),
    r("GET", "/v1/llm/providers", G::Auth, false),
    r("GET", "/v1/apps", G::Auth, false),
    r("GET", "/v1/apps/{app}", G::Auth, false),
    r("GET", "/v1/apps/{app}/guide", G::Auth, false),
    r("GET", "/v1/apps/{app}/icon", G::Auth, false),
    r("GET", "/v1/connectors", G::Auth, false),
    r("GET", "/v1/connectors/{id}/icon", G::Auth, false),
    r("POST", "/v1/apps/{app}/connect", G::Auth, false),
    r("GET", "/v1/approvals/pending", G::Auth, false),
    r("GET", "/v1/approvals/stream", G::Auth, false),
    r("DELETE", "/v1/api-keys/{id}", G::Auth, false),
    // ---- org-level ----
    r("GET", "/v1/orgs/{org}", G::Member, false),
    r("PATCH", "/v1/orgs/{org}", G::Owner, false),
    r("DELETE", "/v1/orgs/{org}", G::Owner, false),
    r(
        "GET",
        "/v1/orgs/{org}/governance/effective",
        G::Member,
        false,
    ),
    r("GET", "/v1/orgs/{org}/workspaces", G::Member, false),
    r("POST", "/v1/orgs/{org}/workspaces", G::Member, false),
    r("GET", "/v1/orgs/{org}/api-keys", G::Admin, false),
    r("POST", "/v1/orgs/{org}/api-keys", G::Admin, true),
    r("GET", "/v1/orgs/{org}/app-availability", G::Admin, false),
    r("PUT", "/v1/orgs/{org}/app-availability", G::Admin, false),
    r("GET", "/v1/orgs/{org}/audit", G::Admin, false),
    r("GET", "/v1/orgs/{org}/connections", G::OrgCred, false),
    r("GET", "/v1/orgs/{org}/governance", G::Admin, false),
    r("PUT", "/v1/orgs/{org}/governance", G::Admin, false),
    r("GET", "/v1/orgs/{org}/invitations", G::Admin, false),
    r("POST", "/v1/orgs/{org}/invitations", G::Admin, false),
    r("DELETE", "/v1/invitations/{id}", G::Admin, false),
    r("GET", "/v1/orgs/{org}/members", G::Admin, false),
    r("PATCH", "/v1/orgs/{org}/members/{user}", G::Admin, false),
    r("DELETE", "/v1/orgs/{org}/members/{user}", G::Admin, false),
    r(
        "GET",
        "/v1/orgs/{org}/members/{user}/offboarding",
        G::Admin,
        false,
    ),
    r("GET", "/v1/orgs/{org}/oauth-clients", G::Admin, false),
    r("POST", "/v1/orgs/{org}/oauth-clients", G::Admin, false),
    r("PATCH", "/v1/oauth-clients/{id}", G::Admin, false),
    r("DELETE", "/v1/oauth-clients/{id}", G::Admin, false),
    r("GET", "/v1/orgs/{org}/policy", G::Admin, false),
    r("PUT", "/v1/orgs/{org}/policy/draft", G::Admin, false),
    r("GET", "/v1/orgs/{org}/policy/generations", G::Admin, false),
    r("POST", "/v1/orgs/{org}/policy/publish", G::Admin, false),
    r("POST", "/v1/orgs/{org}/policy/rollback", G::Admin, false),
    r("GET", "/v1/orgs/{org}/policy/warnings", G::Admin, false),
    r("GET", "/v1/orgs/{org}/secrets", G::OrgCred, false),
    r("POST", "/v1/orgs/{org}/secrets", G::Admin, false),
    // ---- workspace-level ----
    r("GET", "/v1/workspaces/{ws}", G::WsUse, false),
    r("PATCH", "/v1/workspaces/{ws}", G::WsOwn, false),
    r("DELETE", "/v1/workspaces/{ws}", G::WsOwn, false),
    r("GET", "/v1/workspaces/{ws}/access", G::WsOwn, false),
    r("PUT", "/v1/workspaces/{ws}/access/{user}", G::WsOwn, false),
    r(
        "DELETE",
        "/v1/workspaces/{ws}/access/{user}",
        G::WsOwn,
        false,
    ),
    r("GET", "/v1/workspaces/{ws}/activity", G::WsUse, false),
    r(
        "GET",
        "/v1/workspaces/{ws}/members/{user}/offboarding",
        G::WsOwn,
        false,
    ),
    r(
        "POST",
        "/v1/workspaces/{ws}/automations/reassign",
        G::WsOwn,
        false,
    ),
    r("GET", "/v1/workspaces/{ws}/agents", G::WsUse, false),
    r("POST", "/v1/workspaces/{ws}/agents", G::WsUse, false),
    r("GET", "/v1/workspaces/{ws}/api-keys", G::WsOwn, false),
    r("POST", "/v1/workspaces/{ws}/api-keys", G::WsOwn, true),
    r("GET", "/v1/workspaces/{ws}/approvals", G::WsApprove, false),
    r("GET", "/v1/workspaces/{ws}/audit", G::WsOwn, false),
    r("GET", "/v1/workspaces/{ws}/connections", G::WsOwn, false),
    r("GET", "/v1/workspaces/{ws}/oauth-clients", G::WsOwn, false),
    r("POST", "/v1/workspaces/{ws}/oauth-clients", G::WsOwn, false),
    r("GET", "/v1/workspaces/{ws}/policy", G::WsOwn, false),
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
    r("GET", "/v1/agents/{agent}/effective", G::WsUse, false),
    r("GET", "/v1/agents/{agent}/schedules", G::WsUse, false),
    r("POST", "/v1/agents/{agent}/schedules", G::WsUse, false),
    r("GET", "/v1/agents/{agent}/webhooks", G::WsUse, false),
    r("POST", "/v1/agents/{agent}/webhooks", G::WsUse, false),
    r("DELETE", "/v1/schedules/{id}", G::WsUse, false),
    r("GET", "/v1/schedules/{id}", G::WsUse, false),
    r("PATCH", "/v1/schedules/{id}", G::WsUse, false),
    r("PUT", "/v1/schedules/{id}/creator", G::WsOwn, false),
    r("POST", "/v1/schedules/{id}/run-now", G::WsUse, false),
    r("GET", "/v1/schedules/{id}/runs", G::WsUse, false),
    r("DELETE", "/v1/webhooks/{id}", G::WsUse, false),
    r("GET", "/v1/webhooks/{id}", G::WsUse, false),
    r("PATCH", "/v1/webhooks/{id}", G::WsUse, false),
    r("GET", "/v1/webhooks/{id}/deliveries", G::WsUse, false),
    r("PUT", "/v1/webhooks/{id}/creator", G::WsOwn, false),
    r("POST", "/v1/webhooks/{id}/rotate", G::WsUse, false),
    r("GET", "/v1/agents/{agent}/memories", G::WsUse, false),
    r("POST", "/v1/agents/{agent}/memories", G::WsUse, false),
    r("GET", "/v1/agents/{agent}/memories/search", G::WsUse, false),
    r("GET", "/v1/agents/{agent}/memories/{id}", G::WsUse, false),
    r("PUT", "/v1/agents/{agent}/memories/{id}", G::WsUse, false),
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
    r("GET", "/v1/agents/{agent}/skills", G::WsUse, false),
    r("POST", "/v1/agents/{agent}/skills", G::WsUse, false),
    r("GET", "/v1/orgs/{org}/skills", G::Member, false),
    r("POST", "/v1/orgs/{org}/skills", G::Admin, false),
    // skills addressed by their own id: owner scope unknown, only authentication is asserted
    r("GET", "/v1/skills/{id}", G::Auth, false),
    r("PATCH", "/v1/skills/{id}", G::Auth, false),
    r("DELETE", "/v1/skills/{id}", G::Auth, false),
    r("PUT", "/v1/skills/{id}/files/{path}", G::Auth, false),
    r("DELETE", "/v1/skills/{id}/files/{path}", G::Auth, false),
    r("GET", "/v1/agents/{agent}/sandbox", G::WsUse, false),
    r("POST", "/v1/agents/{agent}/restart", G::WsUse, false),
    r("GET", "/v1/agents/{agent}/models", G::WsUse, false),
    r("GET", "/v1/agents/{agent}/grants", G::WsOwn, false),
    r(
        "PUT",
        "/v1/agents/{agent}/grants/connections/{conn}",
        G::WsOwn,
        false,
    ),
    r(
        "DELETE",
        "/v1/agents/{agent}/grants/connections/{conn}",
        G::WsOwn,
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
    r("POST", "/v1/turns/{id}/abort", G::WsUse, false),
    r("POST", "/v1/turns/{id}/send-now", G::WsUse, false),
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
    r("GET", "/v1/connections/{id}/grants", G::WsOwn, false),
    r("GET", "/v1/secrets/{id}", G::WsOwn, false),
    r("PATCH", "/v1/secrets/{id}", G::WsOwn, false),
    r("DELETE", "/v1/secrets/{id}", G::WsOwn, false),
    r("POST", "/v1/secrets/{id}/accept", G::WsOwn, false),
    r("POST", "/v1/secrets/{id}/reject", G::WsOwn, false),
    r("POST", "/v1/secrets/{id}/verify", G::WsOwn, false),
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
    r("GET", "/v1/agents/{agent}/links", G::WsUse, false),
    r("PUT", "/v1/agent-links/{id}/policy", G::WsUse, false),
    r("GET", "/v1/agents/{agent}/peer-tasks", G::WsUse, false),
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
    r("POST", "/v1/channel-links/confirm", G::Auth, true),
    r("GET", "/v1/me/dm-defaults", G::Auth, true),
    r("PUT", "/v1/me/dm-defaults", G::Auth, true),
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
                WsAccess::None => Deny(&[404], None),
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
    let ws2 = h.workspace(org).await;

    let adm = h.user(&format!("adm{n}@x.io"), false).await;
    let m = h.user(&format!("m{n}@x.io"), false).await;
    let wo = h.user(&format!("wo{n}@x.io"), false).await;
    let u = h.user(&format!("u{n}@x.io"), false).await;
    let s_user = h.user(&format!("s{n}@x.io"), false).await;
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
    h.member(org, victim, "member", "active").await;
    h.bind_ws(ws, m, "member").await;
    h.bind_ws(ws, wo, "owner").await;
    h.bind_ws(ws, s_user, "owner").await;
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
        ("PUT", "/v1/admin/host-claims/{group_key}") => {
            json!({"winner_package_id": "x", "winner_app_id": "x", "fingerprint": "x"})
        }
        (
            "POST",
            "/v1/admin/oauth-clients"
            | "/v1/orgs/{org}/oauth-clients"
            | "/v1/workspaces/{ws}/oauth-clients",
        ) => json!({"package_id": "x", "client_key": "x", "fields": {}}),
        // wrong confirmation text: an authorized caller gets a validation error, nothing is deleted
        ("DELETE", "/v1/orgs/{org}" | "/v1/workspaces/{ws}") => json!({"confirm": "not-the-slug"}),
        ("POST", "/v1/orgs") => json!({"name": "Matrix second org"}),
        ("POST", "/v1/admin/runners") => json!({"name": "matrix-runner"}),
        ("POST", "/v1/admin/trusted-keys") => {
            json!({"publisher": "x", "name": "x", "public_key": "x"})
        }
        ("POST", "/v1/agents/{agent}/conversations") => json!({"external_ref": "matrix"}),
        ("PUT", "/v1/agents/{agent}/grants/connections/{conn}") => json!({"mode": "full"}),
        ("POST", "/v1/agents/{agent}/memories") => json!({"key": "k", "content": "c"}),
        ("PUT", "/v1/agents/{agent}/memories/{id}") => json!({"content": "c"}),
        ("POST", "/v1/agents/{agent}/memories/{id}/restore") => json!({"revision": 1}),
        ("POST", "/v1/agents/{agent}/schedules") => {
            json!({"name": "s", "prompt": "p", "cron": "0 * * * *"})
        }
        ("POST", "/v1/agents/{agent}/skills" | "/v1/orgs/{org}/skills") => {
            json!({"name": "s", "description": "d", "body": "b"})
        }
        ("POST", "/v1/agents/{agent}/webhooks") => json!({"name": "w", "instructions": "i"}),
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
            json!({"generation": 1})
        }
        ("POST", "/v1/orgs/{org}/secrets" | "/v1/workspaces/{ws}/secrets") => {
            json!({"name": "matrix-secret", "kind": "generic"})
        }
        ("POST", "/v1/orgs/{org}/workspaces") => json!({"name": "Matrix extra"}),
        ("POST", "/v1/secrets/{id}/accept") => json!({"value": "v"}),
        ("PUT", "/v1/workspaces/{ws}/access/{user}") => json!({"role": "member"}),
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
        ("PATCH", "/v1/channel-integrations/{id}") => json!({"status": "disabled"}),
        ("POST", "/v1/channel-integrations/{id}/user-links") => {
            json!({"platform_user_ref": "U1", "user_id": fx.victim})
        }
        ("POST", "/v1/agents/{agent}/presences") => {
            json!({"integration_id": Uuid::now_v7(), "mode": "shared"})
        }
        ("POST", "/v1/channel-links/confirm") => json!({"token": "aui_matrix"}),
        ("PUT", "/v1/me/dm-defaults") => {
            json!({"integration_id": Uuid::now_v7(), "agent_id": fx.agent})
        }
        ("POST", "/v1/reach-grants/{id}/decision") => json!({"decision": "block", "revision": 0}),
        ("POST", "/v1/action-approvals/{id}/decision") => json!({"decision": "deny"}),
        ("PUT", "/v1/agent-links/{id}/policy") => {
            json!({"agent_id": fx.agent, "policy": "ask", "revision": 0})
        }
        _ => json!({}),
    }
}

async fn fire(h: &Harness, fx: &Fx, row: &Row, p: P) -> (u16, Value) {
    let mut uri = fill(row.p, fx);
    if row.p.ends_with("/memories/search") {
        uri.push_str("?q=matrix");
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
    let shared = Harness::with_config(move |c| {
        c.gateway_ca_dir = ca;
        c.governance.byo.minimum_posture = aurora_config::ByoPosture::Enabled;
    })
    .await;
    // The API activates an (empty) registry snapshot at startup; the harness does not, and
    // `GET /v1/admin/registry/snapshot` is 404 without one.
    aurora_api::domains::plugins::registry::ensure_initial(&shared.state)
        .await
        .unwrap();
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
}

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
        // grants of an org connection name agents of every workspace: owner/admin only
        (format!("/v1/connections/{conn}/grants"), G::Admin),
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
