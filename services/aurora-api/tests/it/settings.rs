#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! SET-1 (D-236..D-238, SEC-89, SEC-90, C-770..C-777, C-784): the instance-settings framework,
//! exercised through the test-only sections `settings_test` and `settings_test_b`.

use crate::common::*;
use aurora_api::domains::settings::{cli, handle};
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

const HELM: &str = "[settings_test]\nlimit = 5\nnames = [\"helm\"]\n[[settings_test.items]]\nid = \"h\"\nvalue = 1\n";

struct Admin {
    cookie: String,
    session: Uuid,
    user: Uuid,
}

async fn admin(h: &Harness, email: &str) -> Admin {
    let user = h.user(email, true).await;
    let (session, tok) = h.good_session(user).await;
    h.reauth(session).await;
    Admin {
        cookie: cookie(&tok),
        session,
        user,
    }
}

async fn call(
    h: &Harness,
    cookie: &str,
    m: Method,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let (s, _, v) = h
        .send(m, uri, &[("cookie", cookie), ("origin", ORIGIN)], body)
        .await;
    (s.as_u16(), v)
}

async fn get(h: &Harness, a: &Admin, section: &str) -> Value {
    let (s, v) = call(
        h,
        &a.cookie,
        Method::GET,
        &format!("/v1/admin/settings/{section}"),
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    v
}

async fn put(
    h: &Harness,
    a: &Admin,
    section: &str,
    generation: i64,
    values: Value,
) -> (u16, Value) {
    call(
        h,
        &a.cookie,
        Method::PUT,
        &format!("/v1/admin/settings/{section}"),
        Some(json!({ "generation": generation, "values": values })),
    )
    .await
}

fn setting<'a>(section: &'a Value, key: &str) -> &'a Value {
    section["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == key)
        .unwrap()
}

#[tokio::test]
async fn get_shows_sources_and_locks() {
    let h = Harness::new().await.with_helm(HELM);
    let a = admin(&h, "root@x.io").await;
    let (s, idx) = call(&h, &a.cookie, Method::GET, "/v1/admin/settings", None).await;
    assert_eq!(s, 200, "{idx}");
    let locked = idx["locked"].as_array().unwrap();
    assert!(
        locked
            .iter()
            .any(|l| l["key"] == "settings_test.limit"
                && l["helm_key"] == "config.settings_test.limit"),
        "{idx}"
    );
    assert!(
        locked
            .iter()
            .any(|l| l["key"] == "settings_test.items" && l["entry_id"] == "h"),
        "{idx}"
    );
    let secs = idx["sections"].as_array().unwrap();
    assert!(
        secs.iter().any(|s| s["section"] == "governance"),
        "all thirteen sections are listed"
    );
    let t = get(&h, &a, "settings_test").await;
    let limit = setting(&t, "settings_test.limit");
    assert_eq!(
        (
            limit["source"].as_str(),
            limit["locked"].as_bool(),
            limit["value"].as_i64()
        ),
        (Some("helm"), Some(true), Some(5))
    );
    let flag = setting(&t, "settings_test.flag");
    assert_eq!(
        (flag["source"].as_str(), flag["locked"].as_bool()),
        (Some("default"), Some(false))
    );
    let items = setting(&t, "settings_test.items");
    assert_eq!(items["entries"][0]["source"], "helm");
    assert_eq!(items["entries"][0]["locked"], true);
    let (s, e) = call(&h, &a.cookie, Method::GET, "/v1/admin/settings/nope", None).await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (404, Some("setting_section_unknown"))
    );
}

