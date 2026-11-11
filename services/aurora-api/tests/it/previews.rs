#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Preview share management routes (phase 4, P4-PRV-1, D-144, D-145, SEC-80; docs/18 §Port
//! previews): the switches, the CRUD matrix, the authority law (`can_use`, never an agent
//! credential or an agent tool call), validation, limits, labels and the audit rows.

use crate::common::*;
use crate::hosted::{Rig, rig_with};
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

const DOMAIN: &str = "prv.example.org";

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

/// The rig with previews enabled on the instance and the org switch on.
async fn prv_rig() -> Rig {
    let r = rig_with(|c| {
        c.previews.enabled = true;
        c.previews.domain = Some(DOMAIN.into());
    })
    .await;
    r.org_on().await;
    r
}

/// [`prv_rig`] with the per-org and per-agent caps lowered by the config.
async fn prv_rig_config(org: u32, agent: u32) -> Rig {
    let r = rig_with(move |c| {
        c.previews.enabled = true;
        c.previews.domain = Some(DOMAIN.into());
        c.previews.shares_per_org = org;
        c.previews.shares_per_agent = agent;
    })
    .await;
    r.org_on().await;
    r
}

impl Rig {
    /// The org switch off (`settings.previews_enabled = false`).
    async fn org_off(&self) {
        sqlx::query(
            "UPDATE organizations SET settings = coalesce(settings,'{}'::jsonb) || '{\"previews_enabled\": false}'::jsonb WHERE id = $1",
        )
        .bind(self.org)
        .execute(self.h.pool())
        .await
        .unwrap();
    }

    /// The org switch on (the rig's default after `org_off`).
    async fn org_on(&self) {
        sqlx::query(
            "UPDATE organizations SET settings = coalesce(settings,'{}'::jsonb) || '{\"previews_enabled\": true}'::jsonb WHERE id = $1",
        )
        .bind(self.org)
        .execute(self.h.pool())
        .await
        .unwrap();
    }

    async fn call(&self, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        let (s, _, v) = self
            .h
            .send(
                m,
                uri,
                &[("cookie", &self.cookie), ("origin", ORIGIN)],
                body,
            )
            .await;
        (s.as_u16(), v)
    }

    async fn create(&self, agent: Uuid, body: Value) -> (u16, Value) {
        self.call(
            Method::POST,
            &format!("/v1/agents/{agent}/previews"),
            Some(body),
        )
        .await
    }

    async fn list(&self, agent: Uuid) -> (u16, Value) {
        self.call(Method::GET, &format!("/v1/agents/{agent}/previews"), None)
            .await
    }

    async fn patch(&self, id: Uuid, body: Value) -> (u16, Value) {
        self.call(Method::PATCH, &format!("/v1/previews/{id}"), Some(body))
            .await
    }

    async fn delete(&self, id: Uuid) -> u16 {
        self.call(Method::DELETE, &format!("/v1/previews/{id}"), None)
            .await
            .0
    }

    async fn start(&self, id: Uuid) -> (u16, Value) {
        self.call(Method::POST, &format!("/v1/previews/{id}/start"), None)
            .await
    }
}

/// A hosted agent of the rig with a running sandbox (for the derived `status`), plus the plain
/// member's session for management calls (`self.cookie`).
#[allow(dead_code)]
async fn hosted_agent(r: &Rig) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness) \
         VALUES ($1,$2,$3,$1::text,$1::text,'hosted','fake')",
    )
    .bind(id)
    .bind(r.org)
    .bind(r.ws)
    .execute(r.h.pool())
    .await
    .unwrap();
    id
}

/// `hosted_agent` with the rig's real runner id (a real FK).
async fn hosted_agent_on(r: &Rig, rep: Uuid) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness) \
         VALUES ($1,$2,$3,$1::text,$1::text,'hosted','fake')",
    )
    .bind(id)
    .bind(r.org)
    .bind(r.ws)
    .execute(r.h.pool())
    .await
    .unwrap();
    let runner: Uuid = sqlx::query_scalar("SELECT id FROM runners LIMIT 1")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO sandboxes (id, agent_id, runner_id, channel_replica_id, state, boot_id) \
         VALUES ($1,$2,$3,$4,'running',$5)",
    )
    .bind(Uuid::now_v7())
    .bind(id)
    .bind(runner)
    .bind(rep)
    .bind(Uuid::now_v7())
    .execute(r.h.pool())
    .await
    .unwrap();
    id
}

fn label_ok(label: &str) -> bool {
    label.len() == 25
        && label.starts_with('p')
        && label[1..]
            .chars()
            .all(|c| ("abcdefghijklmnopqrstuvwxyz234567").contains(c))
}

// ---- the switches ----

