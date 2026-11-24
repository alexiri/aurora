//! (FIX-234, C-1160) The route inventory. Every workspace-, agent-, connection-, schedule- and
//! webhook-scoped route of the OpenAPI document must be classified: either it accepts
//! `can_administer` (the C-1160 administer list) or it needs the caller's own access
//! (`can_use`/`can_own`). A route that is in neither list fails the test, so a new route cannot
//! quietly decide its own rule. The lists are compared against the document the router
//! generates — nothing is hand-typed from memory.

use aurora_api::openapi;

/// `(path, methods)` of the administer routes (C-1160). A method missing from the tuple is not
/// administered.
const ADMINISTER: &[(&str, &[&str])] = &[
    // The org's workspaces: every row for an administrator, `my_role` null without own access.
    ("/orgs/{org}/workspaces", &["get"]),
    // Workspace metadata (`administers`, contacts, audience) and deletion. PATCH stays own.
    ("/workspaces/{ws}", &["get", "delete"]),
    // People (C-1161 applies to each).
    ("/workspaces/{ws}/access", &["get"]),
    (
        "/workspaces/{ws}/access/{user}",
        &["put", "patch", "delete"],
    ),
    ("/workspaces/{ws}/access/by-email", &["post"]),
    ("/workspaces/{ws}/access/pending", &["get"]),
    ("/workspaces/{ws}/access/pending/{id}", &["delete"]),
    ("/workspaces/{ws}/access/groups/{group}", &["put", "delete"]),
    // Freeze (an administrator through `can_administer`).
    ("/workspaces/{ws}/freeze", &["post"]),
    ("/workspaces/{ws}/unfreeze", &["post"]),
    ("/agents/{agent}/freeze", &["post"]),
    ("/agents/{agent}/unfreeze", &["post"]),
    // Offboarding and reassignment (the new creator still needs own `can_use`).
    ("/workspaces/{ws}/members/{user}/offboarding", &["get"]),
    ("/schedules/{id}/creator", &["put"]),
    ("/webhooks/{id}/creator", &["put"]),
    ("/workspaces/{ws}/automations/reassign", &["post"]),
    // Metadata reads.
    ("/workspaces/{ws}/usage", &["get"]),
    ("/workspaces/{ws}/agent-tokens", &["get"]),
    // The organization's accounts: grants, removal and the limit preview (org-scope only).
    ("/connections/{id}/grants", &["get", "post"]),
    ("/agents/{agent}/grants/connections/{conn}", &["delete"]),
    ("/connections/{id}/limit-preview", &["get"]),
];

