#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RFE-008 (D-52, SEC-32): atomic audit for the security-critical set (`04 §Audit`).
//!
//! * the set is enumerated once (`aurora_core::audit::SECURITY_CRITICAL_ACTIONS`);
//! * a source scan proves every listed action is written inside the change's transaction
//!   (`audit::commit_critical` / `audit::insert_tx`) and never through the best-effort
//!   `audit::record`, so a future bypass fails the build's tests;
//! * a database fault injected into critical audit inserts makes representative critical actions
//!   (one per area, through the real routes) answer 503 `audit_unavailable` and change nothing;
//!   with the fault removed the same calls succeed and write `critical = true` rows;
//! * best-effort actions keep working while critical audit inserts fail;
//! * the audit-log `critical` filter and field.

use crate::approvals::{approval, fixture};
use crate::common::*;
use aurora_core::audit::{SECURITY_CRITICAL_ACTIONS, is_critical};
use http::Method;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use uuid::Uuid;

// ---------------------------------------------------------------- source scan (coverage)

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Actions whose name is built at run time (`format!("{}.freeze", ..)`, `format!("plugin.{}", ..)`):
/// their writer file is checked instead of the literal.
const DYNAMIC: &[(&str, &str)] = &[
    ("agent.freeze", "domains/freeze/service.rs"),
    ("agent.unfreeze", "domains/freeze/service.rs"),
    ("workspace.freeze", "domains/freeze/service.rs"),
    ("workspace.unfreeze", "domains/freeze/service.rs"),
    ("org.freeze", "domains/freeze/service.rs"),
    ("org.unfreeze", "domains/freeze/service.rs"),
    ("plugin.approve", "domains/plugins/lifecycle.rs"),
    ("plugin.reject", "domains/plugins/lifecycle.rs"),
    ("plugin.disable", "domains/plugins/lifecycle.rs"),
    ("plugin.enable", "domains/plugins/lifecycle.rs"),
];

#[test]
fn every_critical_action_is_written_in_the_changes_transaction() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    let texts: Vec<(String, Vec<String>)> = files
        .iter()
        .map(|p| {
            let rel = p
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let lines = std::fs::read_to_string(p)
                .unwrap()
                .lines()
                .map(str::to_owned)
                .collect();
            (rel, lines)
        })
        .collect();
    // A domain builds the event in its routes and commits it in its service: the module (the
    // directory of the file) must contain the transactional writer.
    let transactional = |rel: &str| {
        let dir = rel.rsplit_once('/').map_or("", |(d, _)| d);
        texts.iter().any(|(r, lines)| {
            r.rsplit_once('/').map_or("", |(d, _)| d) == dir
                && lines
                    .iter()
                    .any(|l| l.contains("commit_critical") || l.contains("insert_tx"))
        })
    };
    for action in SECURITY_CRITICAL_ACTIONS {
        let needle = format!("\"{action}\"");
        let mut found = false;
        for (rel, lines) in &texts {
            if rel == "audit.rs" {
                continue;
            }
            for (i, l) in lines.iter().enumerate() {
                if !l.contains(&needle) {
                    continue;
                }
                found = true;
                assert!(
                    transactional(rel),
                    "{action}: {rel} writes a security-critical audit row but never in the \
                     change's transaction (use audit::commit_critical)"
                );
                // The event of a critical action must never be handed to the best-effort writer.
                let from = i.saturating_sub(8);
                let window = lines[from..=i].join("\n");
                assert!(
                    !window.contains("audit::record("),
                    "{action}: {rel}:{} passes a security-critical event to audit::record",
                    i + 1
                );
            }
        }
        if !found {
            let (_, file) = DYNAMIC
                .iter()
                .find(|(a, _)| a == action)
                .unwrap_or_else(|| panic!("{action}: no code writes this critical action"));
            assert!(transactional(file), "{action}: {file} is not transactional");
        }
    }
}