#[tokio::test]
async fn instance_off_every_route_is_404_and_instance_says_so() {
    let r = rig_with(|_| {}).await; // previews.enabled stays false
    r.org_on().await;
    let agent = hosted_agent(&r).await;
    for (m, uri, body) in [
        (Method::GET, format!("/v1/agents/{agent}/previews"), None),
        (
            Method::POST,
            format!("/v1/agents/{agent}/previews"),
            Some(json!({"port": 3000})),
        ),
        (
            Method::GET,
            format!("/v1/previews/{}", Uuid::now_v7()),
            None,
        ),
        (
            Method::PATCH,
            format!("/v1/previews/{}", Uuid::now_v7()),
            Some(json!({"state": "paused"})),
        ),
        (
            Method::DELETE,
            format!("/v1/previews/{}", Uuid::now_v7()),
            None,
        ),
        (
            Method::POST,
            format!("/v1/previews/{}/start", Uuid::now_v7()),
            None,
        ),
    ] {
        let (s, _, v) =
            r.h.send(
                m.clone(),
                &uri,
                &[("cookie", &r.cookie), ("origin", ORIGIN)],
                body,
            )
            .await;
        assert_eq!(s.as_u16(), 404, "{m} {uri}: {v}");
        // The uniform 404: code `not_found`; the message names the disabled feature.
        assert_eq!(code(&v), "not_found", "{v}");
        assert!(
            v["error"]["message"].as_str().unwrap().contains("previews"),
            "{v}"
        );
    }
    let (s, _, v) = r.h.send(Method::GET, "/v1/instance", &[], None).await;
    assert_eq!(s.as_u16(), 200);
    assert_eq!(v["features"]["previews"], false, "{v}");
    assert_eq!(v["previews"]["enabled"], false, "{v}");
}

#[tokio::test]
async fn org_switch_off_create_and_start_are_409_and_the_instance_reports_on() {
    let r = prv_rig().await;
    r.org_off().await;
    let agent = hosted_agent(&r).await;
    let (s, v) = r.create(agent, json!({"port": 3000})).await;
    assert_eq!((s, code(&v)), (409, "previews_not_enabled_for_org"), "{v}");
    // The instance flag is on; the org switch is what the org settings page reads.
    let (_, _, v) = r.h.send(Method::GET, "/v1/instance", &[], None).await;
    assert_eq!(v["features"]["previews"], true);
    assert_eq!(v["previews"]["enabled"], true);
    assert_eq!(v["previews"]["domain"], DOMAIN);
    // Turn it on: create works.
    r.org_on().await;
    let (s, v) = r.create(agent, json!({"port": 3000})).await;
    assert_eq!(s, 201, "{v}");
}

// ---- the CRUD matrix ----

#[tokio::test]
async fn create_list_patch_delete_roundtrip() {
    let r = prv_rig().await;
    let agent = hosted_agent(&r).await;
    let (s, v) = r
        .create(
            agent,
            json!({"port": 3000, "start_command": "npm run dev", "cwd": "web"}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["port"], 3000);
    assert_eq!(v["visibility"], "private");
    assert_eq!(v["state"], "active");
    assert_eq!(v["status"], "not_listening");
    assert_eq!(v["start_command"], "npm run dev");
    assert_eq!(v["cwd"], "web");
    assert_eq!(
        v["hostname"],
        format!("{}.{}", v["label"].as_str().unwrap(), DOMAIN)
    );
    assert_eq!(
        v["url"],
        format!("https://{}/", v["hostname"].as_str().unwrap())
    );
    assert!(label_ok(v["label"].as_str().unwrap()), "{}", v["label"]);
    assert_eq!(v["created_by"]["label"], "m@x.io");
    let id: Uuid = v["id"].as_str().unwrap().parse().unwrap();

    // List.
    let (s, v) = r.list(agent).await;
    assert_eq!(s, 200);
    assert_eq!(v["shares"].as_array().unwrap().len(), 1);

    // Patch: pause, clear the command; the port and the label are immutable (the schema has no port).
    let (s, v) = r.patch(id, json!({"state": "paused"})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["state"], "paused");
    assert_eq!(v["status"], "paused");
    let (s, v) = r
        .patch(id, json!({"start_command": null, "cwd": null}))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["start_command"], Value::Null);
    assert_eq!(v["cwd"], Value::Null);
    // Invalid state.
    let (s, v) = r.patch(id, json!({"state": "gone"})).await;
    assert_eq!(s, 400, "{v}");

    // Delete: 204, then the share is gone everywhere.
    assert_eq!(r.delete(id).await, 204);
    let (s, v) = r
        .call(Method::GET, &format!("/v1/previews/{id}"), None)
        .await;
    assert_eq!(s, 404, "{v}");
    let (s, v) = r.list(agent).await;
    assert_eq!(s, 200);
    assert_eq!(v["shares"].as_array().unwrap().len(), 0);
    // Deleting again is 404.
    assert_eq!(r.delete(id).await, 404);
}