#[tokio::test]
async fn put_round_trip_default_and_stale_generation() {
    let h = Harness::new().await;
    let a = admin(&h, "root@x.io").await;
    let (s, v) = put(
        &h,
        &a,
        "settings_test",
        0,
        json!({ "settings_test.flag": true, "settings_test.limit": 20 }),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["generation"], 1);
    assert_eq!(setting(&v, "settings_test.limit")["source"], "dashboard");
    assert_eq!(setting(&v, "settings_test.limit")["value"], 20);
    // The snapshot of this replica already serves it, and the row carries the writer.
    let t = get(&h, &a, "settings_test").await;
    assert_eq!(setting(&t, "settings_test.flag")["value"], true);
    assert_eq!(t["updated_by_label"], "root@x.io");
    // Absent keys return to their default.
    let (s, v) = put(
        &h,
        &a,
        "settings_test",
        1,
        json!({ "settings_test.flag": true }),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let limit = setting(&v, "settings_test.limit");
    assert_eq!(
        (limit["source"].as_str(), limit["value"].as_i64()),
        (Some("default"), Some(10))
    );
    // A stale generation is refused and nothing changes.
    let (s, e) = put(&h, &a, "settings_test", 1, json!({})).await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (409, Some("stale_revision")),
        "{e}"
    );
    assert_eq!(e["error"]["details"]["current_generation"], 2);
    let t = get(&h, &a, "settings_test").await;
    assert_eq!(setting(&t, "settings_test.flag")["value"], true);
    // Type and bounds, unknown key.
    let (s, e) = put(
        &h,
        &a,
        "settings_test",
        2,
        json!({ "settings_test.limit": 500 }),
    )
    .await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (422, Some("settings_invalid")),
        "{e}"
    );
    assert_eq!(
        e["error"]["details"]["fields"][0]["path"],
        "/settings_test.limit"
    );
    let (s, e) = put(
        &h,
        &a,
        "settings_test",
        2,
        json!({ "hosted.allow_fake_harness": true }),
    )
    .await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (400, Some("setting_unknown")),
        "{e}"
    );
    // A cross-section rule names the other key.
    let (s, e) = put(
        &h,
        &a,
        "settings_test",
        2,
        json!({ "settings_test.mode": "open" }),
    )
    .await;
    assert_eq!(s, 422, "{e}");
    assert_eq!(
        e["error"]["details"]["fields"][0]["path"],
        "/settings_test.flag"
    );
    assert!(
        e["error"]["details"]["fields"][0]["message"]
            .as_str()
            .unwrap()
            .contains("settings_test.mode")
    );
}

#[tokio::test]
async fn locked_keys_are_refused_on_every_write_path() {
    let h = Harness::new().await.with_helm(HELM);
    let a = admin(&h, "root@x.io").await;
    let (s, e) = put(
        &h,
        &a,
        "settings_test",
        0,
        json!({ "settings_test.limit": 6 }),
    )
    .await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (409, Some("setting_locked")),
        "{e}"
    );
    assert_eq!(e["error"]["details"]["key"], "settings_test.limit");
    assert_eq!(
        e["error"]["details"]["helm_key"],
        "config.settings_test.limit"
    );
    // Whole list locked as a whole.
    let (s, e) = put(
        &h,
        &a,
        "settings_test",
        0,
        json!({ "settings_test.names": ["dash"] }),
    )
    .await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (409, Some("setting_locked")),
        "{e}"
    );
    // Equal to the effective value: ignored, nothing to write.
    let (s, v) = put(
        &h,
        &a,
        "settings_test",
        0,
        json!({ "settings_test.limit": 5 }),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["generation"], 0);
    // Import (merge and replace) and the CLI reset.
    for mode in ["replace", "merge"] {
        let doc = json!({ "aurora_settings": 1, "sections": { "settings_test": { "settings_test.limit": 6 } } });
        let (s, e) = call(&h, &a.cookie, Method::POST, "/v1/admin/settings/import", Some(json!({ "document": doc.to_string(), "format": "json", "dry_run": false, "mode": mode, "generations": { "settings_test": 0 } }))).await;
        assert_eq!(
            (s, e["error"]["code"].as_str()),
            (409, Some("setting_locked")),
            "{mode}: {e}"
        );
    }
    let r = cli::reset(
        &h.state,
        "settings_test",
        Some("settings_test.limit"),
        None,
        false,
    )
    .await
    .unwrap_err();
    assert_eq!(r.code(), "setting_locked");
    assert_eq!(cli::exit_code(&r), 3);
    let r = cli::reset(
        &h.state,
        "settings_test",
        Some("settings_test.items"),
        Some("h"),
        false,
    )
    .await
    .unwrap_err();
    assert_eq!(r.code(), "setting_locked");
    let t = get(&h, &a, "settings_test").await;
    assert_eq!(setting(&t, "settings_test.limit")["value"], 5);
}

