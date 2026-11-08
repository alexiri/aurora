#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! D-74 / SEC-59: self-hosted app instances through the API: registration (authorization and
//! every refusal), the probe, connect and BYOC binding, deletion, and the catalogue.

use crate::common::*;
use aurora_crypto::{SigningKey, generate_signing_key};
use aurora_registry::builder::PackageBuilder;
use aurora_testkit::wiremock::matchers::{method, path};
use aurora_testkit::wiremock::{Mock, MockServer, ResponseTemplate};
use axum::body::Body;
use http::{Method, Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const SELFHOST: &str = r#"package: { id: selfhost, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: acme, name: Selfhost }
oauth_clients:
  - id: oa
    authorize_url: "https://{instance}/oauth/authorize"
    token_url: "https://{instance}/oauth/token"
    pkce: true
    fields:
      - { name: client_id, label: Client ID }
      - { name: client_secret, label: Secret, secret: true }
apps:
  - about: { id: desk, name: Desk, description: D, category: dev, web_hosts: [saas.desk.example] }
    instances:
      label: Desk instance
      default_host: saas.desk.example
      probe: { method: GET, path: /api/health, expect_status: [200] }
    credentials:
      - { name: api_token, secret: true, optional: true }
      - { name: access_token, secret: true, optional: true }
      - { name: refresh_token, secret: true, optional: true }
    metadata: [ { name: username } ]
    connect:
      - id: api_key
        kind: fields
        fields: [ { name: api_token, label: T, type: secret, required: true } ]
        probe: { url: "https://{instance}/api/me", label: /name, metadata: { username: /name } }
      - id: oauth
        kind: oauth2
        client: oa
        scopes: [ { scope: read, name: Read, access: read } ]
        default_scopes: [read]
        callback: query
        probe: { url: "https://{instance}/api/me", label: /name }
    hosts:
      - match: { host: "{instance}", path_prefix: /api }
        inject:
          - bearer: "{credentials.api_token}"
    tools:
      read:
        items:
          - { id: list_things, name: List, host: "{instance}", path: /api/things, methods: [GET] }
  - about: { id: nodefault, name: No default, description: D, category: dev }
    instances: { label: Self-hosted only }
    credentials: [ { name: api_token, secret: true } ]
    connect:
      - id: api_key
        kind: fields
        fields: [ { name: api_token, label: T, type: secret, required: true } ]
        probe: { url: "https://{instance}/api/me", label: /name }
    hosts:
      - match: { host: "{instance}" }
        inject:
          - bearer: "{credentials.api_token}"
"#;

const PLAIN: &str = r#"package: { id: plainpkg, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: acme, name: Plain }
apps:
  - about: { id: plain, name: Plain, description: D, category: dev, web_hosts: [plain.example.test] }
    credentials: [ { name: api_token, secret: true } ]
    connect:
      - id: api_key
        kind: fields
        fields: [ { name: api_token, label: T, type: secret, required: true } ]
    hosts:
      - match: { host: plain.example.test }
        inject:
          - bearer: "{credentials.api_token}"
      - match: { host: "*.wild.example.test" }
        inject:
          - bearer: "{credentials.api_token}"
"#;

struct W {
    h: Harness,
    up: MockServer,
    org: Uuid,
    org_b: Uuid,
    ws: Uuid,
    root: String,
    owner: String,
    admin: String,
    member: String,
    owner_b: String,
    outsider: String,
}

async fn install(h: &Harness, who: &str, yaml: &str, key: &SigningKey) {
    let mut b = PackageBuilder::new(yaml);
    b.sign(key, &PackageBuilder::key_id(key));
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
        .header("cookie", who)
        .header("origin", ORIGIN)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();
    let res = h.app.clone().oneshot(req).await.unwrap();
    let v: Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(v["status"], "pending_approval", "{}", v["violations"]);
    let (s, _, v) = h
        .send(
            Method::POST,
            &format!("/v1/admin/plugins/{}/approve", v["id"].as_str().unwrap()),
            &[("cookie", who), ("origin", ORIGIN)],
            Some(json!({})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
}

async fn world_with(probe_override: bool) -> W {
    let up = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/health"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&up)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"name": "alice"})))
        .mount(&up)
        .await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"access_token": "at-1", "refresh_token": "rt-1", "expires_in": 3600}),
        ))
        .mount(&up)
        .await;
    // The hook host's host is a reserved name (09 §6): a registered instance may not equal it.
    let mut h = Harness::with_config(|c| {
        c.hookhost_url = "http://hooks.aurora.example.org:4104".into();
    })
    .await;
    if probe_override {
        h.state = h.state.clone().with_connect_override(&up.uri());
        h.app = test_router(h.state.clone());
    }
    let rootu = h.user("root@x.io", true).await;
    let (_, rt) = h.good_session(rootu).await;
    let root = cookie(&rt);
    let key: SigningKey = generate_signing_key().unwrap();
    let (s, _, v) = h
        .send(
            Method::POST,
            "/v1/admin/trusted-keys",
            &[("cookie", &root), ("origin", ORIGIN)],
            Some(json!({"publisher": "acme", "name": "k", "public_key": PackageBuilder::key_id(&key)})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    install(&h, &root, SELFHOST, &key).await;
    install(&h, &root, PLAIN, &key).await;
    let org = h.org().await;
    let org_b = h.org().await;
    let ws = h.workspace(org).await;
    let mut cookies = Vec::new();
    for (email, o, role) in [
        ("owner@x.io", org, "owner"),
        ("admin@x.io", org, "admin"),
        ("mem@x.io", org, "member"),
        ("ownerb@x.io", org_b, "owner"),
    ] {
        let u = h.user(email, false).await;
        h.member(o, u, role, "active").await;
        if o == org {
            h.bind_ws(ws, u, if role == "member" { "member" } else { "owner" })
                .await;
        }
        cookies.push(cookie(&h.good_session(u).await.1));
    }
    let out = h.user("out@x.io", false).await;
    let outsider = cookie(&h.good_session(out).await.1);
    let mut it = cookies.into_iter();
    W {
        h,
        up,
        org,
        org_b,
        ws,
        root,
        owner: it.next().unwrap(),
        admin: it.next().unwrap(),
        member: it.next().unwrap(),
        owner_b: it.next().unwrap(),
        outsider,
    }
}

async fn world() -> W {
    world_with(true).await
}

impl W {
    async fn call(&self, who: &str, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        let (s, _, v) = self
            .h
            .send(m, uri, &[("cookie", who), ("origin", ORIGIN)], body)
            .await;
        (s.as_u16(), v)
    }

    async fn register(&self, who: &str, org: Uuid, app: &str, host: &str) -> (u16, Value) {
        self.call(
            who,
            Method::POST,
            &format!("/v1/orgs/{org}/app-instances"),
            Some(json!({"app_id": app, "host": host, "label": format!("{host} label")})),
        )
        .await
    }

    async fn instance(&self, app: &str, host: &str) -> Uuid {
        let (s, v) = self.register(&self.owner, self.org, app, host).await;
        assert_eq!(s, 201, "{v}");
        v["id"].as_str().unwrap().parse().unwrap()
    }

    async fn connect(&self, who: &str, app: &str, body: Value) -> (u16, Vec<String>, Value) {
        let (s, hd, v) = self
            .h
            .send(
                Method::POST,
                &format!("/v1/apps/{app}/connect"),
                &[("cookie", who), ("origin", ORIGIN)],
                Some(body),
            )
            .await;
        let sc = hd
            .get_all("set-cookie")
            .iter()
            .map(|c| c.to_str().unwrap().to_owned())
            .collect();
        (s.as_u16(), sc, v)
    }

    fn body(&self, method_id: &str, instance: Option<Uuid>) -> Value {
        let mut b = json!({"scope": "workspace", "workspace_id": self.ws, "method_id": method_id});
        if method_id == "api_key" {
            b["fields"] = json!({"api_token": "tok-1"});
        }
        if let Some(i) = instance {
            b["instance_id"] = json!(i);
        }
        b
    }

    async fn audit_count(&self, action: &str) -> i64 {
        // audit rows of critical actions are written in the change's transaction; best-effort
        // ones may land a moment later
        for _ in 0..20 {
            let n: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = $1")
                .bind(action)
                .fetch_one(self.h.pool())
                .await
                .unwrap();
            if n > 0 {
                return n;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        0
    }
}

fn field_path(v: &Value) -> String {
    v["error"]["details"]["fields"][0]["path"]
        .as_str()
        .unwrap_or("")
        .to_owned()
}

#[tokio::test]
async fn registration_is_for_org_admins_and_instance_admins_only() {
    let w = world().await;
    let org = w.org;
    // org owner and admin register; a member and an outsider do not; anonymous gets 401.
    let (s, v) = w.register(&w.owner, org, "desk", "desk.acme.example").await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["org_id"], json!(org));
    assert_eq!(v["host"], "desk.acme.example");
    assert_eq!(v["status"], "active");
    assert_eq!(v["package_id"], "selfhost");
    assert_eq!(v["connections"], 0);
    assert_eq!(v["created_by"]["display_name"], "U");
    let (s, _) = w
        .register(&w.admin, org, "desk", "desk2.acme.example")
        .await;
    assert_eq!(s, 201);
    let (s, v) = w
        .register(&w.member, org, "desk", "desk3.acme.example")
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("forbidden")),
        "{v}"
    );
    assert_eq!(
        w.register(&w.outsider, org, "desk", "desk3.acme.example")
            .await
            .0,
        404
    );
    assert_eq!(
        w.register(&w.owner_b, org, "desk", "desk3.acme.example")
            .await
            .0,
        404
    );
    let anon =
        w.h.send(
            Method::POST,
            &format!("/v1/orgs/{org}/app-instances"),
            &[("origin", ORIGIN)],
            Some(json!({"app_id": "desk", "host": "d.acme.example", "label": "x"})),
        )
        .await;
    assert_eq!(anon.0.as_u16(), 401);
    // Instance-wide instances: only an instance admin; an org owner gets 403.
    let body = json!({"app_id": "desk", "host": "desk.wide.example", "label": "Wide"});
    let (s, _) = w
        .call(
            &w.owner,
            Method::POST,
            "/v1/admin/app-instances",
            Some(body.clone()),
        )
        .await;
    assert_eq!(s, 403);
    let (s, v) = w
        .call(&w.root, Method::POST, "/v1/admin/app-instances", Some(body))
        .await;
    assert_eq!((s, &v["org_id"]), (201, &Value::Null), "{v}");
    // Listing: every member sees the instance-wide ones and the org's own, never another org's.
    let (s, _) = w
        .register(&w.owner_b, w.org_b, "desk", "desk.other.example")
        .await;
    assert_eq!(s, 201);
    let (s, v) = w
        .call(
            &w.member,
            Method::GET,
            &format!("/v1/orgs/{org}/app-instances"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let hosts: Vec<&str> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["host"].as_str().unwrap())
        .collect();
    assert!(
        hosts.contains(&"desk.acme.example") && hosts.contains(&"desk.wide.example"),
        "{hosts:?}"
    );
    assert!(!hosts.contains(&"desk.other.example"), "{hosts:?}");
    let (s, v) = w
        .call(
            &w.member,
            Method::GET,
            &format!("/v1/orgs/{org}/app-instances?app_id=nodefault"),
            None,
        )
        .await;
    assert_eq!((s, v["items"].as_array().unwrap().len()), (200, 0));
    assert_eq!(
        w.call(
            &w.outsider,
            Method::GET,
            &format!("/v1/orgs/{org}/app-instances"),
            None
        )
        .await
        .0,
        404
    );
    assert_eq!(
        w.call(&w.owner, Method::GET, "/v1/admin/app-instances", None)
            .await
            .0,
        403
    );
    let (s, v) = w
        .call(&w.root, Method::GET, "/v1/admin/app-instances", None)
        .await;
    assert_eq!((s, v["items"].as_array().unwrap().len()), (200, 1), "{v}");
}

#[tokio::test]
async fn every_refusal_of_a_host_is_a_422_with_the_path_and_code() {
    let w = world().await;
    let org = w.org;
    // instance_host_invalid: not a lower-case DNS name, port, path, userinfo, IP literal,
    // `aurora.internal`, a single label, a trailing dot.
    for bad in [
        "Desk.Acme.Example",
        "desk.acme.example:8443",
        "desk.acme.example/path",
        "user@desk.acme.example",
        "https://desk.acme.example",
        "10.0.0.1",
        "127.0.0.1",
        "[::1]",
        "::1",
        "x.aurora.internal",
        "aurora.internal",
        "localhost",
        "desk.acme.example.",
        "desk..acme.example",
        "-desk.acme.example",
        "desk acme.example",
        "*.acme.example",
        "",
    ] {
        let (s, v) = w.register(&w.owner, org, "desk", bad).await;
        assert_eq!(s, 422, "{bad}: {v}");
        assert_eq!(v["error"]["code"], "instance_host_invalid", "{bad}: {v}");
        assert_eq!(field_path(&v), "/host", "{bad}");
    }
    // app_not_instance_bound: unknown app, an app without `instances`.
    for app in ["nope", "plain", ""] {
        let (s, v) = w.register(&w.owner, org, app, "desk.acme.example").await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (422, Some("app_not_instance_bound")),
            "{app}: {v}"
        );
        assert_eq!(field_path(&v), "/app_id");
    }
    // instance_host_conflict: another package's static claim (exact and wildcard), the app's own
    // default host, a governed/reserved host.
    for (host, kind, app) in [
        ("plain.example.test", "static_claim", Some("plain")),
        ("anything.wild.example.test", "static_claim", Some("plain")),
        ("saas.desk.example", "static_claim", Some("desk")),
        ("hooks.aurora.example.org", "llm_host", None),
    ] {
        let (s, v) = w.register(&w.owner, org, "desk", host).await;
        assert_eq!(s, 422, "{host}: {v}");
        assert_eq!(v["error"]["code"], "instance_host_conflict", "{host}: {v}");
        assert_eq!(field_path(&v), "/host");
        assert_eq!(
            v["error"]["details"]["conflict"]["kind"], kind,
            "{host}: {v}"
        );
        if let Some(a) = app {
            assert_eq!(v["error"]["details"]["conflict"]["app_id"], a, "{host}");
        }
    }
    // ... another instance of a scope that can see this one.
    let (s, _) = w.register(&w.owner, org, "desk", "desk.acme.example").await;
    assert_eq!(s, 201);
    let (s, v) = w
        .register(&w.admin, org, "nodefault", "desk.acme.example")
        .await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["details"]["conflict"]["kind"], "instance");
    // an instance-wide registration conflicts with any org's instance, an org's with instance-wide
    let (s, v) = w
        .call(
            &w.root,
            Method::POST,
            "/v1/admin/app-instances",
            Some(json!({"app_id": "desk", "host": "desk.acme.example", "label": "x"})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["details"]["conflict"]["kind"].as_str()),
        (422, Some("instance")),
        "{v}"
    );
    let (s, _) = w
        .call(
            &w.root,
            Method::POST,
            "/v1/admin/app-instances",
            Some(json!({"app_id": "desk", "host": "wide.example", "label": "x"})),
        )
        .await;
    assert_eq!(s, 201);
    let (s, v) = w
        .register(&w.owner_b, w.org_b, "desk", "wide.example")
        .await;
    assert_eq!(
        (s, v["error"]["details"]["conflict"]["kind"].as_str()),
        (422, Some("instance")),
        "{v}"
    );
    // ... but two organizations may each register the same self-hosted host
    let (s, _) = w
        .register(&w.owner_b, w.org_b, "desk", "desk.acme.example")
        .await;
    assert_eq!(s, 201);
    // unknown fields are refused
    let (s, v) = w
        .call(&w.owner, Method::POST, &format!("/v1/orgs/{org}/app-instances"),
            Some(json!({"app_id": "desk", "host": "z.acme.example", "label": "x", "status": "disabled"})))
        .await;
    assert!(s == 400 || s == 422, "{s} {v}");
    // nothing was written by the refusals
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM app_instances")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 3);
}

