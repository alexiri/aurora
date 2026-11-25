//! (FIX-234, C-1160) The route inventory. Every workspace-, agent-, connection-, schedule- and
//! webhook-scoped route of the OpenAPI document must be classified: either it accepts
//! `can_administer` (the C-1160 administer list) or it needs the caller's own access
//! (`can_use`/`can_own`). A route that is in neither list fails the test, so a new route cannot
//! quietly decide its own rule. The lists are compared against the document the router
//! generates — nothing is hand-typed from memory.

use crate::common::*;
use crate::org_role_admin::YAML;
use crate::permission_matrix;
use aurora_api::openapi;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

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
    // Conversations, turns and attachments: the transcript belongs to the workspace's people
    // (D-477: administering is not using). Reading and writing need `can_use`
    // (`can_see_conversation`); deleting, exporting and redacting need `can_own`, a direct
    // conversation's person excepted.
    ("/conversations/{id}", &["get", "patch", "delete"]),
    ("/conversations/{id}/archive", &["post"]),
    ("/conversations/{id}/unarchive", &["post"]),
    ("/conversations/{id}/events", &["get"]),
    ("/conversations/{id}/events/{seq}/redact", &["post"]),
    ("/conversations/{id}/attachments", &["post"]),
    ("/conversations/{id}/continue", &["post"]),
    ("/conversations/{id}/export", &["get"]),
    ("/conversations/{id}/followups", &["get"]),
    ("/conversations/{id}/messages", &["post"]),
    ("/conversations/{id}/stream", &["get"]),
    ("/turns/{id}/abort", &["post"]),
    ("/turns/{id}/redact", &["post"]),
    ("/turns/{id}/send-now", &["post"]),
    ("/attachments/{id}", &["delete"]),
    ("/attachments/{id}/content", &["get"]),
    ("/followups/{id}", &["patch"]),
    ("/followups/{id}/cancel", &["post"]),
    ("/followups/{id}/steer", &["post"]),
    // Approvals: deciding needs the own `can_approve` role (C-1160: `can_administer` does not
    // include it — an administrator without a binding cannot decide). The two personal queues
    // answer the caller's own rows and stay 200 for anyone authenticated (the permission
    // matrix's G::Auth rows); the behavioural walk checks they carry no content.
    ("/approvals/pending", &["get"]),
    ("/approvals/stream", &["get"]),
    ("/approvals/{id}/decision", &["post"]),
    ("/approvals/batches/{batch_id}/decision", &["post"]),
    ("/action-approvals/{id}/decision", &["post"]),
    // The remaining item-scoped routes whose authority is the caller's own access.
    ("/eval-questions/{id}", &["get", "patch", "delete"]),
    ("/eval-runs/{id}", &["get", "delete"]),
    ("/eval-runs/{id}/cancel", &["post"]),
    ("/eval-schedules/{id}", &["get", "patch", "delete"]),
    ("/previews/{id}", &["get", "patch", "delete"]),
    ("/previews/{id}/start", &["post"]),
    ("/presences/{id}", &["delete"]),
    ("/presences/{id}/reattach", &["post"]),
    ("/space-bindings/{id}", &["patch", "delete"]),
];

