#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
use aurora_crypto::{SigningKey, generate_signing_key};
use aurora_registry::builder::PackageBuilder;
use axum::body::Body;
use http::{Method, Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::postgres::PgListener;
use tower::ServiceExt;
use uuid::Uuid;

const YAML: &str = r#"package: { id: acme, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: acme, name: P }
oauth_clients:
  - id: goog
    authorize_url: https://accounts.example.com/auth
    token_url: https://accounts.example.com/token
    fields:
      - { name: client_id, label: Client ID }
      - { name: client_secret, label: Secret, secret: true }
apps:
  - about: { id: acme_app, name: Acme, description: D, category: dev }
    credentials: [ { name: api_token, secret: true } ]
    connect:
      - id: api_key
        kind: fields
        fields: [ { name: api_token, label: T, type: secret, required: true } ]
      - id: oauth
        kind: oauth2
        client: goog
    agent: { default_blocklist: [ "blocked.example" ] }
    hosts:
      - match: { host: "api.acme.example" }
        inject:
          - bearer: "{credentials.api_token}"
"#;

struct W {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    owner: String,
    member: String,
    admin: String,
}

async fn world() -> W {
    let h = Harness::new().await;
    let root = h.user("root@x.io", true).await;
    let (_, admin_tok) = h.good_session(root).await;
    let admin = cookie(&admin_tok);
    let key: SigningKey = generate_signing_key().unwrap();
    let hd = [("cookie", admin.as_str()), ("origin", ORIGIN)];
    let (s, _, v) = h
        .send(
            Method::POST,
            "/v1/admin/trusted-keys",
            &hd,
            Some(json!({"publisher": "acme", "name": "k", "public_key": PackageBuilder::key_id(&key)})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let mut b = PackageBuilder::new(YAML);
    b.sign(&key, &PackageBuilder::key_id(&key));
    let boundary = "XB";
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"package\"; filename=\"p\"\r\nContent-Type: application/gzip\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(&b.build().unwrap());
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let req = Request::builder()
        .method(Method::POST)
        .uri("/v1/admin/plugins")
        .header("cookie", &admin)
        .header("origin", ORIGIN)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();
    let res = h.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), 201);
    let v: Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(v["status"], "pending_approval", "{}", v["violations"]);
    let (s, _, v) = h
        .send(
            Method::POST,
            &format!("/v1/admin/plugins/{}/approve", v["id"].as_str().unwrap()),
            &hd,
            Some(json!({})),
        )
        .await;
    assert_eq!(s, 200, "{v}");

    let org = h.org().await;
    let ws = h.workspace(org).await;
    let o = h.user("owner@x.io", false).await;
    h.member(org, o, "owner", "active").await;
    h.bind_ws(ws, o, "owner").await;
    let m = h.user("mem@x.io", false).await;
    h.member(org, m, "member", "active").await;
    h.bind_ws(ws, m, "member").await;
    let (_, ot) = h.good_session(o).await;
    let (_, mt) = h.good_session(m).await;
    W {
        h,
        org,
        ws,
        owner: cookie(&ot),
        member: cookie(&mt),
        admin,
    }
}

impl W {
    async fn call(&self, who: &str, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        let (s, _, v) = self
            .h
            .send(m, uri, &[("cookie", who), ("origin", ORIGIN)], body)
            .await;
        (s.as_u16(), v)
    }

    async fn conn(&self, ws: Option<Uuid>) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref) VALUES ($1,$2,$3,'acme_app','acme','1.0.0','c','active','x')")
            .bind(id).bind(self.org).bind(ws).execute(self.h.pool()).await.unwrap();
        id
    }
}

