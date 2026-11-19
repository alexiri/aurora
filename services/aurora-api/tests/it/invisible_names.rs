#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-181 F6 (D-396, C-1041): no user-set name or label may contain an invisible formatting
//! character (`sanitize::is_stripped`: zero-width space, bidi override, soft hyphen, BOM …).
//! Organisation and workspace names already refused them; every other name now goes through the one
//! `aurora_core::names::clean`. The core crate's unit tests cover the character table; these ITs
//! prove the routes use it (a plain name still works, so the refusal is the character).

use crate::common::*;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

/// Characters a person cannot see: zero-width space, right-to-left override, soft hyphen.
const INVISIBLE: [&str; 3] = ["Ops\u{200B}", "\u{202E}Ops", "O\u{00AD}ps"];

struct Rig {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    agent: Uuid,
    cookie: String,
}

async fn rig() -> Rig {
    let h = Harness::new().await;
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let owner = h.user("owner@x.io", false).await;
    h.member(org, owner, "owner", "active").await;
    let agent = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) \
         VALUES ($1,$2,$3,'bot','Bot','byo','medium','')",
    )
    .bind(agent)
    .bind(org)
    .bind(ws)
    .execute(h.pool())
    .await
    .unwrap();
    let (_, tok) = h.good_session(owner).await;
    Rig {
        cookie: cookie(&tok),
        h,
        org,
        ws,
        agent,
    }
}

async fn send(r: &Rig, method: Method, uri: &str, body: Value) -> (u16, Value) {
    let (s, _, v) =
        r.h.send(
            method,
            uri,
            &[("cookie", r.cookie.as_str()), ("origin", ORIGIN)],
            Some(body),
        )
        .await;
    (s.as_u16(), v)
}

fn field_paths(v: &Value) -> Vec<String> {
    v["error"]["details"]["fields"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|f| f["path"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn group_names_refuse_invisible_formatting_characters() {
    let r = rig().await;
    let uri = format!("/v1/orgs/{}/groups", r.org);
    for bad in INVISIBLE {
        let (s, v) = send(&r, Method::POST, &uri, json!({"name": bad})).await;
        assert_eq!(s, 400, "group {bad:?}: {v}");
        assert_eq!(field_paths(&v), ["/name"], "{v}");
    }
    let (s, v) = send(&r, Method::POST, &uri, json!({"name": "Ops"})).await;
    assert_eq!(s, 201, "{v}");
}

#[tokio::test]
async fn api_key_names_refuse_invisible_formatting_characters() {
    let r = rig().await;
    for uri in [
        format!("/v1/orgs/{}/api-keys", r.org),
        format!("/v1/workspaces/{}/api-keys", r.ws),
    ] {
        for bad in INVISIBLE {
            let (s, v) = send(&r, Method::POST, &uri, json!({"name": bad})).await;
            assert_eq!(s, 400, "{uri} {bad:?}: {v}");
        }
        let (s, v) = send(&r, Method::POST, &uri, json!({"name": "ci"})).await;
        assert_eq!(s, 201, "{uri}: {v}");
    }
}

#[tokio::test]
async fn agent_names_refuse_invisible_formatting_characters() {
    let r = rig().await;
    let uri = format!("/v1/agents/{}", r.agent);
    for bad in INVISIBLE {
        let (s, v) = send(&r, Method::PATCH, &uri, json!({"name": bad})).await;
        assert_eq!(s, 400, "agent {bad:?}: {v}");
    }
    let (s, v) = send(&r, Method::PATCH, &uri, json!({"name": "Renamed"})).await;
    assert_eq!(s, 200, "{v}");
}