#[tokio::test]
async fn non_member_of_the_workspace_gets_404_and_confusion_is_avoided() {
    let r = prv_rig().await;
    let agent = hosted_agent(&r).await;
    let (_, v) = r.create(agent, json!({"port": 3000})).await;
    let id: Uuid = v["id"].as_str().unwrap().parse().unwrap();

    // Another member of the org who cannot use this workspace.
    let outsider = r.h.user("out@x.io", false).await;
    r.h.member(r.org, outsider, "member", "active").await;
    let (_, st) = r.h.good_session(outsider).await;
    let c = cookie(&st);
    let (s, _, v) =
        r.h.send(
            Method::GET,
            &format!("/v1/agents/{agent}/previews"),
            &[("cookie", &c), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!(s.as_u16(), 404, "{v}");
    let (s, _, v) =
        r.h.send(
            Method::GET,
            &format!("/v1/previews/{id}"),
            &[("cookie", &c), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!(s.as_u16(), 404, "{v}");
    let (s, _, v) =
        r.h.send(
            Method::DELETE,
            &format!("/v1/previews/{id}"),
            &[("cookie", &c), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!(s.as_u16(), 404, "{v}");
    // It still exists for the member.
    let (s, _) = r
        .call(Method::GET, &format!("/v1/previews/{id}"), None)
        .await;
    assert_eq!(s, 200);
}

#[tokio::test]
async fn agent_credential_and_agent_tool_call_are_refused_sec_80() {
    let r = prv_rig().await;
    let agent = hosted_agent(&r).await;

    // An agent credential (`aug_`, kind 'agent') is never a `Caller` on these routes: 401.
    let tok = format!("aug_{}", Uuid::now_v7().simple());
    sqlx::query(
        "INSERT INTO gateway_credentials (id, kind, token_hash, display_prefix, agent_id) \
         VALUES ($1,'agent',$2,$3,$4)",
    )
    .bind(Uuid::now_v7())
    .bind(aurora_core::token::hash(&tok).as_slice())
    .bind(aurora_core::token::display_prefix(&tok))
    .bind(agent)
    .execute(r.h.pool())
    .await
    .unwrap();
    let b = bearer(&tok);
    for (m, uri, body) in [
        (Method::GET, format!("/v1/agents/{agent}/previews"), None),
        (
            Method::POST,
            format!("/v1/agents/{agent}/previews"),
            Some(json!({"port": 3000})),
        ),
    ] {
        let (s, _, v) = r.h.send(m, &uri, &[("authorization", &b)], body).await;
        assert_eq!(s.as_u16(), 401, "agent credential must be refused: {v}");
    }

    // An agent tool call (the W5 plane, authenticated as the runner) can never create or change
    // a share: no platform tool of the catalogue touches preview_shares.
    let cat = aurora_api::domains::tools::catalogue(r.h.pool(), agent)
        .await
        .unwrap();
    assert!(
        cat.iter().all(|t| !t.name.contains("preview")),
        "no preview tool in the agent catalogue: {:?}",
        cat.iter().map(|t| t.name.clone()).collect::<Vec<_>>()
    );
    // And the raw table has no path from the tool registry: a direct call of every tool's name
    // against the share routes stays unauthenticated (proven above) — the only writers are the
    // session/key routes.
}

// ---- validation ----

#[tokio::test]
async fn port_bounds_reserved_and_duplicates() {
    let r = prv_rig().await;
    let agent = hosted_agent(&r).await;
    for port in [1023, 0, -5, 65536, 100_000] {
        let (s, v) = r.create(agent, json!({"port": port})).await;
        assert_eq!(
            (s, code(&v)),
            (422, "preview_port_forbidden"),
            "{port}: {v}"
        );
    }
    // The supervisor's loopback listeners are never shareable.
    for port in [3128, 3129, 3130] {
        let (s, v) = r.create(agent, json!({"port": port})).await;
        assert_eq!(
            (s, code(&v)),
            (422, "preview_port_forbidden"),
            "{port}: {v}"
        );
    }
    let (s, v) = r.create(agent, json!({"port": 3000})).await;
    assert_eq!(s, 201, "{v}");
    // Same agent, same port: 409.
    let (s, v) = r.create(agent, json!({"port": 3000})).await;
    assert_eq!((s, code(&v)), (409, "preview_exists"), "{v}");
    // Same port on another agent of the same org is fine.
    let agent2 = hosted_agent(&r).await;
    let (s, _) = r.create(agent2, json!({"port": 3000})).await;
    assert_eq!(s, 201);
}

#[tokio::test]
async fn command_and_cwd_validation() {
    let r = prv_rig().await;
    let agent = hosted_agent(&r).await;
    // Command: empty, too long, multi-line, control characters.
    for cmd in ["", "   ", &"x".repeat(1025), "a\nb", "a\r\nb", "a\u{7}b"] {
        let (s, v) = r
            .create(agent, json!({"port": 4000, "start_command": cmd}))
            .await;
        assert_eq!(s, 400, "{cmd:?}: {v}");
        assert_eq!(
            v["error"]["details"]["fields"][0]["path"], "/start_command",
            "{v}"
        );
    }
    // Exactly 1024 is fine; trimmed.
    let ok_cmd = "x".repeat(1024);
    let (s, v) = r
        .create(
            agent,
            json!({"port": 4000, "start_command": format!("  {ok_cmd}  ")}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["start_command"], ok_cmd);
    let id: Uuid = v["id"].as_str().unwrap().parse().unwrap();

    // cwd: absolute, traversal, too long, control.
    for cwd in ["/abs", "a/../b", "..", &"y".repeat(201), "a\u{7}b"] {
        let (s, v) = r.create(agent, json!({"port": 4001, "cwd": cwd})).await;
        assert_eq!(s, 400, "{cwd:?}: {v}");
        assert_eq!(v["error"]["details"]["fields"][0]["path"], "/cwd", "{v}");
    }
    // Fine: relative, no `..`.
    let (s, v) = r
        .create(agent, json!({"port": 4001, "cwd": "web/src"}))
        .await;
    assert_eq!(s, 201, "{v}");
    // Patch with the same validators.
    let (s, v) = r.patch(id, json!({"start_command": "a\nb"})).await;
    assert_eq!(s, 400, "{v}");
}

#[tokio::test]
async fn byo_agent_is_422() {
    let r = prv_rig().await;
    // A BYO agent.
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind) \
         VALUES ($1,$2,$3,$1::text,$1::text,'byo')",
    )
    .bind(id)
    .bind(r.org)
    .bind(r.ws)
    .execute(r.h.pool())
    .await
    .unwrap();
    let (s, v) = r.create(id, json!({"port": 3000})).await;
    assert_eq!((s, code(&v)), (422, "preview_not_supported"), "{v}");
    // A frozen agent is refused on create (423) and start.
    let agent = hosted_agent(&r).await;
    let (_, v) = r
        .create(agent, json!({"port": 3000, "start_command": "npm run dev"}))
        .await;
    let share: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    sqlx::query(
        "UPDATE agents SET frozen_at = now(), frozen_reason = 'x', frozen_authority = 'workspace_owner' WHERE id = $1",
    )
    .bind(agent)
    .execute(r.h.pool())
    .await
    .unwrap();
    let (s, v) = r.create(agent, json!({"port": 3001})).await;
    assert_eq!(s, 423, "{v}");
    let (s, v) = r.start(share).await;
    assert_eq!(s, 423, "{v}");
    // Frozen shows in the status, and unfreezing restores it.
    let (_, v) = r.list(agent).await;
    assert_eq!(v["shares"][0]["status"], "frozen", "{v}");
    sqlx::query(
        "UPDATE agents SET frozen_at = NULL, frozen_reason = NULL, frozen_authority = NULL WHERE id = $1",
    )
        .bind(agent)
        .execute(r.h.pool())
        .await
        .unwrap();
    let (_, v) = r.list(agent).await;
    assert_eq!(v["shares"][0]["status"], "not_listening", "{v}");
}

// ---- limits ----

#[tokio::test]
async fn per_agent_limit() {
    let r = prv_rig().await;
    // Only what the config allows (built-in 10 is high; the config may lower it): use 3 agents
    // and the config default of 10 per agent is impractical here — instead prove the mechanism
    // by exhausting with a lowered config in `per_agent_limit_config`.
    let agent = hosted_agent(&r).await;
    for i in 0..10 {
        let (s, v) = r.create(agent, json!({"port": 2000 + i})).await;
        assert_eq!(s, 201, "{i}: {v}");
    }
    let (s, v) = r.create(agent, json!({"port": 3000})).await;
    assert_eq!((s, code(&v)), (409, "preview_limit"), "{v}");
}

#[tokio::test]
async fn per_org_limit() {
    // Lower the config's per-org cap to 2 (the operator may only lower the built-in).
    let r = prv_rig_config(2, 10).await;
    for i in 0..2 {
        let agent = hosted_agent(&r).await;
        let (s, v) = r.create(agent, json!({"port": 2000 + i})).await;
        assert_eq!(s, 201, "{i}: {v}");
    }
    let agent = hosted_agent(&r).await;
    let (s, v) = r.create(agent, json!({"port": 3000})).await;
    assert_eq!((s, code(&v)), (409, "preview_limit"), "{v}");
}

#[tokio::test]
async fn per_agent_limit_lowered_by_config() {
    let r = prv_rig_config(10, 2).await;
    let agent = hosted_agent(&r).await;
    for i in 0..2 {
        let (s, _) = r.create(agent, json!({"port": 2000 + i})).await;
        assert_eq!(s, 201);
    }
    let (s, v) = r.create(agent, json!({"port": 3000})).await;
    assert_eq!((s, code(&v)), (409, "preview_limit"), "{v}");
}

// ---- labels ----

#[tokio::test]
async fn labels_are_well_formed_unique_and_never_reused() {
    let r = prv_rig().await;
    let agent = hosted_agent(&r).await;
    let mut seen = std::collections::HashSet::new();
    let mut first_deleted: Option<(Uuid, String)> = None;
    // Mint and delete in turns: the per-agent limit stays clear while the minted set only grows
    // (the point is the label shape, uniqueness and that a deleted label is never minted again).
    for i in 0..6u16 {
        let (s, v) = r
            .create(
                agent,
                json!({"port": 2000 + i, "start_command": "npm run dev"}),
            )
            .await;
        assert_eq!(s, 201, "{i}: {v}");
        let label = v["label"].as_str().unwrap().to_owned();
        assert!(label_ok(&label), "{label}");
        assert!(
            seen.insert(label.clone()),
            "label reused within the run: {label}"
        );
        if i == 0 {
            let id: Uuid = v["id"].as_str().unwrap().parse().unwrap();
            first_deleted = Some((id, label));
        } else {
            let id: Uuid = v["id"].as_str().unwrap().parse().unwrap();
            assert_eq!(r.delete(id).await, 204);
        }
    }
    // Delete the first and create anew: the label is never minted again.
    let (id, label) = first_deleted.unwrap();
    assert_eq!(r.delete(id).await, 204);
    for i in 0..6u16 {
        let (s, v) = r.create(agent, json!({"port": 5000 + i})).await;
        assert_eq!(s, 201, "{i}: {v}");
        let l = v["label"].as_str().unwrap();
        assert_ne!(l, label, "a deleted label was minted again");
        assert!(seen.insert(l.to_owned()), "label reused: {l}");
        let id: Uuid = v["id"].as_str().unwrap().parse().unwrap();
        assert_eq!(r.delete(id).await, 204);
    }
}

// FIX-043: the spent-label tombstone (`preview_labels`) — written transactionally by create,
// kept by delete, and part of the mint's exclusion set (the "never reused" law, SEC-80).
#[tokio::test]
async fn deleted_label_is_tombstoned_and_excluded_from_the_mint() {
    let r = prv_rig().await;
    let agent = hosted_agent(&r).await;
    let (s, v) = r.create(agent, json!({"port": 3000})).await;
    assert_eq!(s, 201, "{v}");
    let id: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let label: String = v["label"].as_str().unwrap().to_owned();

    // Create tombstones the minted label in the same transaction as the share row.
    let tombstoned: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM preview_labels WHERE label = $1)")
            .bind(&label)
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert!(tombstoned, "create did not tombstone {label}");

    // Delete: the share row is gone, the tombstone survives.
    assert_eq!(r.delete(id).await, 204);
    let share_gone: bool =
        sqlx::query_scalar("SELECT NOT EXISTS (SELECT 1 FROM preview_shares WHERE label = $1)")
            .bind(&label)
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert!(share_gone, "the share survived the delete");
    let tombstoned: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM preview_labels WHERE label = $1)")
            .bind(&label)
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert!(tombstoned, "the delete removed the tombstone of {label}");

    // The mint's exclusion query (existing rows union tombstones) still contains the deleted
    // label — the rows alone no longer do, so the tombstone is what keeps it out of the draw.
    let excluded: Vec<String> = sqlx::query_scalar(
        "SELECT label::text FROM preview_shares UNION SELECT label::text FROM preview_labels",
    )
    .fetch_all(r.h.pool())
    .await
    .unwrap();
    assert!(
        excluded.iter().any(|l| l == &label),
        "the mint's exclusion query lost the deleted label {label}"
    );

    // And the port freed by the delete can be taken again, under a fresh label.
    let (s, v) = r.create(agent, json!({"port": 3000})).await;
    assert_eq!(s, 201, "{v}");
    assert_ne!(v["label"].as_str().unwrap(), label, "the label was reused");
}

// ---- start ----

#[tokio::test]
async fn start_requires_active_share_with_command_and_is_rate_limited() {
    let r = prv_rig().await;
    // A running sandbox (the stamps land on it and the derived status reads them).
    let agent = hosted_agent_on(&r, r.rep).await;
    // No command: 409.
    let (_, v) = r.create(agent, json!({"port": 3000})).await;
    let no_cmd: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let (s, v) = r.start(no_cmd).await;
    assert_eq!((s, code(&v)), (409, "preview_no_start_command"), "{v}");
    // Paused: 409.
    let (_, v) = r
        .create(agent, json!({"port": 3001, "start_command": "npm run dev"}))
        .await;
    let paused: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let (s, _) = r.patch(paused, json!({"state": "paused"})).await;
    assert_eq!(s, 200);
    let (s, v) = r.start(paused).await;
    assert_eq!((s, code(&v)), (409, "preview_paused"), "{v}");
    // A good one: 202, both stamps set, the best-effort audit row, no command text anywhere.
    let (_, v) = r
        .create(
            agent,
            json!({"port": 3002, "start_command": "SECRET_CMD npm run dev"}),
        )
        .await;
    let good: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let (s, v) = r.start(good).await;
    assert_eq!(s, 202, "{v}");
    assert_eq!(v["status"], "starting");
    let (wake, req): (bool, bool) = sqlx::query_as(
        "SELECT preview_wake_at IS NOT NULL, preview_start_requested_at IS NOT NULL FROM sandboxes WHERE agent_id = $1",
    )
    .bind(agent)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert!(wake && req, "both stamps must be set");
    // The wake is rate-limited per share (`PREVIEW_WAKES_PER_SHARE_HOUR`).
    let cap = i64::from(aurora_core::limits::PREVIEW_WAKES_PER_SHARE_HOUR);
    for _ in 1..cap {
        let (s, _) = r.start(good).await;
        assert_eq!(s, 202);
    }
    let (s, v) = r.start(good).await;
    assert_eq!((s, code(&v)), (429, "preview_wake_limited"), "{v}");
    // The audit rows never carry the command text.
    let rows: Vec<(String, Value)> = sqlx::query_as(
        "SELECT action, metadata FROM audit_log WHERE action LIKE 'preview.%' ORDER BY created_at",
    )
    .fetch_all(r.h.pool())
    .await
    .unwrap();
    let actions: Vec<&str> = rows.iter().map(|(a, _)| a.as_str()).collect();
    assert!(actions.contains(&"preview.share.create"), "{actions:?}");
    assert!(actions.contains(&"preview.share.start"), "{actions:?}");
    for (a, m) in &rows {
        assert!(
            !m.to_string().contains("SECRET_CMD") && !m.to_string().contains("npm run dev"),
            "command text in {a}: {m}"
        );
    }
    // The create row of the share with a command carries the hash prefix and the flag, not the
    // command (the first share, port 3000, has none).
    let (_, m) = rows
        .iter()
        .find(|(a, m)| a == "preview.share.create" && m["port"] == 3002)
        .unwrap();
    assert_eq!(m["start_command_set"], true);
    assert_eq!(m["start_command_sha256_12"].as_str().unwrap().len(), 12);
}

#[tokio::test]
async fn api_key_management_is_allowed() {
    let r = prv_rig().await;
    let agent = hosted_agent(&r).await;
    // A workspace API key of the member (can_use).
    let user: Uuid =
        sqlx::query_scalar("SELECT user_id FROM workspace_access WHERE workspace_id = $1")
            .bind(r.ws)
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    let (_, key) = r.h.api_key(user, r.org, Some(r.ws), false).await;
    let b = bearer(&key);
    let (s, _, v) =
        r.h.send(
            Method::POST,
            &format!("/v1/agents/{agent}/previews"),
            &[("authorization", &b)],
            Some(json!({"port": 3000})),
        )
        .await;
    assert_eq!(s.as_u16(), 201, "{v}");
    let (s, _, v) =
        r.h.send(
            Method::GET,
            &format!("/v1/agents/{agent}/previews"),
            &[("authorization", &b)],
            None,
        )
        .await;
    assert_eq!(s.as_u16(), 200, "{v}");
    assert_eq!(v["shares"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn wake_of_a_parked_sandbox_stamps_wake_and_never_starts_a_turn() {
    // Lifecycle light: the wake demand (arm 3) is the start route's stamp; no turn is created.
    let r = prv_rig().await;
    let rig2 = &r;
    let agent = hosted_agent(rig2).await;
    // Parked: a stopped sandbox row.
    let runner: Uuid = sqlx::query_scalar("SELECT id FROM runners LIMIT 1")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    let sb = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO sandboxes (id, agent_id, runner_id, state, boot_id) VALUES ($1,$2,$3,'stopped',$4)",
    )
    .bind(sb)
    .bind(agent)
    .bind(runner)
    .bind(Uuid::now_v7())
    .execute(r.h.pool())
    .await
    .unwrap();
    let (_, v) = r
        .create(agent, json!({"port": 3000, "start_command": "npm run dev"}))
        .await;
    let share: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let (s, _) = r.start(share).await;
    assert_eq!(s, 202);
    let (_, wake): (Uuid, Option<chrono::DateTime<chrono::Utc>>) =
        sqlx::query_as("SELECT agent_id, preview_wake_at FROM sandboxes WHERE id = $1")
            .bind(sb)
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert!(wake.is_some(), "parked sandbox must carry the wake demand");
    // No turn was created by the wake (previews never create turns, docs/18).
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM turns WHERE agent_id = $1")
        .bind(agent)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn preview_start_demanded_via_arm5_shape_is_claimable() {
    // The repo-level arm-5 claim: a running sandbox at the applied generation with a waiting
    // start request yields the demand; a frozen agent yields none.
    let r = prv_rig().await;
    let rep = r.rep;
    let agent = hosted_agent_on(&r, rep).await;
    let (_, v) = r
        .create(agent, json!({"port": 3000, "start_command": "npm run dev"}))
        .await;
    let share: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let runner: Uuid = sqlx::query_scalar("SELECT id FROM runners LIMIT 1")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    // The agent's home generation is applied (as after a home sync) and a request waits.
    sqlx::query("UPDATE agents SET home_sync_generation = 3 WHERE id = $1")
        .bind(agent)
        .execute(r.h.pool())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE sandboxes SET home_applied_generation = 4, preview_start_requested_at = now() WHERE agent_id = $1",
    )
    .bind(agent)
    .execute(r.h.pool())
    .await
    .unwrap();
    let mut conn = r.h.pool().acquire().await.unwrap();
    let d = aurora_db::repo::preview_shares::claim_start_demands(&mut conn, runner, rep, 45.0, 10)
        .await
        .unwrap();
    assert_eq!(d.len(), 1, "the waiting demand is claimed");
    assert_eq!(d[0].share_id, share);
    assert_eq!(d[0].agent_id, agent);
    // Not listening: the status is `waking` while the request is young.
    drop(conn);
    let (s, v) = r.list(agent).await;
    assert_eq!(v["shares"][0]["status"], "waking", "{s} {v}");
    // Freeze the agent: no demand is claimed.
    sqlx::query(
        "UPDATE agents SET frozen_at = now(), frozen_reason = 'x', frozen_authority = 'workspace_owner' WHERE id = $1",
    )
    .bind(agent)
    .execute(r.h.pool())
    .await
    .unwrap();
    let mut conn = r.h.pool().acquire().await.unwrap();
    let d = aurora_db::repo::preview_shares::claim_start_demands(&mut conn, runner, rep, 45.0, 10)
        .await
        .unwrap();
    assert_eq!(d.len(), 0, "a frozen agent never claims starts");
}

/// A parked sandbox is woken by the start click (arm 3 claims, `begin_boot` clears the start
/// stamp), and once the boot's home sync is applied the API re-presents the start demand (the
/// sandbox still carries the `preview_wake_at` that caused this boot), so arm 5 emits
/// `preview.start` without a second click (FIX-041). An ordinary turn-caused boot (no wake
/// stamp) presents no start demand at the same moment.
#[tokio::test]
async fn wake_boot_re_stamps_the_start_demand_at_home_sync() {
    let r = prv_rig().await;
    let runner: Uuid = sqlx::query_scalar("SELECT id FROM runners LIMIT 1")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    let live = |agent: Uuid, conv: Uuid, sb: Uuid, boot: Uuid| crate::hosted::Live {
        agent,
        conv,
        sb: sb.to_string(),
        boot: boot.to_string(),
    };
    async fn desired_generation(pool: &sqlx::PgPool, sb: Uuid) -> i64 {
        sqlx::query_scalar(
            "SELECT a.home_sync_generation + w.home_sync_generation + 1 \
             FROM sandboxes s JOIN agents a ON a.id = s.agent_id \
             JOIN workspaces w ON w.id = a.workspace_id WHERE s.id = $1",
        )
        .bind(sb)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    // Parked: a stopped sandbox row, as after an idle park.
    let agent = hosted_agent(&r).await;
    let sb = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO sandboxes (id, agent_id, runner_id, state, boot_id) VALUES ($1,$2,$3,'stopped',$4)",
    )
    .bind(sb)
    .bind(agent)
    .bind(runner)
    .bind(Uuid::now_v7())
    .execute(r.h.pool())
    .await
    .unwrap();
    let (_, v) = r
        .create(agent, json!({"port": 3000, "start_command": "npm run dev"}))
        .await;
    let share: Uuid = v["id"].as_str().unwrap().parse().unwrap();

    // The first click on the parked share: 202, both demands stamped.
    let (s, v) = r.start(share).await;
    assert_eq!(s, 202, "{v}");
    let (wake, req): (bool, bool) = sqlx::query_as(
        "SELECT preview_wake_at IS NOT NULL, preview_start_requested_at IS NOT NULL \
         FROM sandboxes WHERE id = $1",
    )
    .bind(sb)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert!(wake && req, "both stamps must be set");

    // Arm 3 claims the wake; `begin_boot` clears the start stamp (the defect's losing step).
    let items = r.work().await;
    let start = items
        .iter()
        .find(|i| i["kind"] == "sandbox.start")
        .expect("the wake must be claimed by arm 3");
    assert_eq!(start["sandbox_id"].as_str().unwrap(), sb.to_string());
    let boot: Uuid = start["boot_id"].as_str().unwrap().parse().unwrap();
    let (state, wake, req): (String, bool, bool) = sqlx::query_as(
        "SELECT state, preview_wake_at IS NOT NULL, preview_start_requested_at IS NOT NULL \
         FROM sandboxes WHERE id = $1",
    )
    .bind(sb)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(state, "starting");
    assert!(wake, "the wake stamp survives begin_boot");
    assert!(!req, "begin_boot clears the start stamp");

    // The boot completes: running, channel held, supervisor ready, home synced at the desired
    // generation.
    assert_eq!(
        r.report(
            &live(agent, Uuid::now_v7(), sb, boot),
            json!({"kind": "sandbox.status", "state": "running", "container_ref": "c"})
        )
        .await,
        "applied"
    );
    let (s, _) = r
        .runner(
            Some(r.rep),
            "/v1/runner/channel/hold",
            json!({"sandbox_id": sb, "boot_id": boot}),
        )
        .await;
    assert_eq!(s, 200);
    r.report(
        &live(agent, Uuid::now_v7(), sb, boot),
        json!({"kind": "supervisor.ready", "version": "0.1",
            "harness": {"id": "fake", "capabilities": ["steer", "resume"]}}),
    )
    .await;
    let generation = desired_generation(r.h.pool(), sb).await;
    assert_eq!(
        r.report(
            &live(agent, Uuid::now_v7(), sb, boot),
            json!({"kind": "home.synced", "generation": generation})
        )
        .await,
        "applied"
    );

    // The fix's moment: the home-synced handling re-stamped the demand because the wake stamp
    // was still live, and arm 5 claims exactly one start demand — no second click.
    let req: bool = sqlx::query_scalar(
        "SELECT preview_start_requested_at IS NOT NULL FROM sandboxes WHERE id = $1",
    )
    .bind(sb)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert!(req, "the home sync must re-present the start demand");
    let d = {
        let mut conn = r.h.pool().acquire().await.unwrap();
        aurora_db::repo::preview_shares::claim_start_demands(
            &mut conn,
            runner,
            r.rep,
            aurora_core::limits::PREVIEW_WAKE_WAIT.as_secs_f64(),
            10,
        )
        .await
        .unwrap()
    };
    assert_eq!(d.len(), 1, "the re-stamped demand must be claimable");
    assert_eq!(d[0].share_id, share);
    assert_eq!(d[0].agent_id, agent);
    assert_eq!(d[0].sandbox_id, sb);
    assert_eq!(d[0].boot_id, boot);
    // The full arm-5 path on the same state: the poll itself emits the `preview.start` item.
    let items = r.work().await;
    assert!(
        items
            .iter()
            .any(|i| i["kind"] == "preview.start" && i["sandbox_id"] == sb.to_string()),
        "the woken boot must run the share's start command without a second click: {items:?}"
    );

    // The negative control: an ordinary turn-caused boot (no wake stamp) presents no start
    // demand at the same home-synced moment.
    let agent2 = r.agent("prv-fix41-ctrl", true).await;
    let sb2 = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO sandboxes (id, agent_id, runner_id, state, boot_id) VALUES ($1,$2,$3,'stopped',$4)",
    )
    .bind(sb2)
    .bind(agent2)
    .bind(runner)
    .bind(Uuid::now_v7())
    .execute(r.h.pool())
    .await
    .unwrap();
    let (_, v) = r
        .create(
            agent2,
            json!({"port": 3000, "start_command": "npm run dev"}),
        )
        .await;
    let _: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let conv = r.direct(agent2).await;
    r.say(conv, "hello").await;
    let items = r.work().await;
    let start = items
        .iter()
        .find(|i| i["kind"] == "sandbox.start" && i["sandbox_id"] == sb2.to_string())
        .expect("the turn-caused boot must be claimed");
    let boot2: Uuid = start["boot_id"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        r.report(
            &live(agent2, conv, sb2, boot2),
            json!({"kind": "sandbox.status", "state": "running", "container_ref": "c"})
        )
        .await,
        "applied"
    );
    let (s, _) = r
        .runner(
            Some(r.rep),
            "/v1/runner/channel/hold",
            json!({"sandbox_id": sb2, "boot_id": boot2}),
        )
        .await;
    assert_eq!(s, 200);
    r.report(
        &live(agent2, conv, sb2, boot2),
        json!({"kind": "supervisor.ready", "version": "0.1",
            "harness": {"id": "fake", "capabilities": ["steer", "resume"]}}),
    )
    .await;
    let generation2 = desired_generation(r.h.pool(), sb2).await;
    assert_eq!(
        r.report(
            &live(agent2, conv, sb2, boot2),
            json!({"kind": "home.synced", "generation": generation2})
        )
        .await,
        "applied"
    );
    let mut conn = r.h.pool().acquire().await.unwrap();
    let d2 = aurora_db::repo::preview_shares::claim_start_demands(
        &mut conn,
        runner,
        r.rep,
        aurora_core::limits::PREVIEW_WAKE_WAIT.as_secs_f64(),
        10,
    )
    .await
    .unwrap();
    assert!(
        d2.iter().all(|d| d.sandbox_id != sb2),
        "an ordinary boot never presents a start demand: {d2:?}"
    );
}
