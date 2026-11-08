#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P2-011A (RFE-011a, D-56, SEC-49): approval e-mail notifications, watchers, the opt-out and the
//! sanitized stream notice.

use crate::approvals::{agent, fixture};
use crate::common::*;
use aurora_api::domains::approvals::notify::notify_once;
use aurora_api::domains::auth::mail::MemoryMailer;
use http::{Method, Request};
use serde_json::json;
use std::collections::BTreeSet;
use std::time::Duration;
use tower::ServiceExt;
use uuid::Uuid;

async fn person(
    h: &Harness,
    f: &crate::approvals::Fx,
    email: &str,
    org_role: &str,
    bind: Option<&str>,
) -> Uuid {
    let id = h.user(email, false).await;
    h.member(f.org, id, org_role, "active").await;
    if let Some(b) = bind {
        h.bind_ws(f.ws, id, b).await;
    }
    id
}

async fn titled(
    h: &Harness,
    f: &crate::approvals::Fx,
    batch: Option<&str>,
    conv: Option<Uuid>,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO approvals (id, org_id, workspace_id, agent_id, gateway_instance_id, conversation_id, method, host, path,
            summary, batch_id, rule_name, status, deadline)
         VALUES ($1,$2,$3,$4,$5,$6,'POST','api.example.com','/x',
            '{\"title\":\"Deploy to prod\",\"fields\":[{\"label\":\"k\",\"value\":\"SECRETFIELD\"}],\"text\":\"BODYTEXT\",\"link\":null,\"group\":null,\"source\":\"generic\"}',
            $7,'Ask first','pending', now() + interval '10 minutes')",
    )
    .bind(id).bind(f.org).bind(f.ws).bind(f.agent).bind(f.gw).bind(conv).bind(batch)
    .execute(h.pool()).await.unwrap();
    id
}