#[tokio::test]
async fn catalogue_availability_and_counts() {
    let w = world().await;
    w.conn(Some(w.ws)).await;
    let uri = format!("/v1/apps?workspace_id={}", w.ws);
    let (s, v) = w.call(&w.member, Method::GET, &uri, None).await;
    assert_eq!(s, 200, "{v}");
    let a = &v["items"][0];
    assert_eq!(
        (
            a["app_id"].as_str(),
            a["available"].as_bool(),
            a["connection_count"].as_i64()
        ),
        (Some("acme_app"), Some(true), Some(1))
    );
    let (s, v) = w
        .call(
            &w.member,
            Method::GET,
            &format!("/v1/apps/acme_app?workspace_id={}", w.ws),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["connect"][1]["client_key"], "goog");
    assert_eq!(v["hosts"][0]["claim_status"], "active");
    assert_eq!(
        w.call(&w.member, Method::GET, "/v1/apps/nope", None)
            .await
            .0,
        404
    );

    // member cannot change availability; owner restricts the org.
    let put = Some(json!({"posture": "restricted", "allow": []}));
    let p = format!("/v1/orgs/{}/app-availability", w.org);
    assert_eq!(w.call(&w.member, Method::PUT, &p, put.clone()).await.0, 403);
    let mut l = PgListener::connect_with(w.h.pool()).await.unwrap();
    l.listen("aurora_gateway_invalidate").await.unwrap();
    assert_eq!(w.call(&w.owner, Method::PUT, &p, put).await.0, 200);
    assert!(l.recv().await.is_ok());
    let (_, v) = w.call(&w.member, Method::GET, &uri, None).await;
    assert_eq!(v["items"][0]["available"], false);
    let put = Some(
        json!({"posture": "restricted", "allow": [{"app_id": "acme_app", "workspace_id": null}]}),
    );
    assert_eq!(w.call(&w.owner, Method::PUT, &p, put).await.0, 200);
    let (_, v) = w.call(&w.member, Method::GET, &uri, None).await;
    assert_eq!(v["items"][0]["available"], true);
    let (s, v) = w.call(&w.owner, Method::GET, &p, None).await;
    assert_eq!((s, v["allow"].as_array().unwrap().len()), (200, 1));
    let bad = Some(json!({"posture": "x", "allow": []}));
    assert_eq!(w.call(&w.owner, Method::PUT, &p, bad).await.0, 400);
}

#[tokio::test]
async fn connection_patch_and_delete() {
    let w = world().await;
    let id = w.conn(Some(w.ws)).await;
    let org_conn = w.conn(None).await;
    // blocklist rule + grant-less derived rule for the connection
    sqlx::query("INSERT INTO derived_rules (id, org_id, workspace_id, source_kind, source_id, ordinal, logical_id, rule) VALUES ($1,$2,$3,'blocklist',$4,0,$5,'{}')")
        .bind(Uuid::now_v7()).bind(w.org).bind(w.ws).bind(id).bind(Uuid::now_v7())
        .execute(w.h.pool()).await.unwrap();
    let u = format!("/v1/connections/{id}");
    // I-27: reads follow the matrix, so a plain workspace member cannot read the connection
    let (s, v) = w.call(&w.member, Method::GET, &u, None).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("forbidden")),
        "{v}"
    );
    let (s, v) = w.call(&w.owner, Method::GET, &u, None).await;
    assert_eq!(
        (s, v["injecting"][0]["host"].as_str()),
        (200, Some("api.acme.example")),
        "{v}"
    );
    assert!(v.get("credentials_ref").is_none());
    assert_eq!(
        w.call(&w.member, Method::PATCH, &u, Some(json!({"label": "n"})))
            .await
            .0,
        403
    );
    let mut l = PgListener::connect_with(w.h.pool()).await.unwrap();
    l.listen("aurora_gateway_invalidate").await.unwrap();
    let (s, v) = w
        .call(
            &w.owner,
            Method::PATCH,
            &u,
            Some(json!({"status": "disabled"})),
        )
        .await;
    assert_eq!((s, v["status"].as_str()), (200, Some("disabled")), "{v}");
    assert!(l.recv().await.unwrap().payload().contains(&id.to_string()));
    let (_, v) = w
        .call(
            &w.owner,
            Method::PATCH,
            &u,
            Some(json!({"status": "active"})),
        )
        .await;
    assert_eq!(v["status"], "active");
    assert_eq!(
        w.call(
            &w.owner,
            Method::PATCH,
            &u,
            Some(json!({"status": "orphaned"}))
        )
        .await
        .0,
        400
    );
    let (s, v) = w
        .call(
            &w.owner,
            Method::GET,
            &format!("/v1/workspaces/{}/connections", w.ws),
            None,
        )
        .await;
    assert_eq!((s, v["items"].as_array().unwrap().len()), (200, 1));
    let (_, v) = w
        .call(
            &w.owner,
            Method::GET,
            &format!("/v1/orgs/{}/connections", w.org),
            None,
        )
        .await;
    assert_eq!(v["items"][0]["id"], org_conn.to_string());

    assert_eq!(w.call(&w.owner, Method::DELETE, &u, None).await.0, 204);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM derived_rules WHERE source_id = $1")
        .bind(id)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
    assert_eq!(w.call(&w.owner, Method::GET, &u, None).await.0, 404);
}

