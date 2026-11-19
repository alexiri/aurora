#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-167 Part B (D-370, C-1022..C-1024): `GET /v1/admin/attention`, and the one e-mail and one
//! Web Push per administrator for each new decision item.

use crate::common::*;
use crate::plugins_lifecycle::setup;
use crate::web_push::{P, W, call, register, world};
use aurora_api::domains::admin::attention::notify_once;
use aurora_api::domains::push::deliver;
use http::Method;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use uuid::Uuid;

fn yaml(pkg: &str, version: &str, host: &str) -> String {
    format!(
        "package: {{ id: {pkg}, version: {version}, contract: 1, min_core: 0.1.0, publisher: acme, name: P{pkg} }}\napps:\n  - about: {{ id: {pkg}_app, name: A }}\n    credentials: [ {{ name: api_token, secret: true }} ]\n    connect:\n      - id: api_key\n        kind: fields\n        fields: [ {{ name: api_token, label: T, type: secret, required: true }} ]\n    hosts:\n      - match: {{ host: \"{host}\" }}\n        inject:\n          - bearer: \"{{credentials.api_token}}\"\n"
    )
}

async fn items(a: &crate::plugins_lifecycle::Admin<'_>) -> Vec<Value> {
    let (s, v) = a.json(Method::GET, "/v1/admin/attention", None).await;
    assert_eq!(s, 200, "{v}");
    v["items"].as_array().unwrap().clone()
}

fn kinds(items: &[Value]) -> Vec<String> {
    items
        .iter()
        .map(|i| i["kind"].as_str().unwrap().to_owned())
        .collect()
}

