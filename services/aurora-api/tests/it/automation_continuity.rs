#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RFE-021 (SEC-51): reassigning schedules/webhooks, the offboarding view, `reassign_to`
//! atomicity and workspace-anchored automations.

use crate::common::*;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

struct Rig {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    agent: Uuid,
    /// Org owner (implicit `can_own`).
    admin: Uuid,
    /// Workspace owner by binding.
    owner: Uuid,
    /// The person who leaves.
    alice: Uuid,
    /// A colleague who can use the workspace.
    bob: Uuid,
    /// Org member without a binding on the workspace.
    carol: Uuid,
}

async fn rig() -> Rig {
    let h = Harness::new().await;
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let admin = h.user("admin@x.io", false).await;
    h.member(org, admin, "owner", "active").await;
    let mut ids = Vec::new();
    for (mail, role) in [
        ("owner@x.io", "owner"),
        ("alice@x.io", "member"),
        ("bob@x.io", "member"),
    ] {
        let u = h.user(mail, false).await;
        h.member(org, u, "member", "active").await;
        h.bind_ws(ws, u, role).await;
        ids.push(u);
    }
    let carol = h.user("carol@x.io", false).await;
    h.member(org, carol, "member", "active").await;
    let agent = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness)
         VALUES ($1,$2,$3,'a1','Agent One','hosted','fake')",
    )
    .bind(agent)
    .bind(org)
    .bind(ws)
    .execute(h.pool())
    .await
    .unwrap();
    Rig {
        h,
        org,
        ws,
        agent,
        admin,
        owner: ids[0],
        alice: ids[1],
        bob: ids[2],
        carol,
    }
}

impl Rig {
    async fn call(&self, who: Uuid, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        let (_, tok) = self.h.good_session(who).await;
        let (s, _, v) = self
            .h
            .send(
                m,
                uri,
                &[("cookie", &cookie(&tok)), ("origin", ORIGIN)],
                body,
            )
            .await;
        (s.as_u16(), v)
    }