#[tokio::test]
async fn keyed_list_keeps_helm_entries_beside_dashboard_entries() {
    let h = Harness::new().await.with_helm(HELM);
    let a = admin(&h, "root@x.io").await;
    let (s, v) = put(
        &h,
        &a,
        "settings_test",
        0,
        json!({ "settings_test.items": [{ "id": "d", "value": 2 }] }),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let entries = setting(&v, "settings_test.items")["entries"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(entries.len(), 2, "{v}");
    assert_eq!(
        (
            entries[0]["id"].as_str(),
            entries[0]["source"].as_str(),
            entries[0]["locked"].as_bool()
        ),
        (Some("h"), Some("helm"), Some(true))
    );
    assert_eq!(
        (entries[1]["id"].as_str(), entries[1]["source"].as_str()),
        (Some("d"), Some("dashboard"))
    );
    // A dashboard entry may not reuse a Helm entry's id.
    let (s, e) = put(
        &h,
        &a,
        "settings_test",
        1,
        json!({ "settings_test.items": [{ "id": "h", "value": 9 }] }),
    )
    .await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (422, Some("settings_invalid")),
        "{e}"
    );
    let f = &e["error"]["details"]["fields"][0];
    assert_eq!(
        (f["path"].as_str(), f["code"].as_str()),
        (Some("/settings_test.items/h/id"), Some("entry_id_in_helm"))
    );
    // The Helm entry and the dashboard one are untouched.
    let t = get(&h, &a, "settings_test").await;
    assert_eq!(
        setting(&t, "settings_test.items")["entries"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(t["generation"], 1);
}

#[tokio::test]
async fn concurrent_puts_on_one_generation_make_one_conflict() {
    let h = Harness::new().await;
    let a = admin(&h, "root@x.io").await;
    let (r1, r2) = tokio::join!(
        put(
            &h,
            &a,
            "settings_test",
            0,
            json!({ "settings_test.limit": 11 })
        ),
        put(
            &h,
            &a,
            "settings_test",
            0,
            json!({ "settings_test.limit": 12 })
        ),
    );
    let mut codes = [r1.0, r2.0];
    codes.sort_unstable();
    assert_eq!(codes, [200, 409], "{r1:?} {r2:?}");
    let t = get(&h, &a, "settings_test").await;
    assert_eq!(t["generation"], 1);
}

#[tokio::test]
async fn writes_need_an_instance_admin_session_and_a_recent_reauthentication() {
    let h = Harness::new().await;
    let a = admin(&h, "root@x.io").await;
    // 412 without a re-authentication within 600 s.
    let user = h.user("stale@x.io", true).await;
    let (_, tok) = h.good_session(user).await;
    let (s, e) = put(
        &h,
        &Admin {
            cookie: cookie(&tok),
            session: Uuid::nil(),
            user,
        },
        "settings_test",
        0,
        json!({ "settings_test.limit": 3 }),
    )
    .await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (412, Some("reauth_required")),
        "{e}"
    );
    sqlx::query("UPDATE sessions SET reauth_at = now() - interval '11 minutes' WHERE user_id = $1")
        .bind(user)
        .execute(h.pool())
        .await
        .unwrap();
    let (s, _) = put(
        &h,
        &Admin {
            cookie: cookie(&tok),
            session: Uuid::nil(),
            user,
        },
        "settings_test",
        0,
        json!({}),
    )
    .await;
    assert_eq!(s, 412);
    // Org owner, plain user, API key: 403.
    let owner = h.user("owner@x.io", false).await;
    let org = h.org().await;
    h.member(org, owner, "owner", "active").await;
    let (_, otok) = h.good_session(owner).await;
    let (s, e) = call(&h, &cookie(&otok), Method::GET, "/v1/admin/settings", None).await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (403, Some("instance_admin_required")),
        "{e}"
    );
    let (s, _) = put(
        &h,
        &Admin {
            cookie: cookie(&otok),
            session: Uuid::nil(),
            user: owner,
        },
        "settings_test",
        0,
        json!({}),
    )
    .await;
    assert_eq!(s, 403);
    let (_, key) = h.api_key(a.user, org, None, false).await;
    let (s, _, _) = h
        .send(
            Method::GET,
            "/v1/admin/settings",
            &[("authorization", &format!("Bearer {key}"))],
            None,
        )
        .await;
    assert!(s == 403 || s == 401, "{s}");
    let _ = a.session;
}

#[tokio::test]
async fn a_failing_audit_insert_fails_the_write_and_changes_nothing() {
    let h = Harness::new().await;
    let a = admin(&h, "root@x.io").await;
    sqlx::query("CREATE FUNCTION audit_critical_fail() RETURNS trigger AS $$ BEGIN RAISE EXCEPTION 'audit down'; END $$ LANGUAGE plpgsql").execute(h.pool()).await.unwrap();
    sqlx::query("CREATE TRIGGER audit_critical_fail BEFORE INSERT ON audit_log FOR EACH ROW WHEN (NEW.critical) EXECUTE FUNCTION audit_critical_fail()").execute(h.pool()).await.unwrap();
    let (s, e) = put(
        &h,
        &a,
        "settings_test",
        0,
        json!({ "settings_test.limit": 33 }),
    )
    .await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (503, Some("audit_unavailable")),
        "{e}"
    );
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM instance_settings")
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(rows, 0, "no row is written without its audit row (SEC-90)");
    let t = get(&h, &a, "settings_test").await;
    assert_eq!(setting(&t, "settings_test.limit")["value"], 10);
}