/// (FIX-115) OAuth client PATCH is compare-and-set: two updates built from the same read cannot
/// both win. The loser gets 409 `conflict` and the first writer's change is kept.
#[tokio::test]
async fn oauth_client_patch_from_a_stale_read_is_a_409() {
    use aurora_api::audit::AuditActor;
    use aurora_api::domains::oauth_clients::{dto::PatchOAuthClientRequest, service};
    let w = world().await;
    let p = format!("/v1/orgs/{}/oauth-clients", w.org);
    let ok = json!({"package_id": "acme", "client_key": "goog",
        "fields": {"client_id": "cid", "client_secret": "s3cret"}});
    let (s, v) = w.call(&w.owner, Method::POST, &p, Some(ok)).await;
    assert_eq!(s, 201, "{v}");
    let id: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    // Two writers read the same row.
    let seen = aurora_db::repo::oauth_clients::get(w.h.pool(), id)
        .await
        .unwrap()
        .unwrap();
    let actor = AuditActor::system("test");
    let req = |secret: &str| PatchOAuthClientRequest {
        fields: Some([("client_secret".to_owned(), secret.to_owned())].into()),
        enabled: None,
    };
    service::patch(&w.h.state, &seen, req("first-secret"), &actor)
        .await
        .unwrap();
    let ref_after_first: String =
        sqlx::query_scalar("SELECT secret_ref FROM app_oauth_clients WHERE id = $1")
            .bind(id)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    let lost = service::patch(&w.h.state, &seen, req("second-secret"), &actor)
        .await
        .unwrap_err();
    assert_eq!(lost.code(), "conflict");
    // The winner's reference is untouched.
    let ref_now: String =
        sqlx::query_scalar("SELECT secret_ref FROM app_oauth_clients WHERE id = $1")
            .bind(id)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(ref_now, ref_after_first);
}

#[tokio::test]
async fn oauth_clients_validated_sealed_and_protected() {
    let w = world().await;
    let p = format!("/v1/orgs/{}/oauth-clients", w.org);
    let ok = json!({"package_id": "acme", "client_key": "goog", "fields": {"client_id": "cid", "client_secret": "s3cret"}});
    assert_eq!(
        w.call(&w.member, Method::POST, &p, Some(ok.clone()))
            .await
            .0,
        403
    );
    let bad = json!({"package_id": "acme", "client_key": "goog", "fields": {"client_id": "cid", "zzz": "1"}});
    let (s, v) = w.call(&w.owner, Method::POST, &p, Some(bad)).await;
    assert_eq!(s, 400, "{v}");
    let (s, v) = w.call(&w.owner, Method::POST, &p, Some(ok.clone())).await;
    assert_eq!(s, 201, "{v}");
    assert!(!v.to_string().contains("s3cret"));
    assert_eq!(v["fields"], json!({"client_id": "cid"}));
    let stored: String =
        sqlx::query_scalar("SELECT secret_ref || fields::text FROM app_oauth_clients")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert!(!stored.contains("s3cret"));
    assert_eq!(w.call(&w.owner, Method::POST, &p, Some(ok)).await.0, 409);
    let id = v["id"].as_str().unwrap().to_owned();
    let (s, v) = w
        .call(
            &w.owner,
            Method::PATCH,
            &format!("/v1/oauth-clients/{id}"),
            Some(json!({"fields": {"client_secret": "n2"}, "enabled": false})),
        )
        .await;
    assert_eq!((s, v["enabled"].as_bool()), (200, Some(false)), "{v}");

    sqlx::query("UPDATE app_connections SET oauth_client_id = NULL")
        .execute(w.h.pool())
        .await
        .unwrap();
    let c = w.conn(None).await;
    sqlx::query("UPDATE app_connections SET oauth_client_id = $1 WHERE id = $2")
        .bind(id.parse::<Uuid>().unwrap())
        .bind(c)
        .execute(w.h.pool())
        .await
        .unwrap();
    let (s, v) = w
        .call(
            &w.owner,
            Method::DELETE,
            &format!("/v1/oauth-clients/{id}"),
            None,
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("client_in_use"))
    );
    sqlx::query("DELETE FROM app_connections")
        .execute(w.h.pool())
        .await
        .unwrap();
    assert_eq!(
        w.call(
            &w.owner,
            Method::DELETE,
            &format!("/v1/oauth-clients/{id}"),
            None
        )
        .await
        .0,
        204
    );

    // instance clients are instance-admin only
    let ia = "/v1/admin/oauth-clients";
    let body = json!({"package_id": "acme", "client_key": "goog", "fields": {"client_id": "i", "client_secret": "j"}});
    assert_eq!(
        w.call(&w.owner, Method::POST, ia, Some(body.clone()))
            .await
            .0,
        403
    );
    let (s, v) = w.call(&w.admin, Method::POST, ia, Some(body)).await;
    assert_eq!((s, v["scope"].as_str()), (201, Some("instance")), "{v}");
    let iid = v["id"].as_str().unwrap();
    assert_eq!(
        w.call(
            &w.owner,
            Method::DELETE,
            &format!("/v1/oauth-clients/{iid}"),
            None
        )
        .await
        .0,
        403
    );
    assert_eq!(
        w.call(
            &w.admin,
            Method::DELETE,
            &format!("/v1/oauth-clients/{iid}"),
            None
        )
        .await
        .0,
        204
    );
}
