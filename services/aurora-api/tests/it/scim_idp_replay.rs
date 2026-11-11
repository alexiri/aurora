#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Recorded IdP traffic replay (docs/18 §SCIM): the fixtures in
//! `tests/fixtures/scim/{entra,okta,keycloak}/provisioning.json` are request/response pairs
//! reconstructed from each vendor's documented provisioning behaviour (synthetic tenants and
//! users, no real data). The replay runs every pair against the real `/scim/v2` plane and
//! asserts the final state and the exact statuses the IdPs rely on (409 → PATCH dedupe, the
//! lost-response POST, token rotation mid-sync).

use crate::common::*;
use crate::scim_common::*;
use http::{Method, StatusCode};
use serde_json::{Value, json};
use std::path::PathBuf;
use uuid::Uuid;

const USERS: &str = "/scim/v2/Users";
const GROUPS: &str = "/scim/v2/Groups";

struct Pair {
    method: Method,
    path: String,
    body: Option<Value>,
    /// Expected status.
    status: u16,
}

fn load(vendor: &str) -> Vec<Value> {
    let p: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "tests/fixtures/scim",
        vendor,
        "provisioning.json",
    ]
    .iter()
    .collect();
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// Substitute the `«name»` placeholders in a JSON string.
fn subst(s: &str, vars: &[(&str, &str)]) -> String {
    let mut out = s.to_owned();
    for (k, v) in vars {
        out = out.replace(&format!("«{k}»"), v);
    }
    out
}