async fn notices(h: &Harness, kind: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM admin_attention_notices WHERE kind = $1")
        .bind(kind)
        .fetch_one(h.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn only_instance_admins_read_it() {
    let h = Harness::new().await;
    let plain = h.user("plain@x.io", false).await;
    let (_, tok) = h.good_session(plain).await;
    let (s, _, v) = h
        .send(
            Method::GET,
            "/v1/admin/attention",
            &[("cookie", &cookie(&tok)), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!(s, 403, "{v}");
    let (s, _, _) = h.send(Method::GET, "/v1/admin/attention", &[], None).await;
    assert_eq!(s, 401);
}

#[tokio::test]
async fn each_kind_appears_and_disappears_with_its_state() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    assert!(items(&a).await.is_empty());

    // A package version in pending_approval, then approved.
    let alpha = a.upload(&yaml("alpha", "1.0.0", "shared.example")).await;
    let list = items(&a).await;
    assert_eq!(kinds(&list), ["package_approval"], "{list:?}");
    let it = &list[0];
    assert_eq!(it["decision"], true);
    assert_eq!(it["subject_id"], alpha["id"]);
    assert_eq!(it["title"], "Palpha 1.0.0 needs your approval");
    assert_eq!(
        it["href"],
        format!("/admin/apps/packages/{}", alpha["id"].as_str().unwrap())
    );
    assert!(it["since"].is_string());
    assert_eq!(a.act(&alpha["id"], "approve").await.0, 200);
    assert!(items(&a).await.is_empty());

    // A claim that waits for a winner.
    let beta = a.upload(&yaml("beta", "1.0.0", "shared.example")).await;
    assert_eq!(beta["status_reason"], "claim_conflict");
    let list = items(&a).await;
    assert_eq!(kinds(&list), ["host_claim_conflict"], "{list:?}");
    assert_eq!(list[0]["href"], "/admin/apps?tab=advanced#host-conflicts");
    assert_eq!(a.act(&beta["id"], "reject").await.0, 200);
    assert!(items(&a).await.is_empty());

    // A managed connector no host reports: informational, never a decision.
    sqlx::query("INSERT INTO connectors (id, display_name, status, managed, type_id) VALUES ('mm','Mattermost','active',true,'mm')")
        .execute(h.pool())
        .await
        .unwrap();
    let list = items(&a).await;
    assert_eq!(kinds(&list), ["connector_down"], "{list:?}");
    assert_eq!(list[0]["decision"], false);
    assert_eq!(list[0]["title"], "Mattermost is not running");
    let host = Uuid::now_v7();
    sqlx::query("INSERT INTO connector_hosts (instance_id, version, types, last_seen_at) VALUES ($1,'1','[]', now())")
        .bind(host)
        .execute(h.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO connector_host_status (connector_id, host_instance, state, message, updated_at) VALUES ('mm',$1,'error','SECRETMSG', now())")
        .bind(host)
        .execute(h.pool())
        .await
        .unwrap();
    let list = items(&a).await;
    assert_eq!(list[0]["detail"], "Its connector host reports an error.");
    assert!(!list[0].to_string().contains("SECRETMSG"));
    sqlx::query("UPDATE connector_host_status SET state = 'running'")
        .execute(h.pool())
        .await
        .unwrap();
    assert!(items(&a).await.is_empty(), "a running connector is fine");
    // The host stops reporting: down again, once it is older than the staleness window.
    sqlx::query("UPDATE connector_hosts SET last_seen_at = now() - interval '10 minutes'")
        .execute(h.pool())
        .await
        .unwrap();
    assert_eq!(kinds(&items(&a).await), ["connector_down"]);

    // An organization without an owner, until it has one. Decisions sort before the rest.
    let org = h.org().await;
    let gamma = a.upload(&yaml("gamma", "1.0.0", "other.example")).await;
    let list = items(&a).await;
    assert_eq!(
        kinds(&list),
        ["package_approval", "connector_down", "ownerless_org"],
        "{list:?}"
    );
    assert_eq!(list[2]["subject_id"], org.simple().to_string());
    assert_eq!(list[2]["href"], "/admin/people?tab=organizations");
    let owner = h.user("owner@x.io", false).await;
    h.member(org, owner, "owner", "active").await;
    assert_eq!(
        kinds(&items(&a).await),
        ["package_approval", "connector_down"]
    );
    assert_eq!(a.act(&gamma["id"], "reject").await.0, 200);
}

#[tokio::test]
async fn items_that_were_already_pending_are_listed_but_never_mailed() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    // A pending version written before notices existed: no notice row at all.
    sqlx::query("INSERT INTO plugin_packages (id, package_id, version, contract, publisher, digest, signer_key_id, manifest, permissions, source, status, status_reason)
         VALUES (gen_random_uuid(), 'old', '1.0.0', 1, 'acme', 'd', 'builtin', '{\"package\":{\"name\":\"Old\"}}', '{}', 'upload', 'pending_approval', 'first_install')")
        .execute(h.pool())
        .await
        .unwrap();
    let list = items(&a).await;
    assert_eq!(kinds(&list), ["package_approval"]);
    assert_eq!(list[0]["title"], "Old 1.0.0 needs your approval");
    assert_eq!(notify_once(&h.state).await.unwrap(), 0);
    assert!(h.mail.sent().is_empty());
}

#[tokio::test]
async fn non_decision_items_are_never_mailed() {
    let h = Harness::new().await;
    setup(&h).await;
    h.org().await; // ownerless
    sqlx::query("INSERT INTO connectors (id, display_name, status, managed, type_id) VALUES ('mm','Mattermost','active',true,'mm')")
        .execute(h.pool())
        .await
        .unwrap();
    assert_eq!(notify_once(&h.state).await.unwrap(), 0);
    assert!(h.mail.sent().is_empty());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM admin_attention_notices")
            .fetch_one(h.pool())
            .await
            .unwrap(),
        0
    );
}

async fn admin_person(w: &W, email: &str) -> P {
    let id = w.h.user(email, true).await;
    let (session, tok) = w.h.good_session(id).await;
    P {
        id,
        cookie: cookie(&tok),
        session,
    }
}

#[tokio::test]
async fn one_mail_and_one_push_per_admin_per_new_item_and_the_opt_out_is_respected() {
    let w = world().await;
    assert!(
        !w.h.state.auth().smtp,
        "SMTP is unset: the mailer only records (log-only mode)"
    );
    let a1 = admin_person(&w, "a1@x.io").await;
    let a2 = admin_person(&w, "a2@x.io").await;
    register(&w, &a1, "/push/a1", false).await;
    register(&w, &a2, "/push/a2", true).await;
    // a2 turns the e-mail off; the push follows the device.
    let (s, _, me) = call(
        &w,
        &a2,
        Method::PATCH,
        "/v1/me",
        Some(json!({"admin_attention_email": false})),
    )
    .await;
    assert_eq!(s, 200, "{me}");
    assert_eq!(me["admin_attention_email"], false);
    let (_, _, me) = call(&w, &a1, Method::GET, "/v1/me", None).await;
    assert_eq!(me["admin_attention_email"], true, "the default is on");

    let adm = setup(&w.h).await; // root@x.io: an admin with no device
    let v = adm.upload(&yaml("alpha", "1.0.0", "a.example")).await;
    assert_eq!(v["status"], "pending_approval");
    assert_eq!(notices(&w.h, "package_approval").await, 3, "one per admin");

    // E-mail: root and a1; never a2 (opted out); names the item, links /admin only.
    assert_eq!(notify_once(&w.h.state).await.unwrap(), 2);
    let sent = w.h.mail.sent();
    assert_eq!(
        sent.iter().map(|m| m.to.clone()).collect::<BTreeSet<_>>(),
        BTreeSet::from(["root@x.io".to_owned(), "a1@x.io".to_owned()])
    );
    let m = &sent[0];
    let all = format!("{}\n{}", m.subject, m.body);
    assert!(all.contains("Palpha 1.0.0"), "{all}");
    assert_eq!(
        m.link,
        format!(
            "{}/admin",
            w.h.state.config.external_url.trim_end_matches('/')
        )
    );
    assert_eq!(notify_once(&w.h.state).await.unwrap(), 0, "never twice");

    // Push: one per device, fixed fields; the title only on the device that asked for names.
    assert_eq!(deliver::sweep_attention(&w.h.state).await.unwrap(), 2);
    let got = w.mock.received();
    assert_eq!(got.len(), 2);
    for r in &got {
        assert_eq!(r.vapid, Ok(()));
    }
    let by_path = |p: &str| got.iter().find(|r| r.path == p).unwrap().json().unwrap();
    let plain = by_path("/push/a1");
    let mut keys: Vec<_> = plain.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["type", "url", "v"]);
    assert_eq!(plain["type"], "admin_attention");
    assert_eq!(plain["url"], "/admin");
    let detailed = by_path("/push/a2");
    assert_eq!(detailed["title"], "Palpha 1.0.0 needs your approval");
    assert!(detailed.get("detail").is_none());
    assert_eq!(deliver::sweep_attention(&w.h.state).await.unwrap(), 0);

    // A new version is a new subject: it is announced again, once.
    let v2 = adm.upload(&yaml("alpha", "1.1.0", "a.example")).await;
    assert_ne!(v2["id"], v["id"]);
    assert_eq!(notify_once(&w.h.state).await.unwrap(), 2);
    assert_eq!(deliver::sweep_attention(&w.h.state).await.unwrap(), 2);
    assert_eq!(notify_once(&w.h.state).await.unwrap(), 0);
    assert_eq!(deliver::sweep_attention(&w.h.state).await.unwrap(), 0);
}

#[tokio::test]
async fn a_claim_conflict_is_announced_once_as_a_decision() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    let one = a.upload(&yaml("one", "1.0.0", "shared.example")).await;
    assert_eq!(a.act(&one["id"], "approve").await.0, 200);
    notify_once(&h.state).await.unwrap();
    h.mail.clear();
    let two = a.upload(&yaml("two", "1.0.0", "shared.example")).await;
    assert_eq!(two["status_reason"], "claim_conflict");
    assert_eq!(notices(&h, "host_claim_conflict").await, 1);
    assert_eq!(
        notices(&h, "package_approval").await,
        1,
        "only the first install"
    );
    assert_eq!(notify_once(&h.state).await.unwrap(), 1);
    let m = &h.mail.sent()[0];
    assert!(
        m.subject.contains("Ptwo 1.0.0") && m.body.contains("same host"),
        "{m:?}"
    );
}