#[tokio::test]
async fn the_probe_runs_and_a_failure_registers_nothing() {
    let w = world().await;
    // 503 from the instance
    w.up.reset().await;
    Mock::given(method("GET"))
        .and(path("/api/health"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&w.up)
        .await;
    let (s, v) = w
        .register(&w.owner, w.org, "desk", "desk.acme.example")
        .await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["code"], "instance_probe_failed");
    assert_eq!(v["error"]["details"]["status"], 503);
    assert_eq!(v["error"]["details"]["reason"], "unexpected_status");
    assert_eq!(field_path(&v), "/host");
    // `nodefault` declares no probe: no request is made, so a dead upstream does not matter
    let (s, _) = w
        .register(&w.owner, w.org, "nodefault", "nd.acme.example")
        .await;
    assert_eq!(s, 201);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM app_instances")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 1);
    // healthy again
    w.up.reset().await;
    Mock::given(method("GET"))
        .and(path("/api/health"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&w.up)
        .await;
    let (s, _) = w
        .register(&w.owner, w.org, "desk", "desk.acme.example")
        .await;
    assert_eq!(s, 201);
    assert_eq!(w.up.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn the_probe_goes_through_the_destination_guard() {
    // No test override: the real guarded client. The name cannot resolve to a public address
    // (reserved `.invalid` TLD), so the probe fails and nothing is registered.
    let w = world_with(false).await;
    let (s, v) = w
        .register(&w.owner, w.org, "desk", "desk.acme.invalid")
        .await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["code"], "instance_probe_failed");
    let reason = v["error"]["details"]["reason"].as_str().unwrap();
    assert!(
        ["unreachable", "destination_refused"].contains(&reason),
        "{v}"
    );
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM app_instances")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn create_update_delete_are_audited() {
    let w = world().await;
    let id = w.instance("desk", "desk.acme.example").await;
    assert!(w.audit_count("app_instance.create").await >= 1);
    let row: (String, Option<Uuid>, Value) = sqlx::query_as(
        "SELECT target_id, org_id, metadata FROM audit_log WHERE action = 'app_instance.create'",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(row.0, id.to_string());
    assert_eq!(row.1, Some(w.org));
    assert_eq!(row.2["host"], "desk.acme.example");
    let (s, v) = w
        .call(
            &w.admin,
            Method::PATCH,
            &format!("/v1/app-instances/{id}"),
            Some(json!({"label": "Renamed"})),
        )
        .await;
    assert_eq!((s, v["label"].as_str()), (200, Some("Renamed")), "{v}");
    assert!(w.audit_count("app_instance.update").await >= 1);
    let (s, _) = w
        .call(
            &w.admin,
            Method::DELETE,
            &format!("/v1/app-instances/{id}"),
            None,
        )
        .await;
    assert_eq!(s, 204);
    assert!(w.audit_count("app_instance.delete").await >= 1);
    // an instance-wide change is audited without an org
    let (s, v) = w
        .call(
            &w.root,
            Method::POST,
            "/v1/admin/app-instances",
            Some(json!({"app_id": "desk", "host": "wide.example", "label": "x"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'app_instance.create' AND org_id IS NULL AND actor_kind = 'instance_admin'",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn patch_changes_label_and_status_but_never_the_host() {
    let w = world().await;
    let id = w.instance("desk", "desk.acme.example").await;
    let uri = format!("/v1/app-instances/{id}");
    let (s, v) = w
        .call(
            &w.admin,
            Method::PATCH,
            &uri,
            Some(json!({"status": "disabled"})),
        )
        .await;
    assert_eq!((s, v["status"].as_str()), (200, Some("disabled")), "{v}");
    // a disabled instance cannot be connected to
    let (s, _, v) = w
        .connect(&w.owner, "desk", w.body("api_key", Some(id)))
        .await;
    assert_eq!(s, 404, "{v}");
    // the host is immutable
    let (s, v) = w
        .call(
            &w.admin,
            Method::PATCH,
            &uri,
            Some(json!({"host": "evil.example"})),
        )
        .await;
    assert_eq!(s, 400, "{v}");
    let (s, v) = w
        .call(
            &w.admin,
            Method::PATCH,
            &uri,
            Some(json!({"status": "bogus"})),
        )
        .await;
    assert_eq!(s, 400, "{v}");
    // authority: a member 403, an outsider 404
    assert_eq!(
        w.call(&w.member, Method::PATCH, &uri, Some(json!({})))
            .await
            .0,
        403
    );
    assert_eq!(
        w.call(&w.outsider, Method::PATCH, &uri, Some(json!({})))
            .await
            .0,
        404
    );
    assert_eq!(w.call(&w.owner_b, Method::DELETE, &uri, None).await.0, 404);
    // instance-wide: org admins cannot change it, an instance admin can
    let (_, v) = w
        .call(
            &w.root,
            Method::POST,
            "/v1/admin/app-instances",
            Some(json!({"app_id": "desk", "host": "wide.example", "label": "x"})),
        )
        .await;
    let wide = format!("/v1/app-instances/{}", v["id"].as_str().unwrap());
    assert_eq!(
        w.call(&w.owner, Method::PATCH, &wide, Some(json!({"label": "y"})))
            .await
            .0,
        403
    );
    assert_eq!(w.call(&w.owner, Method::DELETE, &wide, None).await.0, 403);
    assert_eq!(
        w.call(&w.root, Method::PATCH, &wide, Some(json!({"label": "y"})))
            .await
            .0,
        200
    );
    assert_eq!(w.call(&w.root, Method::DELETE, &wide, None).await.0, 204);
}

#[tokio::test]
async fn connect_binds_the_connection_to_the_instance_or_the_default_host() {
    let w = world().await;
    let id = w.instance("desk", "desk.acme.example").await;
    // absent instance_id = default_host
    let (s, _, v) = w.connect(&w.owner, "desk", w.body("api_key", None)).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["connection"]["instance"], Value::Null);
    let injecting: Vec<&str> = v["connection"]["injecting"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["host"].as_str().unwrap())
        .collect();
    assert_eq!(injecting, ["saas.desk.example"]);
    // with instance_id: bound; the probe ran against the instance (label from its document)
    let (s, _, v) = w
        .connect(&w.owner, "desk", w.body("api_key", Some(id)))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["connection"]["instance"],
        json!({"id": id, "host": "desk.acme.example", "label": "desk.acme.example label"})
    );
    assert_eq!(v["connection"]["label"], "alice");
    let injecting: Vec<&str> = v["connection"]["injecting"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["host"].as_str().unwrap())
        .collect();
    assert_eq!(injecting, ["desk.acme.example"]);
    let cid: Uuid = v["connection"]["id"].as_str().unwrap().parse().unwrap();
    let stored: Option<Uuid> =
        sqlx::query_scalar("SELECT instance_id FROM app_connections WHERE id = $1")
            .bind(cid)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(stored, Some(id));
    // the connection list and the instance's connection count show it
    let (s, v) = w
        .call(
            &w.owner,
            Method::GET,
            &format!("/v1/workspaces/{}/connections", w.ws),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["instance"]["id"] == json!(id))
    );
    let (_, v) = w
        .call(
            &w.member,
            Method::GET,
            &format!("/v1/orgs/{}/app-instances", w.org),
            None,
        )
        .await;
    assert_eq!(v["items"][0]["connections"], 1);
    // no default host and no instance: 422 instance_required
    let (s, _, v) = w
        .connect(&w.owner, "nodefault", w.body("api_key", None))
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("instance_required")),
        "{v}"
    );
    // an instance of another app, another org's instance, an app without instances
    let nd = w.instance("nodefault", "nd.acme.example").await;
    let (s, _, _) = w
        .connect(&w.owner, "desk", w.body("api_key", Some(nd)))
        .await;
    assert_eq!(s, 404);
    let (_, v) = w
        .register(&w.owner_b, w.org_b, "desk", "desk.b.example")
        .await;
    let other: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let (s, _, _) = w
        .connect(&w.owner, "desk", w.body("api_key", Some(other)))
        .await;
    assert_eq!(s, 404);
    let (s, _, _) = w
        .connect(&w.owner, "desk", w.body("api_key", Some(Uuid::now_v7())))
        .await;
    assert_eq!(s, 404);
    let (s, _, v) = w
        .connect(&w.owner, "plain", w.body("api_key", Some(id)))
        .await;
    assert_eq!(s, 400, "{v}");
    // a reconnect keeps the instance and refuses another one
    let mut b = w.body("api_key", None);
    b["reconnect_connection_id"] = json!(cid);
    let (s, _, v) = w.connect(&w.owner, "desk", b.clone()).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["connection"]["instance"]["id"], json!(id));
    b["instance_id"] = json!(nd);
    let (s, _, _) = w.connect(&w.owner, "desk", b).await;
    assert_eq!(s, 400);
}

fn query_param(url: &str, name: &str) -> String {
    reqwest::Url::parse(url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

#[tokio::test]
async fn oauth_uses_only_a_client_bound_to_the_instance() {
    let w = world().await;
    let id = w.instance("desk", "desk.acme.example").await;
    let client = |instance: Option<Uuid>, cid: &str| {
        let mut b = json!({"package_id": "selfhost", "client_key": "oa", "fields": {"client_id": cid, "client_secret": "sec"}});
        if let Some(i) = instance {
            b["instance_id"] = json!(i);
        }
        b
    };
    let oauth_uri = format!("/v1/orgs/{}/oauth-clients", w.org);
    // No bound client: instance_oauth_client_required (the default-host method needs an unbound one)
    let mut b = w.body("oauth", Some(id));
    let (s, _, v) = w.connect(&w.owner, "desk", b.clone()).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("instance_oauth_client_required")),
        "{v}"
    );
    // BYOC binding validation (C-244): unknown instance, another org's instance, another package's
    let (s, v) = w
        .call(
            &w.owner,
            Method::POST,
            &oauth_uri,
            Some(client(Some(Uuid::now_v7()), "c")),
        )
        .await;
    assert_eq!(s, 400, "{v}");
    let (_, bv) = w
        .register(&w.owner_b, w.org_b, "desk", "desk.b.example")
        .await;
    let other: Uuid = bv["id"].as_str().unwrap().parse().unwrap();
    let (s, _) = w
        .call(
            &w.owner,
            Method::POST,
            &oauth_uri,
            Some(client(Some(other), "c")),
        )
        .await;
    assert_eq!(s, 400);
    // a bound client
    let (s, v) = w
        .call(
            &w.owner,
            Method::POST,
            &oauth_uri,
            Some(client(Some(id), "cid-instance")),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["instance_id"], json!(id));
    let bound: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    // one client per instance, but an unbound one may coexist
    let (s, _) = w
        .call(
            &w.owner,
            Method::POST,
            &oauth_uri,
            Some(client(Some(id), "again")),
        )
        .await;
    assert_eq!(s, 409);
    let (s, v) = w
        .call(
            &w.owner,
            Method::POST,
            &oauth_uri,
            Some(client(None, "cid-default")),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["instance_id"], Value::Null);
    let unbound: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    // the instance connect picks the bound client, never the unbound one
    b["oauth_client_id"] = json!(unbound);
    let (s, _, v) = w.connect(&w.owner, "desk", b.clone()).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("instance_oauth_client_required")),
        "{v}"
    );
    b.as_object_mut().unwrap().remove("oauth_client_id");
    let (s, sc, v) = w.connect(&w.owner, "desk", b).await;
    assert_eq!(s, 200, "{v}");
    let url = v["url"].as_str().unwrap().to_owned();
    assert!(
        url.starts_with("https://desk.acme.example/oauth/authorize?"),
        "{url}"
    );
    assert_eq!(query_param(&url, "client_id"), "cid-instance");
    // the callback exchanges the code on the instance (the override maps it to the mock)
    let ck = sc[0].split(';').next().unwrap().to_owned();
    let cb = format!(
        "/v1/oauth/callback?code=c1&state={}",
        query_param(&url, "state")
    );
    let req = Request::builder()
        .uri(&cb)
        .header("cookie", format!("{}; {ck}", w.owner))
        .body(Body::empty())
        .unwrap();
    let res = w.h.app.clone().oneshot(req).await.unwrap();
    let page =
        String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    assert!(page.contains("aurora:connected"), "{page}");
    let reqs = w.up.received_requests().await.unwrap();
    let tok: Vec<_> = reqs
        .iter()
        .filter(|r| r.url.path() == "/oauth/token")
        .collect();
    assert_eq!(tok.len(), 1);
    let form = String::from_utf8_lossy(&tok[0].body).into_owned();
    assert!(
        form.contains("client_id=cid-instance") && form.contains("client_secret=sec"),
        "{form}"
    );
    let (iid, cl): (Option<Uuid>, Option<Uuid>) =
        sqlx::query_as("SELECT instance_id, oauth_client_id FROM app_connections")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!((iid, cl), (Some(id), Some(bound)));
    // the default host never uses the instance-bound client (and uses the unbound one)
    let mut d = w.body("oauth", None);
    d["oauth_client_id"] = json!(bound);
    let (s, _, v) = w.connect(&w.owner, "desk", d.clone()).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("oauth_client_required")),
        "{v}"
    );
    d.as_object_mut().unwrap().remove("oauth_client_id");
    let (s, _, v) = w.connect(&w.owner, "desk", d).await;
    assert_eq!(s, 200, "{v}");
    assert!(
        v["url"]
            .as_str()
            .unwrap()
            .starts_with("https://saas.desk.example/oauth/authorize?"),
        "{v}"
    );
    assert_eq!(
        query_param(v["url"].as_str().unwrap(), "client_id"),
        "cid-default"
    );
}