fn subst_value(v: &Value, vars: &[(&str, &str)]) -> Value {
    match v {
        Value::String(s) => Value::String(subst(s, vars)),
        Value::Array(a) => Value::Array(a.iter().map(|x| subst_value(x, vars)).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, x)| (subst(k, vars), subst_value(x, vars)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn pairs(vendor: &str, vars: &[(&str, &str)]) -> Vec<Pair> {
    load(vendor)
        .iter()
        .map(|step| {
            let req = &step["request"];
            let method = match req["method"].as_str().unwrap() {
                "GET" => Method::GET,
                "POST" => Method::POST,
                "PUT" => Method::PUT,
                "PATCH" => Method::PATCH,
                "DELETE" => Method::DELETE,
                m => panic!("unknown method {m}"),
            };
            let path = subst(req["path"].as_str().unwrap(), vars);
            let body = req.get("body").map(|b| subst_value(b, vars));
            let res = &step["response"];
            let status = res["status"].as_u64().unwrap() as u16;
            let _ = res.get("body");
            Pair {
                method,
                path,
                body,
                status,
            }
        })
        .collect()
}

/// The e-mail domain the vendor's fixture people use (the org allow-list must name it).
fn vendor_domain(vendor: &str) -> &'static str {
    match vendor {
        "entra" => "contoso.example",
        "okta" => "okta.example",
        "keycloak" => "keycloak.example",
        other => panic!("unknown vendor {other}"),
    }
}

/// Run one vendor's pairs against a fresh org+token; returns the resolved ids.
async fn replay(vendor: &str) -> (ScimFx, String, String) {
    let fx = ScimFx::with_domains(&[vendor_domain(vendor)]).await;
    // The ids the fixtures learn along the way.
    let mut vars: Vec<(String, String)> = vec![];
    let steps = pairs(vendor, &[]);
    for step in &steps {
        let path = {
            let mut p = step.path.clone();
            for (k, v) in &vars {
                p = p.replace(&format!("«{k}»"), v);
            }
            p
        };
        let body = step.body.as_ref().map(|b| {
            let mut s = b.to_string();
            for (k, v) in &vars {
                s = s.replace(&format!("«{k}»"), v);
            }
            serde_json::from_str::<Value>(&s).unwrap()
        });
        let (s, _hd, v) = fx.scim(step.method.clone(), &path, body).await;
        assert_eq!(
            s.as_u16(),
            step.status,
            "{vendor} {} {}: {v}",
            step.method,
            path
        );
        // A body response must carry the scim+json content type; 204s have none.
        if s != StatusCode::NO_CONTENT {
            // (the header assertions live in the protocol suite; here the state matters)
            let _ = v.clone();
        }
        // Learn ids: a create response teaches the fixture's placeholder for the next steps.
        if let Some(id) = v["id"].as_str() {
            if path.starts_with(USERS) && step.method == Method::POST {
                vars.push(("scim_id".into(), id.to_owned()));
            } else if path.starts_with(GROUPS) && step.method == Method::POST {
                vars.push(("group_id".into(), id.to_owned()));
            }
        }
    }
    let scim_id = vars
        .iter()
        .find(|(k, _)| k == "scim_id")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    let group_id = vars
        .iter()
        .find(|(k, _)| k == "group_id")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    (fx, scim_id, group_id)
}

#[tokio::test]
async fn entra_replay() {
    let (fx, scim_id, group_id) = replay("entra").await;
    // Final state: the link was created, renamed (family name), reactivated, group emptied,
    // the user deleted (tombstone).
    let id: Uuid = scim_id.parse().unwrap();
    let row: (String, String, Option<String>) =
        sqlx::query_as("SELECT state, user_name, external_id FROM scim_users WHERE id = $1")
            .bind(id)
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(
        row.0, "removed",
        "the replay ends with the directory delete"
    );
    assert_eq!(row.1, "maria@contoso.example");
    assert_eq!(row.2.as_deref(), Some("00aa11bb22cc"));
    // The group exists with no members.
    let gid: Uuid = group_id.parse().unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM group_members WHERE group_id = $1")
        .bind(gid)
        .fetch_one(fx.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn okta_replay() {
    let (fx, scim_id, group_id) = replay("okta").await;
    // The duplicate create was 409 (the fixture asserts the status); the link survived it.
    let id: Uuid = scim_id.parse().unwrap();
    let state: String = sqlx::query_scalar("SELECT state FROM scim_users WHERE id = $1")
        .bind(id)
        .fetch_one(fx.h.pool())
        .await
        .unwrap();
    assert_eq!(state, "removed", "ends with the app unassignment");
    // The group's member was added and removed again.
    let gid: Uuid = group_id.parse().unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM group_members WHERE group_id = $1")
        .bind(gid)
        .fetch_one(fx.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn keycloak_replay() {
    let (fx, scim_id, group_id) = replay("keycloak").await;
    let id: Uuid = scim_id.parse().unwrap();
    let (state, ext): (String, Option<String>) =
        sqlx::query_as("SELECT state, external_id FROM scim_users WHERE id = $1")
            .bind(id)
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(state, "removed");
    // The later PUTs (deactivate/reactivate) carry no externalId and updates store exactly
    // what the IdP asserts: the create-time externalId was replaced by null.
    assert_eq!(ext.as_deref(), None);
    // The group was renamed to "Platform Core".
    let gid: Uuid = group_id.parse().unwrap();
    let name: String = sqlx::query_scalar("SELECT name FROM groups WHERE id = $1")
        .bind(gid)
        .fetch_one(fx.h.pool())
        .await
        .unwrap();
    assert_eq!(name, "Platform Core");
}

#[tokio::test]
async fn token_rotation_mid_sync() {
    // The IdP rotates its token mid-sync: the old token dies at once, the new one continues.
    let fx = ScimFx::new().await;
    let new_token = mint_token(&fx.h, &fx.owner.cookie, fx.org).await;
    let (s, _, v) = fx
        .scim(Method::POST, USERS, Some(json!({"userName": "rot@x.io"})))
        .await;
    assert_eq!(s, 201, "{v}");
    let old_id = v["id"].as_str().unwrap().to_owned();
    // Revoke the old token (the new one survives: match its display prefix).
    let (s, v) = fx
        .mgmt(
            Method::GET,
            &format!("/v1/orgs/{}/scim/tokens", fx.org),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let new_prefix = aurora_core::token::display_prefix(&new_token);
    for t in v["tokens"].as_array().unwrap() {
        if t["display_prefix"].as_str() == Some(new_prefix.as_str()) {
            continue;
        }
        if t["revoked"].as_bool() != Some(true) {
            let tid: Uuid = t["id"].as_str().unwrap().parse().unwrap();
            let (s, _) = fx
                .mgmt(Method::DELETE, &format!("/v1/scim/tokens/{tid}"), None)
                .await;
            assert_eq!(s, 204);
        }
    }
    // The old token is dead at once.
    let (s, _, _) = fx.scim(Method::GET, USERS, None).await;
    assert_eq!(s, 401);
    // The new token continues the sync without missing a beat.
    let (s, _, v) =
        fx.h.send(
            Method::GET,
            &format!("{USERS}/{old_id}"),
            &[("authorization", &bearer(&new_token))],
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, _, v) =
        fx.h.send(
            Method::POST,
            GROUPS,
            &[("authorization", &bearer(&new_token))],
            Some(json!({
                "schemas": ["urn:ietf:params:scim:schemas:core:2.0:Group"],
                "displayName": "post-rotation",
            })),
        )
        .await;
    assert_eq!(s, 201, "{v}");
}

#[tokio::test]
async fn repeated_post_after_lost_response_dedupes() {
    // The IdP retries the create after a lost response: the retry is 409 and the state has
    // exactly one link.
    let fx = ScimFx::new().await;
    let body = json!({"userName": "retry@x.io", "displayName": "R"});
    let (s, _, v) = fx.scim(Method::POST, USERS, Some(body.clone())).await;
    assert_eq!(s, 201, "{v}");
    let first = v["id"].as_str().unwrap().to_owned();
    let (s, _, v2) = fx.scim(Method::POST, USERS, Some(body)).await;
    assert_eq!(s, 409, "{v2}");
    assert_eq!(scim_type(&v2), Some("uniqueness"), "{v2}");
    // Exactly one link and the probe finds it.
    let n = count(
        &fx.h,
        "SELECT count(*) FROM scim_users WHERE org_id = $1 AND state != 'removed'",
        fx.org,
    )
    .await;
    assert_eq!(n, 1);
    let (s, _, v3) = fx
        .scim(
            Method::GET,
            &format!("{USERS}?filter=userName%20eq%20%22retry%40x.io%22"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v3}");
    assert_eq!(v3["totalResults"], 1, "{v3}");
    assert_eq!(v3["Resources"][0]["id"], first.as_str(), "{v3}");
}