#[tokio::test]
async fn update_is_audited_with_a_diff_and_every_admin_is_told_by_key_name_only() {
    let h = Harness::new().await;
    let a = admin(&h, "root@x.io").await;
    h.user("second@x.io", true).await;
    let gone = h.user("gone@x.io", true).await;
    h.disable_user(gone).await;
    let (s, v) = put(
        &h,
        &a,
        "settings_test",
        0,
        json!({ "settings_test.limit": 42 }),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let row: (String, String, Option<String>, Value, bool) = sqlx::query_as(
        "SELECT action, source, target_id, metadata, critical FROM audit_log WHERE action = 'instance_settings.update'",
    )
    .fetch_one(h.pool())
    .await
    .unwrap();
    assert_eq!(
        (row.0.as_str(), row.1.as_str(), row.2.as_deref(), row.4),
        (
            "instance_settings.update",
            "dashboard",
            Some("settings_test"),
            true
        )
    );
    assert_eq!(row.3["generation"], 1);
    assert_eq!(
        row.3["change"][0],
        json!({ "key": "settings_test.limit", "before": null, "after": 42 })
    );
    // The e-mail goes to every instance admin who is not disabled, names keys, never values.
    let mut sent = Vec::new();
    for _ in 0..200 {
        sent = h.mail.sent();
        if sent.len() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let mut to: Vec<&str> = sent.iter().map(|m| m.to.as_str()).collect();
    to.sort_unstable();
    assert_eq!(to, ["root@x.io", "second@x.io"]);
    let m = &sent[0];
    assert!(
        m.subject.contains("instance settings changed"),
        "{}",
        m.subject
    );
    assert!(
        m.body.contains("settings_test.limit") && !m.body.contains("42"),
        "{}",
        m.body
    );
    assert!(m.link.ends_with("/admin/audit?action=instance_settings"));
}

#[tokio::test]
async fn secrets_are_sealed_inline_kept_cleared_and_never_leak() {
    const CANARY: &str = "canary-9f3a7-plaintext";
    let h = Harness::new().await;
    let a = admin(&h, "root@x.io").await;
    let (s, v) = put(
        &h,
        &a,
        "settings_test",
        0,
        json!({ "settings_test.token": { "$secret": CANARY } }),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert!(!v.to_string().contains(CANARY));
    let tok = setting(&v, "settings_test.token");
    assert_eq!(tok["value"], Value::Null);
    assert_eq!(tok["secret"]["set"], true);
    // Sealed inline: the stored reference opens to the value under its context, nothing else holds it.
    let stored: Value =
        sqlx::query_scalar("SELECT value FROM instance_settings WHERE section = 'settings_test'")
            .fetch_one(h.pool())
            .await
            .unwrap();
    let sec = &stored["settings_test.token"]["$secret"];
    let r: aurora_secrets::SecretRef = sec["ref"].as_str().unwrap().parse().unwrap();
    assert!(r.store_id().is_none(), "inline store");
    let ctx = aurora_secrets::SealContext {
        purpose: aurora_secrets::Purpose::InstanceSetting,
        owner_id: sec["owner"].as_str().unwrap().parse().unwrap(),
        org_id: Uuid::nil(),
        workspace_id: None,
    };
    let opened = h.state.vault.open(&ctx, &r).await.unwrap();
    assert_eq!(opened.expose().as_slice(), CANARY.as_bytes());
    assert!(!stored.to_string().contains(CANARY));
    // `keep` keeps it, even while another key changes.
    let (s, v) = put(
        &h,
        &a,
        "settings_test",
        1,
        json!({ "settings_test.token": { "$secret": "keep" }, "settings_test.limit": 7 }),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(setting(&v, "settings_test.token")["secret"]["set"], true);
    // Export has `set`, never the value; the audit rows and every e-mail never carry it.
    let (s, _, _) = h
        .send(
            Method::GET,
            "/v1/admin/settings/export?format=json",
            &[("cookie", &a.cookie)],
            None,
        )
        .await;
    assert_eq!(s, 200);
    let exported = aurora_api::domains::settings::service::export_document(&h.state, None).unwrap();
    assert_eq!(
        exported["sections"]["settings_test"]["settings_test.token"],
        json!({ "$secret": "set" })
    );
    assert!(!exported.to_string().contains(CANARY));
    let audit: Vec<Value> = sqlx::query_scalar(
        "SELECT metadata FROM audit_log WHERE action LIKE 'instance_settings.%'",
    )
    .fetch_all(h.pool())
    .await
    .unwrap();
    assert!(audit.iter().all(|m| !m.to_string().contains(CANARY)));
    assert!(
        audit
            .iter()
            .any(|m| m["change"][0]["after"] == "(secret changed)"),
        "{audit:?}"
    );
    for m in h.mail.sent() {
        assert!(!m.body.contains(CANARY) && !m.subject.contains(CANARY));
    }
    // Absent clears it; `keep` without a stored secret is a 400.
    let (s, v) = put(&h, &a, "settings_test", 2, json!({})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(setting(&v, "settings_test.token")["secret"]["set"], false);
    let (s, e) = put(
        &h,
        &a,
        "settings_test",
        3,
        json!({ "settings_test.token": { "$secret": "keep" } }),
    )
    .await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (400, Some("invalid_request")),
        "{e}"
    );
}

#[tokio::test]
async fn mirroring_keeps_a_removed_helm_value_as_a_dashboard_value() {
    let h = Harness::new().await.with_helm(HELM);
    let a = admin(&h, "root@x.io").await;
    handle::start(&h.state.settings, h.pool()).await.unwrap();
    let stored: Value =
        sqlx::query_scalar("SELECT value FROM instance_settings WHERE section = 'settings_test'")
            .fetch_one(h.pool())
            .await
            .unwrap();
    assert_eq!(stored["settings_test.limit"], 5);
    assert_eq!(stored["settings_test.items"][0]["id"], "h");
    let sync: Value = sqlx::query_scalar(
        "SELECT metadata FROM audit_log WHERE action = 'instance_settings.helm_sync'",
    )
    .fetch_one(h.pool())
    .await
    .unwrap();
    assert_eq!(sync["section"], "settings_test");
    assert!(
        sync["keys"]
            .as_array()
            .unwrap()
            .iter()
            .any(|k| k == "settings_test.limit")
    );
    let t = get(&h, &a, "settings_test").await;
    assert_eq!(setting(&t, "settings_test.limit")["source"], "helm");
    assert_eq!(t["generation"], 1);
    // The operator removes the key from the values file: the value stays, now editable.
    let h2 = h.with_helm("");
    handle::start(&h2.state.settings, h2.pool()).await.unwrap();
    let t = get(&h2, &a, "settings_test").await;
    let limit = setting(&t, "settings_test.limit");
    assert_eq!(
        (
            limit["source"].as_str(),
            limit["value"].as_i64(),
            limit["locked"].as_bool()
        ),
        (Some("dashboard"), Some(5), Some(false))
    );
    // A second start with the same configuration writes nothing.
    let before = t["generation"].clone();
    handle::start(&h2.state.settings, h2.pool()).await.unwrap();
    assert_eq!(get(&h2, &a, "settings_test").await["generation"], before);
}

#[tokio::test]
async fn export_and_import_round_trip_with_a_dry_run_diff() {
    let h = Harness::new().await.with_helm(HELM);
    let a = admin(&h, "root@x.io").await;
    let (s, v) = put(
        &h,
        &a,
        "settings_test",
        0,
        json!({ "settings_test.flag": true, "settings_test.items": [{ "id": "d", "value": 2 }] }),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let (s, hd, _) = h
        .send(
            Method::GET,
            "/v1/admin/settings/export",
            &[("cookie", &a.cookie)],
            None,
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(hd["content-type"], "application/yaml");
    assert!(
        hd["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment; filename=\"aurora-settings-")
    );
    let doc = aurora_api::domains::settings::service::export_document(&h.state, None).unwrap();
    // Only dashboard values: the locked limit and the Helm entry are not in it.
    assert_eq!(
        doc["sections"]["settings_test"],
        json!({ "settings_test.flag": true, "settings_test.items": [{ "id": "d", "value": 2 }] })
    );
    assert!(
        doc["locked"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["key"] == "settings_test.limit")
    );
    // Change the second section, then import the old document in `replace` mode (dry run first).
    let (s, _) = put(
        &h,
        &a,
        "settings_test_b",
        0,
        json!({ "settings_test_b.count": 4 }),
    )
    .await;
    assert_eq!(s, 200);
    let edited = {
        let mut d = doc.clone();
        d["sections"]["settings_test"]["settings_test.flag"] = json!(false);
        d["sections"]["settings_test"]["settings_test.limit"] = json!(5); // equals the Helm value
        d
    };
    let g = |t: i64, b: i64| json!({ "settings_test": t, "settings_test_b": b });
    let (s, dry) = call(
        &h,
        &a.cookie,
        Method::POST,
        "/v1/admin/settings/import",
        Some(json!({ "document": edited.to_string(), "format": "json", "dry_run": true })),
    )
    .await;
    assert_eq!(s, 200, "{dry}");
    assert_eq!(dry["applied"], false);
    let sec = &dry["sections"][0];
    assert_eq!(sec["section"], "settings_test");
    assert!(
        sec["diff"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["key"] == "settings_test.flag"
                && d["before"] == true
                && d["after"] == false),
        "{dry}"
    );
    assert!(
        sec["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "locked_ignored"),
        "{dry}"
    );
    // Nothing was written by the dry run.
    assert_eq!(get(&h, &a, "settings_test").await["generation"], 1);
    // Apply needs the generations (400), then applies; the second section (absent from the document) is untouched in `merge`.
    let (s, _) = call(
        &h,
        &a.cookie,
        Method::POST,
        "/v1/admin/settings/import",
        Some(json!({ "document": edited.to_string(), "format": "json", "dry_run": false })),
    )
    .await;
    assert_eq!(s, 400);
    let (s, ok) = call(&h, &a.cookie, Method::POST, "/v1/admin/settings/import", Some(json!({ "document": edited.to_string(), "format": "json", "dry_run": false, "mode": "merge", "generations": g(1, 1) }))).await;
    assert_eq!(s, 200, "{ok}");
    assert_eq!(ok["applied"], true);
    assert_eq!(
        setting(&get(&h, &a, "settings_test").await, "settings_test.flag")["value"],
        false
    );
    assert_eq!(
        setting(
            &get(&h, &a, "settings_test_b").await,
            "settings_test_b.count"
        )["value"],
        4
    );
    // A stale generation → 409 naming the section.
    let (s, e) = call(&h, &a.cookie, Method::POST, "/v1/admin/settings/import", Some(json!({ "document": edited.to_string(), "format": "json", "dry_run": false, "generations": g(1, 1) }))).await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (409, Some("stale_revision")),
        "{e}"
    );
    assert_eq!(e["error"]["details"]["section"], "settings_test");
    // One audit row for the whole import.
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'instance_settings.import'",
    )
    .fetch_one(h.pool())
    .await
    .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn import_is_one_transaction_and_refuses_plaintext_secrets() {
    let h = Harness::new().await;
    let a = admin(&h, "root@x.io").await;
    let bad = json!({ "aurora_settings": 1, "sections": {
        "settings_test": { "settings_test.limit": 9 },
        "settings_test_b": { "settings_test_b.count": 99 } } });
    let (s, e) = call(&h, &a.cookie, Method::POST, "/v1/admin/settings/import", Some(json!({ "document": bad.to_string(), "format": "json", "dry_run": false, "generations": { "settings_test": 0, "settings_test_b": 0 } }))).await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (422, Some("settings_invalid")),
        "{e}"
    );
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM instance_settings")
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(rows, 0, "a failing section rolls every section back");
    let (s, dry) = call(
        &h,
        &a.cookie,
        Method::POST,
        "/v1/admin/settings/import",
        Some(json!({ "document": bad.to_string(), "format": "json", "dry_run": true })),
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(dry["errors"][0]["section"], "settings_test_b");
    // A plaintext secret in a document.
    let doc = json!({ "aurora_settings": 1, "sections": { "settings_test": { "settings_test.token": { "$secret": "hunter2" } } } });
    let (s, e) = call(
        &h,
        &a.cookie,
        Method::POST,
        "/v1/admin/settings/import",
        Some(json!({ "document": doc.to_string(), "format": "json", "dry_run": true })),
    )
    .await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (400, Some("secret_in_import")),
        "{e}"
    );
    // `set` without a stored secret.
    let doc = json!({ "aurora_settings": 1, "sections": { "settings_test": { "settings_test.token": { "$secret": "set" } } } });
    let (s, e) = call(&h, &a.cookie, Method::POST, "/v1/admin/settings/import", Some(json!({ "document": doc.to_string(), "format": "json", "dry_run": false, "generations": { "settings_test": 0 } }))).await;
    assert_eq!(s, 400, "{e}");
    // An unknown document version.
    let (s, _) = call(&h, &a.cookie, Method::POST, "/v1/admin/settings/import", Some(json!({ "document": "aurora_settings: 2\nsections: {}\n", "format": "yaml", "dry_run": true }))).await;
    assert_eq!(s, 400);
}

#[tokio::test]
async fn invalid_stored_settings_refuse_start_and_the_cli_reset_repairs_them() {
    let h = Harness::new().await;
    sqlx::query("INSERT INTO instance_settings (section, value, generation) VALUES ('settings_test', '{\"settings_test.limit\": 500}', 1)")
        .execute(h.pool())
        .await
        .unwrap();
    let err = handle::start(&h.state.settings, h.pool())
        .await
        .unwrap_err();
    assert!(
        err.contains("settings_test.limit")
            && err.contains("aurora-api settings reset settings_test"),
        "{err}"
    );
    // The CLI works with the broken section: show it, dry-run the reset, reset it.
    let shown = cli::show(&h.state, Some("settings_test"), false).await;
    assert!(shown.is_ok(), "{shown:?}");
    let dry = cli::reset(&h.state, "settings_test", None, None, true)
        .await
        .unwrap();
    assert!(dry.contains("would change"), "{dry}");
    let out = cli::reset(&h.state, "settings_test", None, None, false)
        .await
        .unwrap();
    assert!(out.contains("changed"), "{out}");
    let row: (String, String, Value) = sqlx::query_as(
        "SELECT action, source, metadata FROM audit_log WHERE action = 'instance_settings.reset'",
    )
    .fetch_one(h.pool())
    .await
    .unwrap();
    assert_eq!(
        (row.0.as_str(), row.1.as_str()),
        ("instance_settings.reset", "cli")
    );
    let label: String = sqlx::query_scalar(
        "SELECT actor_label FROM audit_log WHERE action = 'instance_settings.reset'",
    )
    .fetch_one(h.pool())
    .await
    .unwrap();
    assert_eq!(label, "cli");
    handle::start(&h.state.settings, h.pool()).await.unwrap();
    // A row hand-edited to hold a development switch or an unknown key has no effect (SEC-89).
    sqlx::query("UPDATE instance_settings SET value = '{\"hosted.allow_fake_harness\": true, \"settings_test.flag\": true}', generation = generation + 1 WHERE section = 'settings_test'")
        .execute(h.pool())
        .await
        .unwrap();
    h.state.settings.reload(h.pool(), "api").await.unwrap();
    let snap = h.state.settings.snapshot();
    assert!(!snap.stored.sections["settings_test"].contains_key("hosted.allow_fake_harness"));
    assert_eq!(snap.effective.keys["settings_test.flag"].value, json!(true));
    assert!(!h.state.config.hosted.allow_fake_harness);
}

#[tokio::test]
async fn preview_reports_the_diff_and_loosening_without_writing() {
    let h = Harness::new().await;
    let a = admin(&h, "root@x.io").await;
    // No re-authentication is needed for a preview.
    let user = h.user("p@x.io", true).await;
    let (_, tok) = h.good_session(user).await;
    let (s, v) = call(&h, &cookie(&tok), Method::POST, "/v1/admin/settings/settings_test/preview", Some(json!({ "generation": 0, "values": { "settings_test.flag": true, "settings_test.limit": 50, "settings_test.token": { "$secret": "x-secret" } } }))).await;
    assert_eq!(s, 200, "{v}");
    let keys: Vec<&str> = v["diff"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["key"].as_str().unwrap())
        .collect();
    assert!(
        keys.contains(&"settings_test.flag")
            && keys.contains(&"settings_test.limit")
            && keys.contains(&"settings_test.token"),
        "{v}"
    );
    let loose: Vec<&str> = v["loosening"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["key"].as_str().unwrap())
        .collect();
    assert!(
        loose.contains(&"settings_test.flag") && loose.contains(&"settings_test.limit"),
        "{v}"
    );
    assert!(!v.to_string().contains("x-secret"));
    assert_eq!(v["affected_orgs"], json!([]));
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM instance_settings")
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(rows, 0);
    let (s, e) = call(
        &h,
        &a.cookie,
        Method::POST,
        "/v1/admin/settings/settings_test/preview",
        Some(json!({ "generation": 5, "values": {} })),
    )
    .await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (409, Some("stale_revision"))
    );
}

#[tokio::test]
async fn a_failing_reload_keeps_the_snapshot_and_counts_it() {
    let h = Harness::new().await;
    let a = admin(&h, "root@x.io").await;
    let (s, _) = put(
        &h,
        &a,
        "settings_test",
        0,
        json!({ "settings_test.limit": 8 }),
    )
    .await;
    assert_eq!(s, 200);
    let before = counter_value(
        "aurora_settings_reload_failed_total{service=\"api\",section=\"settings_test\"}",
    );
    sqlx::query("UPDATE instance_settings SET value = '{\"settings_test.limit\": 9999}', generation = generation + 1")
        .execute(h.pool())
        .await
        .unwrap();
    assert!(h.state.settings.reload(h.pool(), "api").await.is_err());
    assert_eq!(
        h.state.settings.snapshot().effective.keys["settings_test.limit"].value,
        json!(8),
        "the previous snapshot stays"
    );
    // The metric names the section; a poll picks up the next valid generation.
    assert!(
        counter_value(
            "aurora_settings_reload_failed_total{service=\"api\",section=\"settings_test\"}"
        ) > before
    );
    sqlx::query("UPDATE instance_settings SET value = '{\"settings_test.limit\": 12}', generation = generation + 1")
        .execute(h.pool())
        .await
        .unwrap();
    assert!(
        h.state
            .settings
            .reload_if_changed(h.pool(), "api")
            .await
            .unwrap()
    );
    assert_eq!(
        h.state.settings.snapshot().effective.keys["settings_test.limit"].value,
        json!(12)
    );
}