#[test]
fn best_effort_writer_refuses_critical_events_in_debug_builds() {
    // `audit::record` carries a debug assertion for security-critical successes; the scan above
    // and the fault-injection test below are what keep release builds honest.
    assert!(is_critical("policy.publish"));
    assert!(!is_critical("agent.create"));
    assert!(!is_critical("user.login"));
}

// ---------------------------------------------------------------- fault injection

struct W {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    agent: Uuid,
    owner_id: Uuid,
    owner: String,
    member_id: Uuid,
    outsider_id: Uuid,
    removable_id: Uuid,
    approval: Uuid,
}

async fn world() -> W {
    let h = Harness::new().await;
    let f = fixture(&h).await;
    let owner_id = h.user("o@x.io", false).await;
    let member_id = h.user("m@x.io", false).await;
    let outsider_id = h.user("x@x.io", false).await;
    let removable_id = h.user("r@x.io", false).await;
    h.member(f.org, owner_id, "owner", "active").await;
    h.member(f.org, member_id, "member", "active").await;
    h.member(f.org, outsider_id, "member", "active").await;
    h.member(f.org, removable_id, "member", "active").await;
    h.bind_ws(f.ws, member_id, "member").await;
    let approval = approval(&h, &f, f.ws, f.agent, None).await;
    let (_, owner) = h.good_session(owner_id).await;
    W {
        h,
        org: f.org,
        ws: f.ws,
        agent: f.agent,
        owner_id,
        owner,
        member_id,
        outsider_id,
        removable_id,
        approval,
    }
}

impl W {
    async fn call(&self, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        let c = cookie(&self.owner);
        let (s, _, v) = self
            .h
            .send(m, uri, &[("cookie", c.as_str()), ("origin", ORIGIN)], body)
            .await;
        (s.as_u16(), v)
    }

    async fn probe(&self, sql: &str) -> String {
        sqlx::query_scalar(sql)
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }

    /// Every audit insert of a security-critical row now fails (the database is "down" for them).
    async fn fail_critical_audit(&self) {
        sqlx::query(
            "CREATE FUNCTION audit_critical_fail() RETURNS trigger AS $$ BEGIN \
             RAISE EXCEPTION 'audit down'; END $$ LANGUAGE plpgsql",
        )
        .execute(self.h.pool())
        .await
        .unwrap();
        sqlx::query(
            "CREATE TRIGGER audit_critical_fail BEFORE INSERT ON audit_log FOR EACH ROW \
             WHEN (NEW.critical) EXECUTE FUNCTION audit_critical_fail()",
        )
        .execute(self.h.pool())
        .await
        .unwrap();
    }

    async fn restore_audit(&self) {
        sqlx::query("DROP TRIGGER audit_critical_fail ON audit_log")
            .execute(self.h.pool())
            .await
            .unwrap();
    }
}