#[tokio::test]
async fn deleting_an_instance_disables_its_connections_and_removes_its_clients() {
    let w = world().await;
    let id = w.instance("desk", "desk.acme.example").await;
    let (s, v) = w
        .call(
            &w.owner,
            Method::POST,
            &format!("/v1/orgs/{}/oauth-clients", w.org),
            Some(
                json!({"package_id": "selfhost", "client_key": "oa", "instance_id": id,
                        "fields": {"client_id": "c", "client_secret": "s"}}),
            ),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let (s, _, v) = w
        .connect(&w.owner, "desk", w.body("api_key", Some(id)))
        .await;
    assert_eq!(s, 200, "{v}");
    let cid: Uuid = v["connection"]["id"].as_str().unwrap().parse().unwrap();
    // a default-host connection is untouched
    let (s, _, v) = w.connect(&w.owner, "desk", w.body("api_key", None)).await;
    assert_eq!(s, 200);
    let keep: Uuid = v["connection"]["id"].as_str().unwrap().parse().unwrap();
    let (s, _) = w
        .call(
            &w.owner,
            Method::DELETE,
            &format!("/v1/app-instances/{id}"),
            None,
        )
        .await;
    assert_eq!(s, 204);
    let (st, reason, inst, client): (String, Option<String>, Option<Uuid>, Option<Uuid>) =
        sqlx::query_as("SELECT status, status_reason, instance_id, oauth_client_id FROM app_connections WHERE id = $1")
            .bind(cid)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(
        (st.as_str(), reason.as_deref(), inst, client),
        ("disabled", Some("instance_deleted"), None, None)
    );
    let (st,): (String,) = sqlx::query_as("SELECT status FROM app_connections WHERE id = $1")
        .bind(keep)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(st, "active");
    for t in ["app_instances", "app_oauth_clients"] {
        let n: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {t}"))
            .fetch_one(w.h.pool())
            .await
            .unwrap();
        assert_eq!(n, 0, "{t}");
    }
    // re-enabling would make it look like a default-host connection: refused
    let (s, v) = w
        .call(
            &w.owner,
            Method::PATCH,
            &format!("/v1/connections/{cid}"),
            Some(json!({"status": "active"})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("instance_deleted")),
        "{v}"
    );
    // the instance and its host are gone; the host can be registered again
    assert_eq!(
        w.call(
            &w.owner,
            Method::DELETE,
            &format!("/v1/app-instances/{id}"),
            None
        )
        .await
        .0,
        404
    );
    let (s, _) = w
        .register(&w.owner, w.org, "desk", "desk.acme.example")
        .await;
    assert_eq!(s, 201);
}

#[tokio::test]
async fn the_catalogue_says_which_apps_are_instance_bound() {
    let w = world().await;
    let (s, v) = w.call(&w.member, Method::GET, "/v1/apps/desk", None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["instances"],
        json!({"label": "Desk instance", "default_host": "saas.desk.example"})
    );
    let (_, v) = w
        .call(&w.member, Method::GET, "/v1/apps/nodefault", None)
        .await;
    assert_eq!(
        v["instances"],
        json!({"label": "Self-hosted only", "default_host": null})
    );
    let (_, v) = w.call(&w.member, Method::GET, "/v1/apps/plain", None).await;
    assert_eq!(v["instances"], Value::Null);
}