/// The item-scoped route prefixes the inventory walks: the routes whose authority is a
/// workspace's. The document's paths carry the `/v1` nest prefix; it is stripped here so the
/// lists read like the route catalogue. (`/secrets/{id}`, `/agent-links/{id}`, `/skills/{id}`,
/// `/apps/{app}` and the channel-setup routes keep their own mixed org/workspace authority and
/// stay outside this walk — the permission matrix covers every documented route.)
fn scoped(path: &str) -> bool {
    let p = path.strip_prefix("/v1").unwrap_or(path);
    [
        "/workspaces/{ws}",
        "/agents/{agent}",
        "/connections/{id}",
        "/schedules/{id}",
        "/webhooks/{id}",
        "/conversations/{id}",
        "/turns/{id}",
        "/approvals/",
        "/attachments/{id}",
        "/followups/{id}",
        "/previews/{id}",
        "/presences/{id}",
        "/space-bindings/{id}",
        "/eval-questions/{id}",
        "/eval-runs/{id}",
        "/eval-schedules/{id}",
        "/action-approvals/{id}",
    ]
    .iter()
    .any(|pre| p.starts_with(pre))
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

// ---------------------------------------------------------------- behavioural walk

/// One run order step: GETs first, then the writes in list order, and the state-consuming
/// deletes after their producers (the workspace deletion very last).
fn phase(m: &str, p: &str) -> u8 {
    let delete = m.eq_ignore_ascii_case("delete");
    match (m.to_uppercase().as_str(), p) {
        ("DELETE", "/workspaces/{ws}") => 9,
        ("DELETE", "/agents/{agent}/grants/connections/{conn}") => 3,
        ("DELETE", "/workspaces/{ws}/access/pending/{id}") => 3,
        ("DELETE", "/workspaces/{ws}/access/{user}") => 3,
        ("DELETE", "/workspaces/{ws}/access/groups/{group}") => 3,
        ("GET", _) => 0,
        _ if delete => 3,
        _ => 1,
    }
}

/// The world the walk runs against: an org whose admin holds no workspace binding, two bound
/// members (one as the `{user}` target, one as the author of the content and automations), and
/// the items the administer routes act on.
struct World {
    fx: permission_matrix::Fx,
    h: Harness,
    admin_tok: String,
    group: Uuid,
    conn: Uuid,
    schedule: Uuid,
    webhook: Uuid,
    extra_ws: Uuid,
    extra_slug: String,
    markers: Vec<String>,
}

async fn seed_world() -> World {
    let h = Harness::new().await;
    // The org, created through the API so the default agent exists (the matrix's pattern).
    let boot = h.user("inv-boot@x.io", true).await;
    let (_, boot_tok) = h.good_session(boot).await;
    let c = cookie(&boot_tok);
    let (s, _, v) = h
        .send(
            Method::POST,
            "/v1/orgs",
            &[("cookie", c.as_str()), ("origin", ORIGIN)],
            Some(json!({"name": "Inventory"})),
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
    let agent: Uuid = sqlx::query_scalar("SELECT id FROM agents WHERE workspace_id=$1 LIMIT 1")
        .bind(ws)
        .fetch_one(h.pool())
        .await
        .unwrap();

    // The protagonist: an org admin with no binding anywhere (C-1160), a `{user}` target and a
    // content author, both bound members.
    let admin = h.user("inv-admin@x.io", false).await;
    let victim = h.user("inv-victim@x.io", false).await;
    let author = h.user("inv-author@x.io", false).await;
    h.member(org, admin, "admin", "active").await;
    h.member(org, victim, "member", "active").await;
    h.member(org, author, "member", "active").await;
    h.bind_ws(ws, victim, "member").await;
    // The author owns the workspace so their seeding may write the secret (a `can_own` route).
    h.bind_ws(ws, author, "owner").await;
    let (_, admin_tok) = h.good_session(admin).await;
    let (_, author_tok) = h.good_session(author).await;

    // The acme package, so the org account's bulk grant accepts `full` (SEC-67: an app absent
    // from the registry is custom_only with an empty catalogue).
    crate::plugins_lifecycle::setup(&h)
        .await
        .install(YAML)
        .await;

    // An org-scope connection (workspace_id NULL): the administered organization account.
    let conn = Uuid::now_v7();
    sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref) VALUES ($1,$2,NULL,'acme_app','acme','1.0.0','c','active','x')")
        .bind(conn)
        .bind(org)
        .execute(h.pool())
        .await
        .unwrap();

    // A group, a schedule and a webhook authored by the second member (the creator PUTs then
    // have a real change to make), a direct conversation with a message, a memory and a
    // workspace secret: content an administered response must never carry.
    let (sc, sv) = h
        .call_tok(
            &admin_tok,
            Method::POST,
            &format!("/v1/orgs/{org}/groups"),
            Some(json!({"name": "inventory"})),
        )
        .await;
    assert!((200..300).contains(&sc), "{sv}");
    let group: Uuid = sv["id"].as_str().unwrap().parse().unwrap();

    let (sc, sv) = h
        .call_tok(
            &author_tok,
            Method::POST,
            &format!("/v1/agents/{agent}/schedules"),
            Some(json!({"name": "s", "prompt": "p", "cron": "0 * * * *"})),
        )
        .await;
    assert!((200..300).contains(&sc), "{sv}");
    let schedule: Uuid = sv["id"].as_str().unwrap().parse().unwrap();

    let (sc, sv) = h
        .call_tok(
            &author_tok,
            Method::POST,
            &format!("/v1/agents/{agent}/webhooks"),
            Some(json!({"name": "w", "instructions": "i"})),
        )
        .await;
    assert!((200..300).contains(&sc), "{sv}");
    // `WebhookWithUrl`: the webhook nested one level, the throwaway signing secret beside it.
    let webhook: Uuid = sv["webhook"]["id"].as_str().unwrap().parse().unwrap();

    let (sc, sv) = h
        .call_tok(
            &author_tok,
            Method::POST,
            &format!("/v1/agents/{agent}/conversations/direct"),
            None,
        )
        .await;
    assert!((200..300).contains(&sc), "{sv}");
    let conversation: Uuid = sv["id"].as_str().unwrap().parse().unwrap();
    let (sc, sv) = h
        .call_tok(
            &author_tok,
            Method::POST,
            &format!("/v1/conversations/{conversation}/messages"),
            Some(json!({"text": "MATRIX-message-content"})),
        )
        .await;
    assert!((200..300).contains(&sc), "{sv}");
    let (sc, sv) = h
        .call_tok(
            &author_tok,
            Method::POST,
            &format!("/v1/agents/{agent}/memories"),
            Some(json!({"key": "k", "content": "MATRIX-memory-content"})),
        )
        .await;
    assert!((200..300).contains(&sc), "{sv}");
    let (sc, sv) = h
        .call_tok(
            &author_tok,
            Method::POST,
            &format!("/v1/workspaces/{ws}/secrets"),
            Some(
                json!({"name": "n", "kind": "generic", "host_pattern": "example.com",
                "value": "MATRIX-secret-content"}),
            ),
        )
        .await;
    assert!((200..300).contains(&sc), "{sv}");

    // A second workspace the admin deletes at the very end (the org's default stays).
    let (sc, sv) = h
        .call_tok(
            &admin_tok,
            Method::POST,
            &format!("/v1/orgs/{org}/workspaces"),
            Some(json!({"name": "Inventory extra"})),
        )
        .await;
    assert!((200..300).contains(&sc), "{sv}");
    let extra_ws: Uuid = sv["workspace"]["id"].as_str().unwrap().parse().unwrap();
    let extra_slug: String = sqlx::query_scalar("SELECT slug FROM workspaces WHERE id = $1")
        .bind(extra_ws)
        .fetch_one(h.pool())
        .await
        .unwrap();

    World {
        fx: permission_matrix::Fx {
            org,
            ws,
            agent,
            victim,
            auth: std::collections::HashMap::new(),
        },
        h,
        admin_tok,
        group,
        conn,
        schedule,
        webhook,
        extra_ws,
        extra_slug,
        markers: vec![
            "MATRIX-message-content".into(),
            "MATRIX-memory-content".into(),
            "MATRIX-secret-content".into(),
        ],
    }
}

/// The body of one walk row: the shared `body_for` table, with the walk's overrides.
fn body_for_row(m: &str, p: &str, w: &World) -> Value {
    match (m, p) {
        ("POST", "/connections/{id}/grants") => {
            json!({"agent_ids": [w.fx.agent], "mode": "full"})
        }
        ("DELETE", "/workspaces/{ws}") => json!({"confirm": w.extra_slug}),
        // Routes body_for does not cover (their matrix rows are EXEMPT or need a field).
        ("PUT", "/agents/{agent}/terminal/shell") => json!({"shell": "bash"}),
        ("PUT", "/agents/{agent}/eval-settings") => json!({"regression_notify": false}),
        ("POST", "/agents/{agent}/files/ops") => {
            json!({"op": "mkdir", "path": "/workspace/matrix.sh"})
        }
        // The matrix's body table normalizes the doc's `{key}` to `{id}`; the walk uses the
        // document's names.
        ("PUT", "/agents/{agent}/memories/{key}") => json!({"content": "c"}),
        ("PUT", "/agents/{agent}/memories/{key}/locked") => json!({"locked": false}),
        ("POST", "/agents/{agent}/memories/{key}/restore") => json!({"revision": 1}),
        // Feature-switched: the matrix probes them EXEMPT; a syntactically valid body keeps the
        // 404 `previews_disabled` (the walk's expected refusal) from masking as a body error.
        ("POST", "/agents/{agent}/previews") => json!({"port": 8080}),
        ("PATCH", "/previews/{id}") => json!({"state": "paused"}),
        ("POST", "/agents/{agent}/space-bindings") => {
            json!({"integration_id": Uuid::now_v7(), "space_ref": "x"})
        }
        _ => permission_matrix::body_for(m, &format!("/v1{p}"), &w.fx),
    }
}

/// The URI of one walk row: the filled template with the walk's real ids.
fn uri_for(m: &str, p: &str, w: &World, pending: &mut Option<Uuid>) -> String {
    // The lists read like the route catalogue (no `/v1`); the router nests under it.
    let mut uri = format!("/v1{}", permission_matrix::fill(p, &w.fx));
    match p {
        // The org-scope connection the walk seeded (fill() can only mint a random id).
        "/connections/{id}/grants" | "/connections/{id}/limit-preview" => {
            uri = format!(
                "/v1/connections/{}{}",
                w.conn,
                p.trim_start_matches("/connections/{id}")
            );
            // The preview's query is required (`?limit=`; a mode, not a number).
            if p.ends_with("/limit-preview") {
                uri.push_str("?limit=act");
            }
        }
        "/agents/{agent}/grants/connections/{conn}" => {
            uri = format!("/v1/agents/{}/grants/connections/{}", w.fx.agent, w.conn);
        }
        p if p.contains("/access/groups/{group}") => {
            uri = format!("/v1/workspaces/{}/access/groups/{}", w.fx.ws, w.group);
        }
        "/schedules/{id}/creator" => {
            uri = format!("/v1/schedules/{}/creator", w.schedule);
        }
        "/webhooks/{id}/creator" => {
            uri = format!("/v1/webhooks/{}/creator", w.webhook);
        }
        // The extra workspace: the org's default one must survive the walk.
        "/workspaces/{ws}" if m.eq_ignore_ascii_case("delete") => {
            uri = format!("/v1/workspaces/{}", w.extra_ws);
        }
        "/workspaces/{ws}/access/pending/{id}" if m.eq_ignore_ascii_case("delete") => {
            let id = pending.expect("the by-email row created a pending access row");
            uri = format!("/v1/workspaces/{}/access/pending/{id}", w.fx.ws);
        }
        _ => {}
    }
    uri
}

/// (FIX-234 review 6) The lists are not just names: an org admin with no workspace binding gets
/// a **contentless success** on every ADMINISTER route (they administer, they do not use — no
/// message, memory or secret content may leak), a refusal (403/404) on every OWN route, and the
/// two personal approval queues stay their own empty 200s.
#[tokio::test]
async fn administer_admits_contentless_and_own_refuses_without_a_binding() {
    let w = seed_world().await;
    let c = cookie(&w.admin_tok);

    // Expand both lists to (method, path) rows, ordered for their state dependencies.
    let mut rows: Vec<(String, &'static str)> = Vec::new();
    for (p, ms) in ADMINISTER {
        for m in ms.iter() {
            rows.push((m.to_string(), p));
        }
    }
    rows.sort_by_key(|(m, p)| phase(m, p));
    let own_rows: Vec<(String, &'static str)> = OWN
        .iter()
        .flat_map(|(p, ms)| ms.iter().map(move |m| (m.to_string(), *p)))
        .collect();

    let mut pending: Option<Uuid> = None;
    // GETs of the administer walk run before the by-email write creates the pending row.
    for (m, p) in &rows {
        let uri = uri_for(&m, p, &w, &mut pending);
        // DELETE /workspaces/{ws} carries the typed confirmation.
        let write = m.eq_ignore_ascii_case("post")
            || m.eq_ignore_ascii_case("put")
            || m.eq_ignore_ascii_case("patch");
        let body = (write || (m.eq_ignore_ascii_case("delete") && *p == "/workspaces/{ws}"))
            .then(|| body_for_row(&m.to_uppercase(), p, &w));
        let (s, v) =
            w.h.call(
                &c,
                Method::from_bytes(m.to_uppercase().as_bytes()).unwrap(),
                &uri,
                body,
            )
            .await;
        assert!(
            (200..300).contains(&s),
            "ADMINISTER {m} {p} for an admin without a binding: {s} {v}"
        );
        let text = v.to_string();
        for marker in &w.markers {
            assert!(
                !text.contains(marker),
                "ADMINISTER {m} {p} leaked content marker {marker}: {text}"
            );
        }
        // The by-email row for the pending-delete later in the walk.
        if *p == "/workspaces/{ws}/access/by-email" {
            let (s, v) =
                w.h.call(
                    &c,
                    Method::GET,
                    &format!("/v1/workspaces/{}/access/pending", w.fx.ws),
                    None,
                )
                .await;
            assert_eq!(s, 200, "{v}");
            pending = Some(
                v["items"][0]["id"]
                    .as_str()
                    .unwrap_or_else(|| panic!("by-email created a pending row: {v}"))
                    .parse()
                    .unwrap(),
            );
        }
    }
    // The walk must actually have exercised the administered surface.
    assert!(rows.len() >= 25, "{} administer rows", rows.len());

    for (m, p) in own_rows {
        let uri = format!("/v1{}", permission_matrix::fill(p, &w.fx));
        // Routes whose query extractor runs before authorization need a valid query, or the
        // 400 would mask the refusal (the permission matrix probes these the same way).
        let uri = if p.ends_with("/memories/search") || p.ends_with("/messages/search") {
            format!("{uri}?q=matrix")
        } else if p.ends_with("/helpers/content")
            || p.ends_with("/files/content")
            || p.ends_with("/files/text")
        {
            format!("{uri}?path=/workspace/matrix.sh")
        } else {
            uri
        };
        let body = (m.eq_ignore_ascii_case("post")
            || m.eq_ignore_ascii_case("put")
            || m.eq_ignore_ascii_case("patch")
            || (m.eq_ignore_ascii_case("delete") && p.ends_with("/workspaces/{ws}")))
        .then(|| body_for_row(&m.to_uppercase(), p, &w));
        // SSE routes never finish their body once authorized: a timeout means the stream
        // opened (the personal approval queue for a session does exactly that).
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            w.h.send(
                Method::from_bytes(m.to_uppercase().as_bytes()).unwrap(),
                &uri,
                &[("cookie", c.as_str()), ("origin", ORIGIN)],
                body,
            ),
        )
        .await;
        let (s, v) = match outcome {
            Ok((s, _, v)) => (s.as_u16(), v),
            Err(_) => (200, Value::Null),
        };
        if p == "/approvals/pending" {
            // The caller's own queue: an empty 200, never anyone's approvals.
            assert_eq!(s, 200, "OWN {m} {p}: {v}");
            assert_eq!(v["items"].as_array().map(Vec::len), Some(0), "{v}");
            continue;
        }
        if p == "/approvals/stream" {
            assert_eq!(s, 200, "OWN {m} {p}: the personal stream stays open");
            continue;
        }
        assert!(
            matches!(s, 403 | 404),
            "OWN {m} {p} for an admin without a binding: {s} {}",
            serde_json::to_string(&v).unwrap_or_default()
        );
    }
}