/// `(path, methods)` of the routes that need the caller's own access (`can_use` or `can_own`):
/// everything scoped the administer list does not name (D-477: administering is not using). An
/// unclassified route fails the test, so a new one cannot quietly decide its own rule.
const OWN: &[(&str, &[&str])] = &[
    // PATCH stays the own owner's (C-1160): DELETE is the administered one.
    ("/workspaces/{ws}", &["patch"]),
    // The workspace's own surfaces: its agents, people's requests, keys, secrets, apps,
    // policy, activity and audit.
    ("/workspaces/{ws}/access-requests", &["get"]),
    ("/workspaces/{ws}/accounts", &["get"]),
    ("/workspaces/{ws}/action-approvals", &["get"]),
    ("/workspaces/{ws}/activity", &["get"]),
    ("/workspaces/{ws}/agents", &["get", "post"]),
    ("/workspaces/{ws}/agents/from-template", &["post"]),
    ("/workspaces/{ws}/agent-templates", &["get"]),
    ("/workspaces/{ws}/agent-templates/{template_id}", &["get"]),
    (
        "/workspaces/{ws}/agent-templates/{template_id}/avatar",
        &["get"],
    ),
    ("/workspaces/{ws}/api-keys", &["get", "post"]),
    ("/workspaces/{ws}/approvals", &["get"]),
    ("/workspaces/{ws}/attention", &["get"]),
    ("/workspaces/{ws}/audit", &["get"]),
    ("/workspaces/{ws}/connections", &["get"]),
    ("/workspaces/{ws}/internet-access", &["get"]),
    ("/workspaces/{ws}/messages/search", &["get"]),
    ("/workspaces/{ws}/oauth-clients", &["get", "post"]),
    ("/workspaces/{ws}/policy", &["get"]),
    ("/workspaces/{ws}/policy/draft", &["put"]),
    ("/workspaces/{ws}/policy/generations", &["get"]),
    ("/workspaces/{ws}/policy/publish", &["post"]),
    ("/workspaces/{ws}/policy/rollback", &["post"]),
    ("/workspaces/{ws}/policy/simulate", &["post"]),
    ("/workspaces/{ws}/policy/warnings", &["get"]),
    ("/workspaces/{ws}/requests", &["get", "post"]),
    ("/workspaces/{ws}/requests/{id}", &["delete"]),
    ("/workspaces/{ws}/secrets", &["get", "post"]),
    ("/workspaces/{ws}/skill-library-updates", &["get"]),
    ("/workspaces/{ws}/skills", &["get", "post"]),
    (
        "/workspaces/{ws}/apps/{app}/setup-request",
        &["post", "delete"],
    ),
    // The agent itself and everything a person does with it: use it, read it, configure it.
    ("/agents/{agent}", &["get", "patch", "delete"]),
    ("/agents/{agent}/answer-readiness", &["get"]),
    ("/agents/{agent}/apps/{app}/access", &["get"]),
    ("/agents/{agent}/avatar", &["get", "put", "delete"]),
    ("/agents/{agent}/byo", &["put"]),
    ("/agents/{agent}/container-config", &["get"]),
    ("/agents/{agent}/conversation-folders", &["get"]),
    ("/agents/{agent}/conversations", &["get", "post"]),
    ("/agents/{agent}/conversations/direct", &["post"]),
    ("/agents/{agent}/conversations/direct/new", &["post"]),
    ("/agents/{agent}/effective", &["get"]),
    ("/agents/{agent}/eval-fingerprint", &["get"]),
    ("/agents/{agent}/eval-questions", &["get", "post"]),
    ("/agents/{agent}/eval-runs", &["get", "post"]),
    ("/agents/{agent}/eval-schedules", &["get", "post"]),
    ("/agents/{agent}/eval-settings", &["get", "put"]),
    ("/agents/{agent}/files", &["get"]),
    ("/agents/{agent}/files/content", &["get", "put"]),
    ("/agents/{agent}/files/text", &["get"]),
    ("/agents/{agent}/files/ops", &["post"]),
    ("/agents/{agent}/grants", &["get"]),
    ("/agents/{agent}/grants/apps/{app}", &["put"]),
    ("/agents/{agent}/grants/connections/{conn}", &["put"]),
    (
        "/agents/{agent}/grants/secrets/{secret}",
        &["put", "delete"],
    ),
    ("/agents/{agent}/helpers", &["get", "delete"]),
    ("/agents/{agent}/helpers/content", &["get"]),
    ("/agents/{agent}/linkable", &["get"]),
    ("/agents/{agent}/links", &["get", "post"]),
    ("/agents/{agent}/memories", &["get", "post"]),
    ("/agents/{agent}/memories/{key}", &["get", "put", "delete"]),
    ("/agents/{agent}/memories/{key}/locked", &["put"]),
    ("/agents/{agent}/memories/{key}/revisions", &["get"]),
    ("/agents/{agent}/memories/{key}/restore", &["post"]),
    (
        "/agents/{agent}/memories/{key}/revisions/{rev}/redact",
        &["post"],
    ),
    ("/agents/{agent}/memories/search", &["get"]),
    ("/agents/{agent}/model-readiness", &["get"]),
    ("/agents/{agent}/models", &["get"]),
    ("/agents/{agent}/peer-tasks", &["get"]),
    ("/agents/{agent}/presence-options", &["get"]),
    ("/agents/{agent}/presences", &["get", "post"]),
    ("/agents/{agent}/previews", &["get", "post"]),
    ("/agents/{agent}/reach", &["get"]),
    ("/agents/{agent}/restart", &["post"]),
    ("/agents/{agent}/sandbox", &["get"]),
    ("/agents/{agent}/sandbox/retry", &["post"]),
    ("/agents/{agent}/sandbox/sleep", &["post"]),
    ("/agents/{agent}/sandbox/wake", &["post"]),
    ("/agents/{agent}/schedules", &["get", "post"]),
    ("/agents/{agent}/schedules/preview", &["post"]),
    ("/agents/{agent}/skill-library-updates", &["get"]),
    ("/agents/{agent}/skills", &["get", "post"]),
    ("/agents/{agent}/skills/effective", &["get"]),
    ("/agents/{agent}/space-bindings", &["get", "post"]),
    ("/agents/{agent}/ssh-certificate", &["post"]),
    ("/agents/{agent}/ssh-sessions", &["get"]),
    ("/agents/{agent}/start-failures", &["get"]),
    ("/agents/{agent}/template", &["get", "delete"]),
    ("/agents/{agent}/template/apply", &["post"]),
    ("/agents/{agent}/terminal/shell", &["get", "put"]),
    ("/agents/{agent}/timeline", &["get"]),
    ("/agents/{agent}/timeline/stream", &["get"]),
    ("/agents/{agent}/token", &["post"]),
    ("/agents/{agent}/webhooks", &["get", "post"]),
    // Credentials, schedules and webhooks addressed by their own id: own `can_use`/`can_own`
    // (an org-scope connection's grants are the administer list's rows).
    ("/connections/{id}", &["get", "patch", "delete"]),
    ("/connections/{id}/webhook", &["get", "put"]),
    ("/connections/{id}/webhook/test", &["post"]),
    ("/schedules/{id}", &["get", "patch", "delete"]),
    ("/schedules/{id}/runs", &["get"]),
    ("/schedules/{id}/run-now", &["post"]),
    ("/webhooks/{id}", &["get", "patch", "delete"]),
    ("/webhooks/{id}/deliveries", &["get"]),
    ("/webhooks/{id}/rotate", &["post"]),
    ("/webhooks/{id}/rotate-secret", &["post"]),
];