async fn age(h: &Harness) {
    sqlx::query("UPDATE approvals SET created_at = now() - interval '30 seconds'")
        .execute(h.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE action_approvals SET created_at = now() - interval '30 seconds'")
        .execute(h.pool())
        .await
        .unwrap();
}

fn recipients(h: &Harness) -> BTreeSet<String> {
    h.mail.sent().into_iter().map(|m| m.to).collect()
}

#[tokio::test]
async fn mail_goes_to_explicit_deciders_after_the_delay_with_title_and_link_only() {
    let h = Harness::new().await;
    let f = fixture(&h).await;
    person(&h, &f, "owner@x.io", "member", Some("owner")).await;
    person(&h, &f, "appr@x.io", "member", Some("approver")).await;
    person(&h, &f, "plain@x.io", "member", Some("member")).await;
    person(&h, &f, "admin@x.io", "admin", None).await;
    let id = titled(&h, &f, None, None).await;
    assert_eq!(
        notify_once(&h.state).await.unwrap(),
        0,
        "not before the delay"
    );
    age(&h).await;
    assert_eq!(notify_once(&h.state).await.unwrap(), 2);
    assert_eq!(
        recipients(&h),
        BTreeSet::from(["owner@x.io".to_owned(), "appr@x.io".to_owned()])
    );
    let m = &h.mail.sent()[0];
    let all = format!("{}\n{}", m.subject, m.body);
    assert!(all.contains("Deploy to prod") && all.contains("Ask first"));
    for leak in ["SECRETFIELD", "BODYTEXT", "/decision", "deny", "token"] {
        assert!(!all.contains(leak), "{leak} leaked: {all}");
    }
    let slug = f.org.simple().to_string();
    assert_eq!(
        m.link,
        format!(
            "{}/o/{slug}/w/{}/approvals?focus={id}",
            h.state.config.external_url.trim_end_matches('/'),
            f.ws.simple()
        )
    );
    assert_eq!(notify_once(&h.state).await.unwrap(), 0, "idempotent");
}

#[tokio::test]
async fn org_owners_and_admins_when_the_workspace_has_no_explicit_decider() {
    let h = Harness::new().await;
    let f = fixture(&h).await;
    person(&h, &f, "plain@x.io", "member", Some("member")).await;
    person(&h, &f, "admin@x.io", "admin", None).await;
    person(&h, &f, "own@x.io", "owner", None).await;
    titled(&h, &f, None, None).await;
    age(&h).await;
    notify_once(&h.state).await.unwrap();
    assert_eq!(
        recipients(&h),
        BTreeSet::from(["admin@x.io".to_owned(), "own@x.io".to_owned()])
    );
}

#[tokio::test]
async fn a_fresh_watcher_is_a_live_surface_a_stale_one_is_not() {
    let h = Harness::new().await;
    let f = fixture(&h).await;
    let appr = person(&h, &f, "appr@x.io", "member", Some("approver")).await;
    let plain = person(&h, &f, "plain@x.io", "member", Some("member")).await;
    titled(&h, &f, None, None).await;
    age(&h).await;
    // A watcher who cannot decide does not count.
    sqlx::query("INSERT INTO approval_watchers (replica_id, user_id, workspace_id, last_seen_at) VALUES ($1,$2,$3,now())")
        .bind(Uuid::now_v7()).bind(plain).bind(f.ws).execute(h.pool()).await.unwrap();
    // A decider's fresh watcher does.
    let rep = Uuid::now_v7();
    sqlx::query("INSERT INTO approval_watchers (replica_id, user_id, workspace_id, last_seen_at) VALUES ($1,$2,$3,now())")
        .bind(rep).bind(appr).bind(f.ws).execute(h.pool()).await.unwrap();
    assert_eq!(notify_once(&h.state).await.unwrap(), 0);
    sqlx::query("UPDATE approval_watchers SET last_seen_at = now() - interval '60 seconds' WHERE replica_id = $1")
        .bind(rep).execute(h.pool()).await.unwrap();
    assert_eq!(notify_once(&h.state).await.unwrap(), 1);
}

#[tokio::test]
async fn a_delivered_channel_card_is_a_live_surface() {
    let h = Harness::new().await;
    let f = fixture(&h).await;
    person(&h, &f, "appr@x.io", "member", Some("approver")).await;
    let id = titled(&h, &f, None, None).await;
    age(&h).await;
    let integ = Uuid::now_v7();
    sqlx::query("INSERT INTO connectors (id, token_hash, display_name, manifest, status) VALUES ('chat', $1, 'chat', '{}', 'active')")
        .bind(vec![0u8; 32]).execute(h.pool()).await.unwrap();
    sqlx::query("INSERT INTO channel_integrations (id, org_id, connector_id, tenant_ref, tenant_label, config, credentials_ref, status) VALUES ($1,$2,'chat','T','T','{}','x','active')")
        .bind(integ).bind(f.org).execute(h.pool()).await.unwrap();
    let op = Uuid::now_v7();
    sqlx::query("INSERT INTO channel_ops (id, connector_id, integration_id, kind, payload, dedupe_key, status) VALUES ($1,'chat',$2,'message.post','{}','k1','failed')")
        .bind(op).bind(integ).execute(h.pool()).await.unwrap();
    sqlx::query("INSERT INTO channel_cards (id, subject_kind, subject_id, integration_id, post_op_id, space_ref, thread_ref, state) VALUES ($1,'approval',$2,$3,$4,'C','t','posting')")
        .bind(Uuid::now_v7()).bind(id).bind(integ).bind(op).execute(h.pool()).await.unwrap();
    // The post failed: no surface, mail goes out.
    // (Use a second approval so the first sweep's claim does not interfere.)
    sqlx::query("UPDATE channel_ops SET status = 'done' WHERE id = $1")
        .bind(op)
        .execute(h.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE channel_cards SET state = 'posted'")
        .execute(h.pool())
        .await
        .unwrap();
    assert_eq!(notify_once(&h.state).await.unwrap(), 0, "card delivered");
}

#[tokio::test]
async fn opt_out_instance_switch_and_me_patch() {
    let h = Harness::new().await;
    let f = fixture(&h).await;
    let a = person(&h, &f, "a@x.io", "member", Some("approver")).await;
    person(&h, &f, "b@x.io", "member", Some("approver")).await;
    let (_, tok) = h.good_session(a).await;
    let (s, _, v) = h
        .send(
            Method::PATCH,
            "/v1/me",
            &[("cookie", &cookie(&tok)), ("origin", ORIGIN)],
            Some(json!({"approval_email": false})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["approval_email"], false);
    let (_, _, v) = h
        .send(Method::GET, "/v1/me", &[("cookie", &cookie(&tok))], None)
        .await;
    assert_eq!(v["approval_email"], false);
    titled(&h, &f, None, None).await;
    age(&h).await;
    notify_once(&h.state).await.unwrap();
    assert_eq!(recipients(&h), BTreeSet::from(["b@x.io".to_owned()]));

    // Instance switch off: nothing is sent.
    let off = Harness::with_config(|c| c.approvals.email_notifications = false).await;
    let f2 = fixture(&off).await;
    person(&off, &f2, "c@x.io", "member", Some("approver")).await;
    titled(&off, &f2, None, None).await;
    age(&off).await;
    assert_eq!(notify_once(&off.state).await.unwrap(), 0);
    assert!(off.mail.sent().is_empty());
}

#[tokio::test]
async fn direct_conversations_reach_only_those_who_can_see_them() {
    let h = Harness::new().await;
    let f = fixture(&h).await;
    let du = person(&h, &f, "du@x.io", "member", Some("approver")).await;
    person(&h, &f, "other@x.io", "member", Some("approver")).await;
    person(&h, &f, "own@x.io", "member", Some("owner")).await;
    let conv = Uuid::now_v7();
    sqlx::query("INSERT INTO conversations (id, workspace_id, agent_id, kind, direct_user_id, external_ref, last_activity_at) VALUES ($1,$2,$3,'direct',$4,$5, now())")
        .bind(conv).bind(f.ws).bind(f.agent).bind(du).bind(format!("d:{conv}"))
        .execute(h.pool()).await.unwrap();
    titled(&h, &f, None, Some(conv)).await;
    age(&h).await;
    notify_once(&h.state).await.unwrap();
    assert_eq!(
        recipients(&h),
        BTreeSet::from(["du@x.io".to_owned(), "own@x.io".to_owned()])
    );
}

#[tokio::test]
async fn a_batch_produces_one_mail() {
    let h = Harness::new().await;
    let f = fixture(&h).await;
    person(&h, &f, "a@x.io", "member", Some("approver")).await;
    for _ in 0..3 {
        titled(&h, &f, Some("b1"), None).await;
    }
    age(&h).await;
    assert_eq!(notify_once(&h.state).await.unwrap(), 1);
    assert!(h.mail.sent()[0].body.contains("and 2 more"));
    assert_eq!(notify_once(&h.state).await.unwrap(), 0);
}

#[tokio::test]
async fn hourly_cap_folds_the_rest_into_one_digest() {
    let h = Harness::new().await;
    let f = fixture(&h).await;
    person(&h, &f, "a@x.io", "member", Some("approver")).await;
    for i in 0..23 {
        let ag = agent(&h, f.org, f.ws, &format!("bot{i}")).await;
        let _ = ag;
        titled(&h, &f, None, None).await;
    }
    age(&h).await;
    notify_once(&h.state).await.unwrap();
    let sent = h.mail.sent();
    assert_eq!(sent.len(), 21, "20 mails and one digest");
    assert!(
        sent.iter()
            .filter(|m| m.subject.contains("more approval"))
            .count()
            == 1
    );
    assert_eq!(notify_once(&h.state).await.unwrap(), 0);
}

#[tokio::test]
async fn two_replicas_send_each_mail_once() {
    let h = Harness::new().await;
    let f = fixture(&h).await;
    person(&h, &f, "a@x.io", "member", Some("approver")).await;
    person(&h, &f, "b@x.io", "member", Some("owner")).await;
    for _ in 0..5 {
        titled(&h, &f, None, None).await;
    }
    age(&h).await;
    let m2 = MemoryMailer::new();
    let s2 =
        aurora_api::app::AppState::new(h.pool().clone(), config(|_| {})).with_mailer(m2.clone());
    let (x, y) = tokio::join!(notify_once(&h.state), notify_once(&s2));
    x.unwrap();
    y.unwrap();
    assert_eq!(
        h.mail.sent().len() + m2.sent().len(),
        10,
        "5 approvals x 2 deciders, once each"
    );
}

#[tokio::test]
async fn action_approvals_are_mailed_too() {
    let h = Harness::new().await;
    let f = fixture(&h).await;
    person(&h, &f, "a@x.io", "member", Some("approver")).await;
    sqlx::query("INSERT INTO action_approvals (id, workspace_id, agent_id, action_key, payload, summary, status, expires_at) VALUES ($1,$2,$3,'send_message','{\"text\":\"BODYTEXT\"}','Send a message to #ops','pending', now() + interval '1 hour')")
        .bind(Uuid::now_v7()).bind(f.ws).bind(f.agent).execute(h.pool()).await.unwrap();
    age(&h).await;
    assert_eq!(notify_once(&h.state).await.unwrap(), 1);
    let m = &h.mail.sent()[0];
    assert!(m.body.contains("Send a message to #ops") && !m.body.contains("BODYTEXT"));
}

#[tokio::test]
async fn stream_registers_a_watcher_and_created_carries_only_the_notice() {
    let h = Harness::new().await;
    let f = fixture(&h).await;
    let u = person(&h, &f, "a@x.io", "member", Some("approver")).await;
    let (_, tok) = h.good_session(u).await;
    let app = test_router(
        h.state
            .clone()
            .with_sse_keepalive(Duration::from_millis(200)),
    );
    let res = app
        .oneshot(
            Request::builder()
                .uri("/v1/approvals/stream")
                .header("cookie", cookie(&tok))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM approval_watchers WHERE user_id=$1 AND workspace_id=$2 AND last_seen_at > now() - interval '45 seconds'")
        .bind(u).bind(f.ws).fetch_one(h.pool()).await.unwrap();
    assert_eq!(n, 1, "an open stream is a watcher");
    let id = titled(&h, &f, None, None).await;
    aurora_db::bus::notify(
        h.pool(),
        aurora_db::bus::Channel::Approval,
        &json!({"id": id, "workspace_id": f.ws, "event": "created"}),
    )
    .await
    .unwrap();
    let mut body = res.into_body();
    let mut seen = String::new();
    while !seen.contains("\"notice\"") {
        use http_body_util::BodyExt;
        let fr = tokio::time::timeout(Duration::from_secs(5), body.frame())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let Some(d) = fr.data_ref() {
            seen.push_str(&String::from_utf8_lossy(d));
        }
    }
    assert!(
        seen.contains("\"kind\":\"gateway\"") && seen.contains("\"expires_at\""),
        "{seen}"
    );
}