    async fn schedule(&self, who: Uuid, name: &str, extra: Value) -> String {
        let mut b = json!({"name": name, "prompt": "p", "cron": "0 9 * * *", "timezone": "UTC"});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        let (s, v) = self
            .call(
                who,
                Method::POST,
                &format!("/v1/agents/{}/schedules", self.agent),
                Some(b),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        v["id"].as_str().unwrap().to_owned()
    }

    async fn webhook(&self, who: Uuid, name: &str, extra: Value) -> (String, String) {
        let mut b = json!({"name": name, "instructions": "i"});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        let (s, v) = self
            .call(
                who,
                Method::POST,
                &format!("/v1/agents/{}/webhooks", self.agent),
                Some(b),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        (
            v["webhook"]["id"].as_str().unwrap().to_owned(),
            v["url"].as_str().unwrap().to_owned(),
        )
    }

    async fn authority(
        &self,
        table: &str,
        id: &str,
    ) -> (Option<Uuid>, String, bool, Option<String>) {
        sqlx::query_as(&format!(
            "SELECT creator_user_id, anchor, enabled, disabled_reason FROM {table} WHERE id = $1::uuid"
        ))
        .bind(id)
        .fetch_one(self.h.pool())
        .await
        .unwrap()
    }

    async fn audits(&self) -> Vec<Value> {
        sqlx::query_scalar(
            "SELECT metadata FROM audit_log WHERE action = 'automation.reassign' ORDER BY created_at, id",
        )
        .fetch_all(self.h.pool())
        .await
        .unwrap()
    }

    /// Make every schedule due and run arm 9 once.
    async fn fire_due(&self) {
        sqlx::query("UPDATE schedules SET next_fire_at = now() - interval '2 days' WHERE enabled")
            .execute(self.h.pool())
            .await
            .unwrap();
        let mut tx = self.h.pool().begin().await.unwrap();
        aurora_api::domains::schedules::fire::arm9(&self.h.state, &mut tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    async fn last_run(&self, id: &str) -> Option<String> {
        sqlx::query_scalar(
            "SELECT outcome FROM schedule_runs WHERE schedule_id = $1::uuid
             ORDER BY created_at DESC, id DESC LIMIT 1",
        )
        .bind(id)
        .fetch_optional(self.h.pool())
        .await
        .unwrap()
    }

    async fn deliver(&self, url: &str) -> u16 {
        use http_body_util::BodyExt;
        use tower::ServiceExt;
        let path = url.split_once("/v1/").map_or(url, |(_, p)| p);
        let req = http::Request::builder()
            .method(Method::POST)
            .uri(format!("/v1/{path}"))
            .body(axum::body::Body::from("{}"))
            .unwrap();
        let res = self.h.app.clone().oneshot(req).await.unwrap();
        let status = res.status().as_u16();
        let _ = res.into_body().collect().await;
        status
    }
}

#[tokio::test]
async fn reassign_is_can_own_only_to_an_authorized_member_and_audited() {
    let r = rig().await;
    let id = r.schedule(r.alice, "daily", json!({})).await;
    let wid = r.webhook(r.alice, "hook", json!({})).await.0;
    let url = |kind: &str| {
        format!(
            "/v1/{kind}/{}/creator",
            if kind == "schedules" { &id } else { &wid }
        )
    };
    for kind in ["schedules", "webhooks"] {
        // A plain member (even the creator) cannot reassign.
        let (s, _) = r
            .call(
                r.alice,
                Method::PUT,
                &url(kind),
                Some(json!({"user_id": r.bob})),
            )
            .await;
        assert_eq!(s, 403, "{kind}");
        // Targets that cannot use the workspace: unbound org member, stranger.
        for bad in [r.carol, Uuid::now_v7()] {
            let (s, v) = r
                .call(
                    r.owner,
                    Method::PUT,
                    &url(kind),
                    Some(json!({"user_id": bad})),
                )
                .await;
            assert_eq!(
                (s, v["error"]["code"].as_str()),
                (422, Some("new_creator_unauthorized")),
                "{v}"
            );
        }
        // A suspended member is not eligible either.
        r.h.set_member_status(r.org, r.bob, "suspended").await;
        let (s, _) = r
            .call(
                r.owner,
                Method::PUT,
                &url(kind),
                Some(json!({"user_id": r.bob})),
            )
            .await;
        assert_eq!(s, 422);
        r.h.set_member_status(r.org, r.bob, "active").await;
    }
    assert!(
        r.audits().await.is_empty(),
        "refused attempts change nothing"
    );
    assert_eq!(r.authority("schedules", &id).await.0, Some(r.alice));

    let (s, v) = r
        .call(
            r.owner,
            Method::PUT,
            &url("schedules"),
            Some(json!({"user_id": r.bob})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["created_by"]["id"], json!(r.bob));
    assert_eq!(v["anchor"], "creator");
    let (s, v) = r
        .call(
            r.admin,
            Method::PUT,
            &url("webhooks"),
            Some(json!({"user_id": r.bob})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(r.authority("webhooks", &wid).await.0, Some(r.bob));
    let a = r.audits().await;
    assert_eq!(a.len(), 2);
    assert_eq!(a[0]["kind"], "schedule");
    assert_eq!(a[0]["id"], json!(id));
    assert_eq!(a[0]["old_creator"], json!(r.alice));
    assert_eq!(a[0]["new_creator"], json!(r.bob));
    assert_eq!(a[1]["kind"], "webhook");
}

#[tokio::test]
async fn reassign_can_re_enable_an_item_disabled_for_authorization() {
    let r = rig().await;
    let id = r.schedule(r.alice, "daily", json!({})).await;
    let (wid, _) = r.webhook(r.alice, "hook", json!({})).await;
    sqlx::query("UPDATE schedules SET enabled=false, disabled_reason='authorization', next_fire_at=NULL, consecutive_failures=3")
        .execute(r.h.pool()).await.unwrap();
    sqlx::query("UPDATE webhooks SET enabled=false, disabled_reason='authorization'")
        .execute(r.h.pool())
        .await
        .unwrap();
    // Without `re_enable` the item stays disabled.
    let (s, _) = r
        .call(
            r.owner,
            Method::PUT,
            &format!("/v1/schedules/{id}/creator"),
            Some(json!({"user_id": r.bob})),
        )
        .await;
    assert_eq!(s, 200);
    let (_, _, en, why) = r.authority("schedules", &id).await;
    assert_eq!((en, why.as_deref()), (false, Some("authorization")));
    for (kind, id) in [("schedules", &id), ("webhooks", &wid)] {
        let (s, v) = r
            .call(
                r.owner,
                Method::PUT,
                &format!("/v1/{kind}/{id}/creator"),
                Some(json!({"user_id": r.bob, "re_enable": true})),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["enabled"], true);
        assert!(v["disabled_reason"].is_null());
    }
    let (next, fails): (Option<chrono::DateTime<chrono::Utc>>, i32) = sqlx::query_as(
        "SELECT next_fire_at, consecutive_failures FROM schedules WHERE id=$1::uuid",
    )
    .bind(&id)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert!(next.is_some_and(|n| n > chrono::Utc::now()));
    assert_eq!(fails, 0);
}

#[tokio::test]
async fn bulk_reassign_is_atomic_and_audited_per_item() {
    let r = rig().await;
    let s1 = r.schedule(r.alice, "s1", json!({})).await;
    let s2 = r.schedule(r.alice, "s2", json!({})).await;
    let (w1, _) = r.webhook(r.alice, "w1", json!({})).await;
    let other = r.schedule(r.bob, "bobs", json!({})).await;
    let uri = format!("/v1/workspaces/{}/automations/reassign", r.ws);
    // Members cannot; an unauthorized target changes nothing.
    let (s, _) = r
        .call(
            r.alice,
            Method::POST,
            &uri,
            Some(json!({"from_user_id": r.alice, "to": r.bob})),
        )
        .await;
    assert_eq!(s, 403);
    let (s, v) = r
        .call(
            r.owner,
            Method::POST,
            &uri,
            Some(json!({"from_user_id": r.alice, "to": r.carol})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("new_creator_unauthorized"))
    );
    assert_eq!(r.authority("schedules", &s1).await.0, Some(r.alice));
    assert!(r.audits().await.is_empty());
    // Kinds filter.
    let (s, v) = r
        .call(
            r.owner,
            Method::POST,
            &uri,
            Some(json!({"from_user_id": r.alice, "to": r.bob, "kinds": ["webhook"]})),
        )
        .await;
    assert_eq!((s, v["reassigned"].as_i64()), (200, Some(1)), "{v}");
    assert_eq!(r.authority("webhooks", &w1).await.0, Some(r.bob));
    assert_eq!(r.authority("schedules", &s1).await.0, Some(r.alice));
    // The rest, in one call, one audit row per item.
    let (s, v) = r
        .call(
            r.owner,
            Method::POST,
            &uri,
            Some(json!({"from_user_id": r.alice, "to": r.bob})),
        )
        .await;
    assert_eq!((s, v["reassigned"].as_i64()), (200, Some(2)), "{v}");
    for id in [&s1, &s2, &other] {
        assert_eq!(r.authority("schedules", id).await.0, Some(r.bob));
    }
    assert_eq!(r.audits().await.len(), 3);
    // To the workspace: anchor changes, creator stays as provenance.
    let (s, v) = r
        .call(
            r.owner,
            Method::POST,
            &uri,
            Some(json!({"from_user_id": r.bob, "to": "workspace", "ids": [s1]})),
        )
        .await;
    assert_eq!((s, v["reassigned"].as_i64()), (200, Some(1)), "{v}");
    let (c, anchor, ..) = r.authority("schedules", &s1).await;
    assert_eq!((c, anchor.as_str()), (Some(r.bob), "workspace"));
    let last = r.audits().await.pop().unwrap();
    assert_eq!(last["to"], "workspace");
    assert_eq!(last["new_anchor"], "workspace");
}

#[tokio::test]
async fn reassign_to_commits_or_rolls_back_with_member_removal() {
    let r = rig().await;
    let s1 = r.schedule(r.alice, "s1", json!({})).await;
    let (w1, _) = r.webhook(r.alice, "w1", json!({})).await;
    let del = |to: String| format!("/v1/orgs/{}/members/{}?reassign_to={to}", r.org, r.alice);
    // carol cannot use the workspace: 422, and the removal did not happen either.
    let (s, v) = r
        .call(r.admin, Method::DELETE, &del(r.carol.to_string()), None)
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("new_creator_unauthorized")),
        "{v}"
    );
    let member: i64 = sqlx::query_scalar("SELECT count(*) FROM org_members WHERE user_id=$1")
        .bind(r.alice)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(member, 1);
    assert_eq!(r.authority("schedules", &s1).await.0, Some(r.alice));
    assert!(r.audits().await.is_empty());
    // Not reassigning to the person being removed; malformed target.
    let (s, _) = r
        .call(r.admin, Method::DELETE, &del(r.alice.to_string()), None)
        .await;
    assert_eq!(s, 400);
    let (s, _) = r
        .call(r.admin, Method::DELETE, &del("nobody".into()), None)
        .await;
    assert_eq!(s, 400);
    // An unknown user is refused even though the person has automations elsewhere too.
    let (s, _) = r
        .call(
            r.admin,
            Method::DELETE,
            &del(Uuid::now_v7().to_string()),
            None,
        )
        .await;
    assert_eq!(s, 422);
    // Success: removal and reassignment together.
    let (s, v) = r
        .call(r.admin, Method::DELETE, &del(r.bob.to_string()), None)
        .await;
    assert_eq!(s, 204, "{v}");
    assert_eq!(r.authority("schedules", &s1).await.0, Some(r.bob));
    assert_eq!(r.authority("webhooks", &w1).await.0, Some(r.bob));
    let member: i64 = sqlx::query_scalar("SELECT count(*) FROM org_members WHERE user_id=$1")
        .bind(r.alice)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(member, 0);
    assert_eq!(r.audits().await.len(), 2);
}

#[tokio::test]
async fn reassign_to_workspace_on_suspension_and_binding_removal() {
    let r = rig().await;
    let s1 = r.schedule(r.alice, "s1", json!({})).await;
    // reassign_to only makes sense when suspending.
    let patch = format!("/v1/orgs/{}/members/{}", r.org, r.alice);
    let (s, _) = r
        .call(
            r.admin,
            Method::PATCH,
            &patch,
            Some(json!({"role": "member", "reassign_to": "workspace"})),
        )
        .await;
    assert_eq!(s, 400);
    let (s, v) = r
        .call(
            r.admin,
            Method::PATCH,
            &patch,
            Some(json!({"status": "suspended", "reassign_to": "workspace"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (c, anchor, en, _) = r.authority("schedules", &s1).await;
    assert_eq!((c, anchor.as_str(), en), (Some(r.alice), "workspace", true));
    // Binding removal (bob) reassigns to the workspace owner and rolls back on a bad target.
    let s2 = r.schedule(r.bob, "s2", json!({})).await;
    let del = |to: Uuid| format!("/v1/workspaces/{}/access/{}?reassign_to={to}", r.ws, r.bob);
    let (s, v) = r.call(r.owner, Method::DELETE, &del(r.carol), None).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("new_creator_unauthorized")),
        "{v}"
    );
    let bound: i64 = sqlx::query_scalar("SELECT count(*) FROM workspace_access WHERE user_id=$1")
        .bind(r.bob)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(bound, 1);
    assert_eq!(r.authority("schedules", &s2).await.0, Some(r.bob));
    let (s, v) = r.call(r.owner, Method::DELETE, &del(r.owner), None).await;
    assert_eq!(s, 204, "{v}");
    assert_eq!(r.authority("schedules", &s2).await.0, Some(r.owner));
}

#[tokio::test]
async fn offboarding_view_matches_what_the_removal_disables() {
    let r = rig().await;
    let s_on = r.schedule(r.alice, "on", json!({})).await;
    let s_off = r.schedule(r.alice, "off", json!({})).await;
    sqlx::query("UPDATE schedules SET enabled=false, disabled_reason='user' WHERE id=$1::uuid")
        .bind(&s_off)
        .execute(r.h.pool())
        .await
        .unwrap();
    let s_anch = r
        .schedule(r.owner, "anch", json!({"anchor": "workspace"}))
        .await;
    let (w_on, w_url) = r.webhook(r.alice, "hook", json!({})).await;
    let _bobs = r.schedule(r.bob, "bobs", json!({})).await;
    let org_view = format!("/v1/orgs/{}/members/{}/offboarding", r.org, r.alice);
    let ws_view = format!("/v1/workspaces/{}/members/{}/offboarding", r.ws, r.alice);
    // Authorization: org view needs org owner/admin, workspace view `can_own`.
    assert_eq!(r.call(r.owner, Method::GET, &org_view, None).await.0, 403);
    assert_eq!(r.call(r.alice, Method::GET, &ws_view, None).await.0, 403);
    assert_eq!(
        r.call(
            r.admin,
            Method::GET,
            &format!("/v1/orgs/{}/members/{}/offboarding", r.org, Uuid::now_v7()),
            None
        )
        .await
        .0,
        404
    );
    sqlx::query("INSERT INTO api_keys (id, org_id, user_id, kind, name, token_hash, display_prefix) VALUES ($1,$2,$3,'user','k',sha256('k'::bytea),'auk_')")
        .bind(Uuid::now_v7()).bind(r.org).bind(r.alice).execute(r.h.pool()).await.unwrap();
    // An org-wide key survives losing one workspace binding; the org view counts it.
    for (who, uri, keys) in [(r.admin, &org_view, 1), (r.owner, &ws_view, 0)] {
        let (s, v) = r.call(who, Method::GET, uri, None).await;
        assert_eq!(s, 200, "{v}");
        let sched: Vec<&str> = v["schedules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            sched,
            [s_on.as_str()],
            "only enabled, creator-anchored items: {v}"
        );
        assert_eq!(v["webhooks"][0]["id"], json!(w_on));
        assert_eq!(v["schedules"][0]["would_disable"], true);
        assert_eq!(v["schedules"][0]["agent"]["id"], json!(r.agent));
        assert_eq!(v["api_keys"], keys);
        assert_eq!(v["peer_tasks"].as_array().unwrap().len(), 0);
    }
    // Do the removal without reassigning: exactly the listed items stop.
    let (s, _) = r
        .call(
            r.admin,
            Method::DELETE,
            &format!("/v1/orgs/{}/members/{}", r.org, r.alice),
            None,
        )
        .await;
    assert_eq!(s, 204);
    r.fire_due().await;
    assert_eq!(
        r.last_run(&s_on).await.as_deref(),
        Some("skipped_unauthorized")
    );
    assert_eq!(
        r.authority("schedules", &s_on).await.3.as_deref(),
        Some("authorization")
    );
    assert_eq!(
        r.last_run(&s_anch).await.as_deref(),
        Some("fired"),
        "anchored item is unaffected"
    );
    assert_eq!(r.deliver(&w_url).await, 403);
    assert_eq!(
        r.authority("webhooks", &w_on).await.3.as_deref(),
        Some("authorization")
    );
    // Admins have implicit access that a binding removal cannot take away: nothing listed.
    let (s, v) = r
        .call(
            r.owner,
            Method::GET,
            &format!("/v1/workspaces/{}/members/{}/offboarding", r.ws, r.admin),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["schedules"].as_array().unwrap().len(), 0);
}

/// RFE-006 / FU-P2-021a: schedules and webhooks of a frozen agent stop — creator-anchored and
/// workspace-anchored alike — without being disabled or counted as an authority failure, and
/// resume on unfreeze.
#[tokio::test]
async fn schedules_and_webhooks_stop_while_the_agent_is_frozen_whatever_the_anchor() {
    let r = rig().await;
    let s_creator = r.schedule(r.alice, "by-creator", json!({})).await;
    let s_anchor = r
        .schedule(r.owner, "by-anchor", json!({"anchor": "workspace"}))
        .await;
    let (w_creator, url_creator) = r.webhook(r.alice, "hook-c", json!({})).await;
    let (w_anchor, url_anchor) = r
        .webhook(r.owner, "hook-a", json!({"anchor": "workspace"}))
        .await;
    let turns = |r: &Rig| {
        let pool = r.h.pool().clone();
        let agent = r.agent;
        async move {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM turns WHERE agent_id = $1")
                .bind(agent)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    // Freezing the workspace (not the agent) must stop both anchors.
    let (s, v) = r
        .call(
            r.owner,
            Method::POST,
            &format!("/v1/workspaces/{}/freeze", r.ws),
            Some(json!({"reason": "drill"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    r.fire_due().await;
    for id in [&s_creator, &s_anchor] {
        assert_eq!(
            r.last_run(id).await.as_deref(),
            Some("skipped_frozen"),
            "{id}"
        );
        let (enabled, reason, outcome, next_in_future): (
            bool,
            Option<String>,
            Option<String>,
            bool,
        ) = sqlx::query_as(
            "SELECT enabled, disabled_reason, last_outcome, next_fire_at > now()
                 FROM schedules WHERE id = $1::uuid",
        )
        .bind(id)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
        assert!(
            enabled && reason.is_none(),
            "frozen is not an authority failure"
        );
        assert_eq!(outcome.as_deref(), Some("skipped_frozen"));
        assert!(next_in_future, "next_fire_at advances: no catch-up burst");
    }
    assert_eq!(turns(&r).await, 0);
    for (url, id, table) in [
        (&url_creator, &w_creator, "webhooks"),
        (&url_anchor, &w_anchor, "webhooks"),
    ] {
        assert_eq!(r.deliver(url).await, 410);
        let (enabled, reason) = {
            let a = r.authority(table, id).await;
            (a.2, a.3)
        };
        assert!(enabled && reason.is_none(), "the webhook stays enabled");
        let (status, why): (String, Option<String>) = sqlx::query_as(
            "SELECT status, reason FROM webhook_deliveries WHERE webhook_id = $1::uuid
             ORDER BY created_at DESC, id DESC LIMIT 1",
        )
        .bind(id)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
        assert_eq!(
            (status.as_str(), why.as_deref()),
            ("paused", Some("frozen"))
        );
    }
    assert_eq!(turns(&r).await, 0);
    // Unfreeze: both anchors fire again.
    let (s, _) = r
        .call(
            r.owner,
            Method::POST,
            &format!("/v1/workspaces/{}/unfreeze", r.ws),
            None,
        )
        .await;
    assert_eq!(s, 200);
    sqlx::query("UPDATE schedules SET next_fire_at = now() - interval '1 day'")
        .execute(r.h.pool())
        .await
        .unwrap();
    // The run conversations are separate, so both can fire in the same pass.
    r.fire_due().await;
    assert_eq!(r.last_run(&s_creator).await.as_deref(), Some("fired"));
    assert_eq!(r.last_run(&s_anchor).await.as_deref(), Some("fired"));
    assert_eq!(r.deliver(&url_creator).await, 202);
    assert_eq!(r.deliver(&url_anchor).await, 202);
}

#[tokio::test]
async fn anchored_automations_need_an_owner_and_outlive_their_creator() {
    let r = rig().await;
    // Only `can_own` may create or switch an anchor.
    let uri = format!("/v1/agents/{}/schedules", r.agent);
    let body = json!({"name": "x", "prompt": "p", "cron": "0 9 * * *", "timezone": "UTC", "anchor": "workspace"});
    assert_eq!(r.call(r.alice, Method::POST, &uri, Some(body)).await.0, 403);
    let hooks = format!("/v1/agents/{}/webhooks", r.agent);
    let hb = json!({"name": "x", "instructions": "i", "anchor": "workspace"});
    assert_eq!(r.call(r.alice, Method::POST, &hooks, Some(hb)).await.0, 403);
    let bad = json!({"name": "x", "prompt": "p", "cron": "0 9 * * *", "anchor": "nope"});
    assert_eq!(r.call(r.owner, Method::POST, &uri, Some(bad)).await.0, 400);

    let mine = r.schedule(r.alice, "mine", json!({})).await;
    let patch = format!("/v1/schedules/{mine}");
    assert_eq!(
        r.call(
            r.alice,
            Method::PATCH,
            &patch,
            Some(json!({"anchor": "workspace"}))
        )
        .await
        .0,
        403
    );
    let (s, v) = r
        .call(
            r.owner,
            Method::PATCH,
            &patch,
            Some(json!({"anchor": "workspace"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["anchor"], "workspace");
    assert_eq!(
        v["created_by"]["id"],
        json!(r.alice),
        "creator stays as provenance"
    );
    let a = r.audits().await;
    assert_eq!(
        (a[0]["to"].as_str(), a[0]["new_anchor"].as_str()),
        (Some("workspace"), Some("workspace"))
    );
    let (_, wurl) = r
        .webhook(r.owner, "anch-hook", json!({"anchor": "workspace"}))
        .await;

    // The creator leaves: the anchored items keep working.
    r.h.set_member_status(r.org, r.alice, "suspended").await;
    r.fire_due().await;
    assert_eq!(r.last_run(&mine).await.as_deref(), Some("fired"));
    assert_ne!(r.deliver(&wurl).await, 403);
    // Even the hook's creator (a workspace owner) can go; any other active owner suffices.
    r.h.set_member_status(r.org, r.owner, "suspended").await;
    r.fire_due().await;
    assert!(
        r.authority("schedules", &mine).await.2,
        "the org owner remains"
    );
    // With every owner suspended, nobody is left to answer for the item.
    r.h.set_member_status(r.org, r.admin, "suspended").await;
    sqlx::query("UPDATE schedules SET next_fire_at = now() - interval '2 days'")
        .execute(r.h.pool())
        .await
        .unwrap();
    r.fire_due().await;
    assert_eq!(
        r.last_run(&mine).await.as_deref(),
        Some("skipped_unauthorized")
    );
    assert_eq!(
        r.authority("schedules", &mine).await.3.as_deref(),
        Some("authorization")
    );
    assert_eq!(r.deliver(&wurl).await, 403);

    // Back to creator anchoring needs a creator who still has access.
    r.h.set_member_status(r.org, r.admin, "active").await;
    let (s, v) = r
        .call(
            r.admin,
            Method::PATCH,
            &patch,
            Some(json!({"anchor": "creator"})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("new_creator_unauthorized")),
        "{v}"
    );
}