/// The item-scoped route prefixes the inventory walks: the routes whose authority is a
/// workspace's. The document's paths carry the `/v1` nest prefix; it is stripped here so the
/// lists read like the route catalogue.
fn scoped(path: &str) -> bool {
    let p = path.strip_prefix("/v1").unwrap_or(path);
    p.starts_with("/workspaces/{ws}")
        || p.starts_with("/agents/{agent}")
        || p.starts_with("/connections/{id}")
        || p.starts_with("/schedules/{id}")
        || p.starts_with("/webhooks/{id}")
}

fn listed(route: &[(&str, &[&str])], path: &str, method: &str) -> bool {
    let p = path.strip_prefix("/v1").unwrap_or(path);
    route
        .iter()
        .any(|(lp, ms)| *lp == p && ms.contains(&method))
}

#[test]
fn every_scoped_route_is_classified() {
    let doc = openapi::document();
    let mut problems = Vec::new();
    let mut classified = 0usize;
    for (path, item) in doc.paths.paths.iter() {
        if !scoped(path) {
            continue;
        }
        for method in ["get", "put", "post", "delete", "patch"] {
            let op = match method {
                "get" => item.get.as_ref(),
                "put" => item.put.as_ref(),
                "post" => item.post.as_ref(),
                "delete" => item.delete.as_ref(),
                "patch" => item.patch.as_ref(),
                _ => None,
            };
            let Some(_) = op else { continue };
            if listed(ADMINISTER, path, method) || listed(OWN, path, method) {
                classified += 1;
            } else {
                problems.push(format!("{method} {path}"));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "unclassified route(s) — put each into ADMINISTER (accepts `can_administer`) or OWN \
         (needs own access) in tests/it/route_inventory.rs:\n  {}",
        problems.join("\n  ")
    );
    // The lists themselves must not drift: every entry still names a documented route.
    let mut stale = Vec::new();
    for (path, methods) in ADMINISTER.iter().chain(OWN.iter()) {
        for m in methods.iter() {
            let key = format!("/v1{path}");
            let Some(item) = doc.paths.paths.get(key.as_str()) else {
                stale.push(format!("{m} {path} (path gone)"));
                continue;
            };
            let has = match *m {
                "get" => item.get.is_some(),
                "put" => item.put.is_some(),
                "post" => item.post.is_some(),
                "delete" => item.delete.is_some(),
                "patch" => item.patch.is_some(),
                _ => false,
            };
            if !has {
                stale.push(format!("{m} {path} (method gone)"));
            }
        }
    }
    assert!(
        stale.is_empty(),
        "inventory entries without a route (remove them):\n  {}",
        stale.join("\n  ")
    );
    assert!(classified > 0, "the document lists no scoped routes");
}