struct Case {
    action: &'static str,
    method: Method,
    uri: String,
    body: Option<Value>,
    /// Text of the state the action changes; must be identical after a refused attempt.
    probe: String,
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

async fn cases(w: &W) -> Vec<Case> {
    let secret = Uuid::now_v7();
    sqlx::query("INSERT INTO secrets (id, org_id, workspace_id, name, kind, host_pattern, injection, value_source, value_ref) VALUES ($1,$2,$3,'k','generic','api.example.com','{\"kind\":\"bearer\"}','managed','x:')")
        .bind(secret).bind(w.org).bind(w.ws).execute(w.h.pool()).await.unwrap();
    // A draft with one rule, so that publishing has something to promote.
    let (s, v) = w
        .call(
            Method::PUT,
            &format!("/v1/workspaces/{}/policy/draft", w.ws),
            Some(json!({"revision": 0, "default_action": "none", "rules": [
                {"name": "r1", "action": "allow",
                 "targets": [{"type": "network", "host": "a.example.com"}]}]})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (org, ws, agent) = (w.org, w.ws, w.agent);
    let slug = org.simple().to_string();
    vec![
        Case {
            action: "policy.publish",
            method: Method::POST,
            uri: format!("/v1/workspaces/{ws}/policy/publish"),
            body: Some(json!({"revision": 1})),
            probe: format!(
                "SELECT coalesce(max(number),0)::text FROM policy_generations WHERE workspace_id = '{ws}' AND status = 'published'"
            ),
        },
        Case {
            action: "grant.create",
            method: Method::PUT,
            uri: format!("/v1/agents/{agent}/grants/secrets/{secret}"),
            body: None,
            probe: format!("SELECT count(*)::text FROM grants WHERE agent_id = '{agent}'"),
        },
        Case {
            action: "member.update",
            method: Method::PATCH,
            uri: format!("/v1/orgs/{org}/members/{}", w.member_id),
            body: Some(json!({"role": "admin"})),
            probe: format!(
                "SELECT role FROM org_members WHERE org_id = '{org}' AND user_id = '{}'",
                w.member_id
            ),
        },
        Case {
            action: "workspace_access.set",
            method: Method::PUT,
            uri: format!("/v1/workspaces/{ws}/access/{}", w.outsider_id),
            body: Some(json!({"role": "member"})),
            probe: format!(
                "SELECT count(*)::text FROM workspace_access WHERE workspace_id = '{ws}'"
            ),
        },
        Case {
            action: "invitation.create",
            method: Method::POST,
            uri: format!("/v1/orgs/{org}/invitations"),
            body: Some(json!({"email": "new@x.io", "role": "member"})),
            probe: format!("SELECT count(*)::text FROM invitations WHERE org_id = '{org}'"),
        },
        Case {
            action: "api_key.create",
            method: Method::POST,
            uri: format!("/v1/orgs/{org}/api-keys"),
            body: Some(json!({"name": "ci"})),
            probe: format!("SELECT count(*)::text FROM api_keys WHERE org_id = '{org}'"),
        },
        Case {
            action: "approval.decide",
            method: Method::POST,
            uri: format!("/v1/approvals/{}/decision", w.approval),
            body: Some(json!({"decision": "approve"})),
            probe: format!("SELECT status FROM approvals WHERE id = '{}'", w.approval),
        },
        Case {
            action: "agent.freeze",
            method: Method::POST,
            uri: format!("/v1/agents/{agent}/freeze"),
            body: Some(json!({"reason": "incident"})),
            probe: format!("SELECT (frozen_at IS NOT NULL)::text FROM agents WHERE id = '{agent}'"),
        },
        Case {
            action: "org.update",
            method: Method::PATCH,
            uri: format!("/v1/orgs/{org}"),
            body: Some(json!({"name": "Renamed"})),
            probe: format!("SELECT name FROM organizations WHERE id = '{org}'"),
        },
        Case {
            action: "member.remove",
            method: Method::DELETE,
            uri: format!("/v1/orgs/{org}/members/{}", w.removable_id),
            body: None,
            probe: format!("SELECT count(*)::text FROM org_members WHERE org_id = '{org}'"),
        },
        // Last: it deletes the organization (and with it the rows the other cases looked at).
        Case {
            action: "org.delete",
            method: Method::DELETE,
            uri: format!("/v1/orgs/{org}"),
            body: Some(json!({"confirm": slug})),
            probe: format!("SELECT count(*)::text FROM organizations WHERE id = '{org}'"),
        },
    ]
}

#[tokio::test]
async fn failed_audit_write_fails_the_action_and_changes_nothing() {
    let w = world().await;
    let cases = cases(&w).await;
    // Every case names a critical action.
    for c in &cases {
        assert!(
            is_critical(c.action),
            "{} is not in the critical set",
            c.action
        );
    }
    w.fail_critical_audit().await;
    let mut before = Vec::new();
    for c in &cases {
        before.push(w.probe(&c.probe).await);
    }
    for (c, b) in cases.iter().zip(&before) {
        let (s, v) = w.call(c.method.clone(), &c.uri, c.body.clone()).await;
        assert_eq!(
            (s, code(&v)),
            (503, "audit_unavailable"),
            "{}: expected a refused action, got {s} {v}",
            c.action
        );
        assert_eq!(&w.probe(&c.probe).await, b, "{}: state changed", c.action);
    }
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE critical")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0, "no critical row may exist for a refused action");

    // The database is back: the same calls succeed and write critical rows in the same step.
    w.restore_audit().await;
    for (c, b) in cases.iter().zip(&before) {
        let (s, v) = w.call(c.method.clone(), &c.uri, c.body.clone()).await;
        assert!(matches!(s, 200 | 201 | 204), "{}: {s} {v}", c.action);
        if c.action != "org.delete" {
            assert_ne!(&w.probe(&c.probe).await, b, "{}: no change", c.action);
        }
        let (crit, outcome): (bool, String) = sqlx::query_as(
            "SELECT critical, outcome FROM audit_log WHERE action = $1 ORDER BY created_at DESC LIMIT 1",
        )
        .bind(c.action)
        .fetch_one(w.h.pool())
        .await
        .unwrap_or_else(|e| panic!("{}: no audit row ({e})", c.action));
        assert!(
            crit && outcome == "success",
            "{}: critical row expected",
            c.action
        );
    }
}

#[tokio::test]
async fn best_effort_actions_still_succeed_while_critical_audit_fails() {
    let w = world().await;
    w.fail_critical_audit().await;
    // Workspace rename is best-effort (`workspace.update` is not in the set).
    assert!(!is_critical("workspace.update"));
    let (s, v) = w
        .call(
            Method::PATCH,
            &format!("/v1/workspaces/{}", w.ws),
            Some(json!({"name": "Renamed"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let mut seen = 0_i64;
    for _ in 0..50 {
        seen = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE action = 'workspace.update' AND NOT critical",
        )
        .fetch_one(w.h.pool())
        .await
        .unwrap();
        if seen > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(seen, 1, "the best-effort row is still written");
}

#[tokio::test]
async fn critical_flag_is_derived_and_filterable() {
    let w = world().await;
    let put = |action: &'static str, outcome: &'static str| {
        let pool = w.h.pool().clone();
        let (org, actor) = (w.org, w.owner_id);
        async move {
            aurora_db::repo::audit::insert(
                &pool,
                &aurora_db::repo::audit::NewAudit {
                    org_id: Some(org),
                    workspace_id: None,
                    actor_kind: "user",
                    actor_id: Some(actor),
                    actor_label: "o@x.io",
                    action,
                    target_kind: None,
                    target_id: None,
                    outcome,
                    source: "api",
                    ip: None,
                    metadata: &json!({}),
                },
            )
            .await
            .unwrap();
        }
    };
    put("policy.publish", "success").await;
    put("policy.publish", "denied").await; // a refused attempt is not a critical row
    put("agent.create", "success").await;
    let uri = format!("/v1/orgs/{}/audit", w.org);
    let (s, all) = w.call(Method::GET, &uri, None).await;
    assert_eq!(s, 200);
    assert_eq!(all["items"].as_array().unwrap().len(), 3);
    let (s, only) = w
        .call(Method::GET, &format!("{uri}?critical=true"), None)
        .await;
    assert_eq!(s, 200);
    let items = only["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{only}");
    assert_eq!(items[0]["action"], "policy.publish");
    assert_eq!(items[0]["outcome"], "success");
    assert_eq!(items[0]["critical"], true);
    let flags: Vec<bool> = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["critical"].as_bool().unwrap())
        .collect();
    assert_eq!(flags.iter().filter(|f| **f).count(), 1);
    // `critical=false` lists everything, like the absent parameter.
    let (_, f) = w
        .call(Method::GET, &format!("{uri}?critical=false"), None)
        .await;
    assert_eq!(f["items"].as_array().unwrap().len(), 3);
}
