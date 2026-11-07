#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! BE-064: attachments (upload, bind, delivery, download, runner plane, outbound caps, sweep).

use crate::common::*;
use aurora_core::limits::{ATTACH_IN_FILE_MAX, ATTACH_OUT_COUNT, ATTACH_OUT_FILE_MAX};
use axum::body::Body;
use base64::Engine;
use http::{HeaderMap, Method, Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

struct Rig {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    admin_cookie: String,
    tok: String,
    cookie: String,
    user: Uuid,
    rep: Uuid,
}

struct Live {
    conv: Uuid,
    sb: String,
    boot: String,
    turn: String,
    att: Uuid,
}

const NOTES: &[u8] = b"hello world";

fn caps() -> Value {
    json!({
        "backend": "fake", "home_durability": "resident",
        "resources": {"cpu": {"min": 250, "max": 4000}, "memory_mib": {"min": 256, "max": 8192},
                      "pids": {"min": 64, "max": 4096}, "home_gib": {"min": 1, "max": 100}},
        "max_sandboxes": 4, "harnesses": ["fake"], "max_attachment_bytes": 1048576
    })
}

fn multipart(part: &str, filename: &str, ctype: &str, data: &[u8]) -> (String, Vec<u8>) {
    let b = "XBOUNDARYX";
    let mut body = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"{part}\"; filename=\"{filename}\"\r\nContent-Type: {ctype}\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(data);
    body.extend_from_slice(format!("\r\n--{b}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={b}"), body)
}

async fn raw(
    h: &Harness,
    m: Method,
    uri: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> (u16, HeaderMap, Vec<u8>) {
    let mut b = Request::builder().method(m).uri(uri);
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let res = h
        .app
        .clone()
        .oneshot(b.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let (p, body) = res.into_parts();
    (
        p.status.as_u16(),
        p.headers,
        body.collect().await.unwrap().to_bytes().to_vec(),
    )
}

fn json_of(b: &[u8]) -> Value {
    serde_json::from_slice(b).unwrap_or(Value::Null)
}

async fn register(h: &Harness, admin_cookie: &str, name: &str) -> (String, Uuid) {
    let (s, _, v) = h
        .send(
            Method::POST,
            "/v1/admin/runners",
            &[("cookie", admin_cookie), ("origin", ORIGIN)],
            Some(json!({"name": name})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let tok = v["token"].as_str().unwrap().to_owned();
    let b = bearer(&tok);
    let (s, _, v) = h
        .send(
            Method::POST,
            "/v1/runner/register",
            &[("authorization", &b)],
            Some(
                json!({"version": "0.1", "backend": "fake", "capabilities": caps(),
                "advertised_host": name}),
            ),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    (tok, v["replica_id"].as_str().unwrap().parse().unwrap())
}

async fn rig() -> Rig {
    let dir = std::env::temp_dir().join(format!("aurora-ca-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("ca.pem"), "-----BEGIN CERTIFICATE-----\nX\n").unwrap();
    let d = dir.to_string_lossy().into_owned();
    let h = Harness::with_config(move |c| {
        c.gateway_ca_dir = d;
        c.llm.endpoints.push(aurora_config::LlmEndpoint {
            id: "anthropic".into(),
            provider: "anthropic".into(),
            default_model: Some("claude-test".into()),
            ..Default::default()
        });
    })
    .await;
    let admin = h.user("admin@x.io", true).await;
    let (_, sess) = h.good_session(admin).await;
    let admin_cookie = cookie(&sess);
    let (tok, rep) = register(&h, &admin_cookie, "runner-a").await;
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let user = h.user("m@x.io", false).await;
    h.member(org, user, "member", "active").await;
    h.bind_ws(ws, user, "member").await;
    let (_, st) = h.good_session(user).await;
    Rig {
        h,
        org,
        ws,
        admin_cookie,
        tok,
        cookie: cookie(&st),
        user,
        rep,
    }
}

impl Rig {
    async fn runner(&self, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        let b = bearer(&self.tok);
        let rid = self.rep.to_string();
        let hd = [("authorization", b.as_str()), ("x-aurora-replica", &rid)];
        let (s, _, v) = self.h.send(m, uri, &hd, body).await;
        (s.as_u16(), v)
    }

    async fn work(&self) -> Vec<Value> {
        let (s, v) = self
            .runner(
                Method::POST,
                "/v1/runner/work",
                Some(json!({"wait_secs": 0, "capacity": {"running": 0, "max": 4}})),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v["items"].as_array().unwrap().clone()
    }

    async fn report(&self, lv: &Live, mut body: Value) {
        body["report_id"] = json!(Uuid::now_v7());
        body["sandbox_id"] = json!(lv.sb);
        body["boot_id"] = json!(lv.boot);
        let (s, v) = self
            .runner(
                Method::POST,
                "/v1/runner/reports",
                Some(json!({"reports": [body]})),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["results"][0]["status"], "applied", "{v}");
    }

    async fn user_call(&self, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
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

    /// Multipart upload as `cookie`.
    async fn upload_as(
        &self,
        cookie: &str,
        conv: Uuid,
        filename: &str,
        ctype: &str,
        data: &[u8],
    ) -> (u16, HeaderMap, Value) {
        let (ct, body) = multipart("file", filename, ctype, data);
        let (s, hd, b) = raw(
            &self.h,
            Method::POST,
            &format!("/v1/conversations/{conv}/attachments"),
            &[
                ("cookie", cookie),
                ("origin", ORIGIN),
                ("content-type", &ct),
            ],
            body,
        )
        .await;
        (s, hd, json_of(&b))
    }

    async fn upload(&self, conv: Uuid, filename: &str, data: &[u8]) -> (u16, Value) {
        let (s, _, v) = self
            .upload_as(&self.cookie, conv, filename, "text/plain", data)
            .await;
        (s, v)
    }

    async fn agent(&self, handle: &str) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness) VALUES ($1,$2,$3,$4,$4,'hosted','fake')")
            .bind(id).bind(self.org).bind(self.ws).bind(handle).execute(self.h.pool()).await.unwrap();
        let sec = Uuid::now_v7();
        sqlx::query("INSERT INTO secrets (id, org_id, name, kind, llm_provider, llm_auth_mode, llm_endpoint_id, host_pattern, injection, value_source, value_ref) VALUES ($1,$2,$3,'llm','anthropic','api_key','anthropic','api.anthropic.com','{}','managed','x')")
            .bind(sec).bind(self.org).bind(format!("k-{handle}")).execute(self.h.pool()).await.unwrap();
        sqlx::query(
            "INSERT INTO grants (id, workspace_id, agent_id, secret_id) VALUES ($1,$2,$3,$4)",
        )
        .bind(Uuid::now_v7())
        .bind(self.ws)
        .bind(id)
        .bind(sec)
        .execute(self.h.pool())
        .await
        .unwrap();
        id
    }

    async fn direct(&self, agent: Uuid) -> Uuid {
        let (s, v) = self
            .user_call(
                Method::POST,
                &format!("/v1/agents/{agent}/conversations/direct"),
                None,
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v["id"].as_str().unwrap().parse().unwrap()
    }

    /// Upload `notes.txt`, send it with the first message, start the sandbox up to
    /// `home.synced`. The turn is still `queued`; [`Rig::deliver`] dispatches it.
    async fn boot(&self, handle: &str) -> Live {
        let agent = self.agent(handle).await;
        let conv = self.direct(agent).await;
        let (s, v) = self.upload(conv, "notes.txt", NOTES).await;
        assert_eq!(s, 201, "{v}");
        let att: Uuid = v["id"].as_str().unwrap().parse().unwrap();
        let (s, v) = self
            .user_call(
                Method::POST,
                &format!("/v1/conversations/{conv}/messages"),
                Some(json!({"text": "hello", "attachment_ids": [att]})),
            )
            .await;
        assert_eq!(s, 202, "{v}");
        let turn = v["turn"]["id"].as_str().unwrap().to_owned();
        let items = self.work().await;
        let start = items.iter().find(|i| i["kind"] == "sandbox.start").unwrap();
        let (sb, boot) = (
            start["sandbox_id"].as_str().unwrap().to_owned(),
            start["boot_id"].as_str().unwrap().to_owned(),
        );
        let raw_doc = start["spec"]["boot_secret"].as_str().unwrap();
        let doc: Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(raw_doc)
                .unwrap(),
        )
        .unwrap();
        let lv = Live {
            conv,
            sb,
            boot,
            turn,
            att,
        };
        self.report(
            &lv,
            json!({"kind": "sandbox.status", "state": "running", "container_ref": "c"}),
        )
        .await;
        let (s, v) = self
            .runner(
                Method::POST,
                "/v1/runner/channel/claim",
                Some(json!({"bootstrap_token": doc["bootstrap_token"]})),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        let (s, v) = self
            .runner(
                Method::POST,
                "/v1/runner/channel/hold",
                Some(json!({"sandbox_id": lv.sb, "boot_id": lv.boot})),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        self.report(
            &lv,
            json!({"kind": "supervisor.ready", "version": "0.1",
                "harness": {"id": "fake", "capabilities": ["steer", "resume"]}}),
        )
        .await;
        let items = self.work().await;
        let last = items.iter().rfind(|i| i["kind"] == "home.sync").unwrap();
        let generation = last["generation"].as_u64().unwrap();
        self.report(
            &lv,
            json!({"kind": "home.synced", "generation": generation}),
        )
        .await;
        lv
    }

    async fn deliver(&self) -> Value {
        self.work()
            .await
            .into_iter()
            .find(|i| i["kind"] == "turn.deliver")
            .expect("turn.deliver")
    }

    /// Runner-plane request with the given token/replica.
    async fn runner_raw(
        &self,
        tok: &str,
        rep: Uuid,
        m: Method,
        uri: &str,
        extra: &[(&str, String)],
        body: Vec<u8>,
    ) -> (u16, HeaderMap, Vec<u8>) {
        let b = bearer(tok);
        let r = rep.to_string();
        let mut hd: Vec<(&str, &str)> = vec![("authorization", &b), ("x-aurora-replica", &r)];
        for (k, v) in extra {
            hd.push((k, v));
        }
        raw(&self.h, m, uri, &hd, body).await
    }

    fn out_headers(&self, lv: &Live, name: &str) -> Vec<(&'static str, String)> {
        vec![
            ("x-aurora-sandbox", lv.sb.clone()),
            ("x-aurora-boot", lv.boot.clone()),
            ("x-aurora-conversation", lv.conv.to_string()),
            ("x-aurora-turn", lv.turn.clone()),
            ("x-aurora-filename", name.to_owned()),
            ("content-type", "application/octet-stream".to_owned()),
        ]
    }

    async fn send_out(&self, lv: &Live, name: &str, data: Vec<u8>) -> (u16, Value) {
        let hd = self.out_headers(lv, name);
        let (s, _, b) = self
            .runner_raw(
                &self.tok,
                self.rep,
                Method::POST,
                "/v1/runner/attachments",
                &hd,
                data,
            )
            .await;
        (s, json_of(&b))
    }
}

#[tokio::test]
async fn upload_bind_deliver_and_download() {
    let r = rig().await;
    let lv = r.boot("daneel").await;

    // The upload was bound by the message.
    let (st, tid): (String, Option<Uuid>) =
        sqlx::query_as("SELECT status, turn_id FROM attachments WHERE id = $1")
            .bind(lv.att)
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert_eq!(st, "bound");
    assert_eq!(tid, Some(lv.turn.parse().unwrap()));

    // Not readable by the runner until the turn is delivered.
    let (s, _, _) = r
        .runner_raw(
            &r.tok,
            r.rep,
            Method::GET,
            &format!("/v1/runner/attachments/{}", lv.att),
            &[],
            vec![],
        )
        .await;
    assert_eq!(s, 404);

    let d = r.deliver().await;
    assert_eq!(d["turn_id"], lv.turn);
    assert_eq!(
        d["attachments"],
        json!([{"attachment_id": lv.att, "filename": "notes.txt", "size": NOTES.len()}])
    );

    // Runner fetch of delivered bytes.
    let (s, hd, body) = r
        .runner_raw(
            &r.tok,
            r.rep,
            Method::GET,
            &format!("/v1/runner/attachments/{}", lv.att),
            &[],
            vec![],
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(body, NOTES);
    assert_eq!(hd["x-content-type-options"], "nosniff");

    // Another runner (not owning the sandbox) gets 404.
    let (tok2, rep2) = register(&r.h, &r.admin_cookie, "runner-b").await;
    let (s, _, _) = r
        .runner_raw(
            &tok2,
            rep2,
            Method::GET,
            &format!("/v1/runner/attachments/{}", lv.att),
            &[],
            vec![],
        )
        .await;
    assert_eq!(s, 404);
    // Unknown id.
    let (s, _, _) = r
        .runner_raw(
            &r.tok,
            r.rep,
            Method::GET,
            &format!("/v1/runner/attachments/{}", Uuid::now_v7()),
            &[],
            vec![],
        )
        .await;
    assert_eq!(s, 404);
    // The user door of the same bytes.
    let (s, hd, body) = raw(
        &r.h,
        Method::GET,
        &format!("/v1/attachments/{}/content", lv.att),
        &[("cookie", &r.cookie)],
        vec![],
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(body, NOTES);
    assert_eq!(hd["content-type"], "application/octet-stream");
}

#[tokio::test]
async fn download_headers_never_echo_the_stored_type() {
    let r = rig().await;
    let agent = r.agent("zed").await;
    let conv = r.direct(agent).await;
    let (s, _, v) = r
        .upload_as(
            &r.cookie,
            conv,
            "../evil <b>.html",
            "Text/HTML; charset=utf-8",
            b"<script>alert(1)</script>",
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["filename"], "evil__b_.html");
    assert_eq!(v["content_type"], "text/html");
    assert_eq!(v["status"], "pending");
    assert_eq!(v["direction"], "in");
    assert_eq!(v["size_bytes"], 25);
    assert_eq!(v["turn_id"], Value::Null);
    assert_eq!(v["caption"], Value::Null);
    assert_eq!(v["conversation_id"], conv.to_string());
    let id = v["id"].as_str().unwrap();

    let (s, hd, body) = raw(
        &r.h,
        Method::GET,
        &format!("/v1/attachments/{id}/content"),
        &[("cookie", &r.cookie)],
        vec![],
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(body, b"<script>alert(1)</script>");
    assert_eq!(hd["content-type"], "application/octet-stream");
    assert_eq!(
        hd["content-disposition"],
        "attachment; filename=\"evil__b_.html\""
    );
    assert_eq!(hd["x-content-type-options"], "nosniff");

    // No file part -> 400; wrong part name -> 400.
    let (ct, body) = multipart("other", "a.txt", "text/plain", b"x");
    let (s, _, _) = raw(
        &r.h,
        Method::POST,
        &format!("/v1/conversations/{conv}/attachments"),
        &[
            ("cookie", &r.cookie),
            ("origin", ORIGIN),
            ("content-type", &ct),
        ],
        body,
    )
    .await;
    assert_eq!(s, 400);
}

#[tokio::test]
async fn oversize_upload_is_413() {
    let r = rig().await;
    let agent = r.agent("big").await;
    let conv = r.direct(agent).await;
    // Just over the cap: caught by the streaming counter.
    let (s, v) = r
        .upload(conv, "big.bin", &vec![0u8; ATTACH_IN_FILE_MAX as usize + 1])
        .await;
    assert_eq!(s, 413, "{v}");
    assert_eq!(v["error"]["code"], "payload_too_large");
    // Far over the cap: caught by the route's body limit.
    let (s, v) = r
        .upload(
            conv,
            "big.bin",
            &vec![0u8; ATTACH_IN_FILE_MAX as usize + 200_000],
        )
        .await;
    assert_eq!(s, 413, "{v}");
    // Exactly at the cap is accepted.
    let (s, v) = r
        .upload(conv, "ok.bin", &vec![1u8; ATTACH_IN_FILE_MAX as usize])
        .await;
    assert_eq!(s, 201, "{v}");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM attachments WHERE conversation_id = $1")
        .bind(conv)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn more_than_the_count_on_one_message_is_422() {
    let r = rig().await;
    let agent = r.agent("many").await;
    let conv = r.direct(agent).await;
    let mut ids = Vec::new();
    for i in 0..11 {
        let (s, v) = r.upload(conv, &format!("f{i}.txt"), b"x").await;
        assert_eq!(s, 201, "{v}");
        ids.push(v["id"].clone());
    }
    let (s, v) = r
        .user_call(
            Method::POST,
            &format!("/v1/conversations/{conv}/messages"),
            Some(json!({"text": "hi", "attachment_ids": ids})),
        )
        .await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["code"], "too_many_attachments");
    // Nothing was bound and no turn was created.
    let bound: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM attachments WHERE conversation_id = $1 AND status = 'bound'",
    )
    .bind(conv)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(bound, 0);
    // Ten are fine.
    let (s, v) = r
        .user_call(
            Method::POST,
            &format!("/v1/conversations/{conv}/messages"),
            Some(json!({"text": "hi", "attachment_ids": &ids[..10]})),
        )
        .await;
    assert_eq!(s, 202, "{v}");
}

#[tokio::test]
async fn another_users_direct_conversation_is_404() {
    let r = rig().await;
    let agent = r.agent("priv").await;
    let conv = r.direct(agent).await;
    let (s, v) = r.upload(conv, "mine.txt", b"secret").await;
    assert_eq!(s, 201, "{v}");
    let id = v["id"].as_str().unwrap().to_owned();

    let other = r.h.user("o@x.io", false).await;
    r.h.member(r.org, other, "member", "active").await;
    r.h.bind_ws(r.ws, other, "member").await;
    let (_, tok) = r.h.good_session(other).await;
    let oc = cookie(&tok);
    let (s, _, _) = r.upload_as(&oc, conv, "x.txt", "text/plain", b"x").await;
    assert_eq!(s, 404);
    let (s, _, _) = raw(
        &r.h,
        Method::GET,
        &format!("/v1/attachments/{id}/content"),
        &[("cookie", &oc)],
        vec![],
    )
    .await;
    assert_eq!(s, 404);
    // Unknown conversation and attachment ids are 404 as well; the owner still reads it.
    let (s, _, _) = r
        .upload_as(&r.cookie, Uuid::now_v7(), "x.txt", "text/plain", b"x")
        .await;
    assert_eq!(s, 404);
    let (s, _, b) = raw(
        &r.h,
        Method::GET,
        &format!("/v1/attachments/{id}/content"),
        &[("cookie", &r.cookie)],
        vec![],
    )
    .await;
    assert_eq!((s, b.as_slice()), (200, &b"secret"[..]));
    let _ = r.user;
}

#[tokio::test]
async fn outbound_upload_creates_event_and_enforces_caps() {
    let r = rig().await;
    let lv = r.boot("sender").await;
    let _ = r.deliver().await;
    r.report(&lv, json!({"kind": "turn.started", "turn_id": lv.turn}))
        .await;

    // Filename arrives percent-encoded; sanitized on store.
    let mut hd = r.out_headers(&lv, "re%20port%20%C3%A9.pdf");
    hd.push(("x-aurora-caption", "the%20report".to_owned()));
    let (s, _, b) = r
        .runner_raw(
            &r.tok,
            r.rep,
            Method::POST,
            "/v1/runner/attachments",
            &hd,
            b"%PDF".to_vec(),
        )
        .await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));
    let id: Uuid = json_of(&b)["attachment_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let (dir, status, turn, name, size, cap, created_by): (
        String,
        String,
        Option<Uuid>,
        String,
        i64,
        Option<String>,
        Option<Uuid>,
    ) = sqlx::query_as(
        "SELECT direction, status, turn_id, filename, size_bytes, caption, created_by_user_id FROM attachments WHERE id = $1",
    )
    .bind(id)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(
        (dir.as_str(), status.as_str(), turn, name.as_str(), size),
        (
            "out",
            "bound",
            Some(lv.turn.parse().unwrap()),
            "re_port__.pdf",
            4
        )
    );
    assert_eq!((cap.as_deref(), created_by), (Some("the report"), None));
    let (ty, payload, ev_turn): (String, Value, Option<Uuid>) = sqlx::query_as(
        "SELECT type, payload, turn_id FROM transcript_events WHERE conversation_id = $1 AND type = 'attachment'",
    )
    .bind(lv.conv)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(ty, "attachment");
    assert_eq!(ev_turn, Some(lv.turn.parse().unwrap()));
    assert_eq!(
        payload,
        json!({"turn_id": lv.turn, "attachment_id": id, "filename": "re_port__.pdf",
               "size": 4, "caption": "the report"})
    );
    // The user can read it back.
    let (s, _, body) = raw(
        &r.h,
        Method::GET,
        &format!("/v1/attachments/{id}/content"),
        &[("cookie", &r.cookie)],
        vec![],
    )
    .await;
    assert_eq!((s, body.as_slice()), (200, &b"%PDF"[..]));

    // Per-file cap.
    let (s, v) = r
        .send_out(&lv, "huge.bin", vec![0u8; ATTACH_OUT_FILE_MAX as usize + 1])
        .await;
    assert_eq!(s, 413, "{v}");
    // Fenced boot, foreign sandbox owner, wrong conversation, missing headers.
    let mut bad = r.out_headers(&lv, "a.txt");
    bad[1].1 = Uuid::now_v7().to_string();
    let (s, _, _) = r
        .runner_raw(
            &r.tok,
            r.rep,
            Method::POST,
            "/v1/runner/attachments",
            &bad,
            b"x".to_vec(),
        )
        .await;
    assert_eq!(s, 409);
    let (tok2, rep2) = register(&r.h, &r.admin_cookie, "runner-b").await;
    let ok = r.out_headers(&lv, "a.txt");
    let (s, _, _) = r
        .runner_raw(
            &tok2,
            rep2,
            Method::POST,
            "/v1/runner/attachments",
            &ok,
            b"x".to_vec(),
        )
        .await;
    assert_eq!(s, 404);
    let other_agent = r.agent("other").await;
    let other_conv = r.direct(other_agent).await;
    let mut bad = r.out_headers(&lv, "a.txt");
    bad[2].1 = other_conv.to_string();
    let (s, _, _) = r
        .runner_raw(
            &r.tok,
            r.rep,
            Method::POST,
            "/v1/runner/attachments",
            &bad,
            b"x".to_vec(),
        )
        .await;
    assert_eq!(s, 404);
    let (s, _, _) = r
        .runner_raw(
            &r.tok,
            r.rep,
            Method::POST,
            "/v1/runner/attachments",
            &[],
            b"x".to_vec(),
        )
        .await;
    assert_eq!(s, 400);

    // Per-turn count: one is stored, so ATTACH_OUT_COUNT - 1 more fit.
    for i in 1..ATTACH_OUT_COUNT {
        let (s, v) = r.send_out(&lv, &format!("f{i}.txt"), b"x".to_vec()).await;
        assert_eq!(s, 200, "{i}: {v}");
    }
    let (s, v) = r.send_out(&lv, "one-too-many.txt", b"x".to_vec()).await;
    assert_eq!(s, 429, "{v}");
    assert_eq!(v["error"]["code"], "attachment_count_exceeded");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM attachments WHERE conversation_id = $1 AND direction = 'out'",
    )
    .bind(lv.conv)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(n, i64::from(ATTACH_OUT_COUNT));
}

#[tokio::test]
async fn outbound_daily_quota_per_conversation() {
    let r = rig().await;
    let lv = r.boot("quota").await;
    let _ = r.deliver().await;
    // 199 MiB of earlier output (not tied to this turn) leaves room for 1 MiB only.
    let ws = r.ws;
    sqlx::query(
        "INSERT INTO attachments (id, workspace_id, conversation_id, direction, status, filename,
            content_type, size_bytes, sha256, blob_backend, blob_key)
         VALUES ($1,$2,$3,'out','bound','old.bin','application/octet-stream',$4,'x','postgres','k')",
    )
    .bind(Uuid::now_v7())
    .bind(ws)
    .bind(lv.conv)
    .bind(199i64 * 1024 * 1024)
    .execute(r.h.pool())
    .await
    .unwrap();
    let (s, v) = r.send_out(&lv, "two.bin", vec![0u8; 2 * 1024 * 1024]).await;
    assert_eq!(s, 429, "{v}");
    assert_eq!(v["error"]["code"], "attachment_quota_exceeded");
    let blobs: i64 = sqlx::query_scalar("SELECT count(*) FROM blobs")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    // Only the inbound notes.txt blob remains; the rejected upload left nothing behind.
    assert_eq!(blobs, 1);
    let (s, v) = r.send_out(&lv, "one.bin", vec![0u8; 1024 * 1024]).await;
    assert_eq!(s, 200, "{v}");
    // Rows older than 24 h do not count.
    sqlx::query("UPDATE attachments SET created_at = now() - interval '25 hours' WHERE filename = 'old.bin'")
        .execute(r.h.pool())
        .await
        .unwrap();
    let (s, v) = r
        .send_out(&lv, "three.bin", vec![0u8; 5 * 1024 * 1024])
        .await;
    assert_eq!(s, 200, "{v}");
}

#[tokio::test]
async fn sweep_removes_expired_pending_rows_and_blobs() {
    let r = rig().await;
    let agent = r.agent("sweeper").await;
    let conv = r.direct(agent).await;
    let mut ids = Vec::new();
    for n in ["old.txt", "fresh.txt", "old-bound.txt"] {
        let (s, v) = r.upload(conv, n, n.as_bytes()).await;
        assert_eq!(s, 201, "{v}");
        ids.push(v["id"].as_str().unwrap().parse::<Uuid>().unwrap());
    }
    // old-bound is bound to a turn, then everything but fresh ages past the TTL.
    let (s, v) = r
        .user_call(
            Method::POST,
            &format!("/v1/conversations/{conv}/messages"),
            Some(json!({"text": "hi", "attachment_ids": [ids[2]]})),
        )
        .await;
    assert_eq!(s, 202, "{v}");
    sqlx::query(
        "UPDATE attachments SET created_at = now() - interval '25 hours' WHERE id = ANY($1)",
    )
    .bind(vec![ids[0], ids[2]])
    .execute(r.h.pool())
    .await
    .unwrap();
    let keys = |ids: Vec<Uuid>| {
        let pool = r.h.pool().clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT a.blob_key FROM attachments a JOIN blobs b ON b.key = a.blob_key WHERE a.id = ANY($1)",
            )
            .bind(ids)
            .fetch_all(&pool)
            .await
            .unwrap()
            .len()
        }
    };
    assert_eq!(keys(ids.clone()).await, 3);
    let old_key: String = sqlx::query_scalar("SELECT blob_key FROM attachments WHERE id = $1")
        .bind(ids[0])
        .fetch_one(r.h.pool())
        .await
        .unwrap();

    assert_eq!(
        aurora_api::jobs::sweep_attachments(r.h.pool())
            .await
            .unwrap(),
        1
    );
    let left: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM attachments WHERE id = ANY($1) ORDER BY id")
            .bind(ids.clone())
            .fetch_all(r.h.pool())
            .await
            .unwrap();
    let mut want = vec![ids[1], ids[2]];
    want.sort();
    assert_eq!(left, want);
    let gone: i64 = sqlx::query_scalar("SELECT count(*) FROM blobs WHERE key = $1")
        .bind(&old_key)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(gone, 0);
    assert_eq!(keys(vec![ids[1], ids[2]]).await, 2);
    // Idempotent.
    assert_eq!(
        aurora_api::jobs::sweep_attachments(r.h.pool())
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn sweep_removes_blobs_orphaned_by_cascades_and_keeps_referenced_ones() {
    let r = rig().await;
    let a1 = r.agent("keeper").await;
    let a2 = r.agent("doomed").await;
    let keep_conv = r.direct(a1).await;
    let gone_conv = r.direct(a2).await;
    let (s, v) = r.upload(keep_conv, "keep.txt", b"keep").await;
    assert_eq!(s, 201, "{v}");
    let keep: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let (s, v) = r.upload(gone_conv, "gone.txt", b"gone").await;
    assert_eq!(s, 201, "{v}");
    let gone: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let key_of = |id: Uuid| {
        let pool = r.h.pool().clone();
        async move {
            sqlx::query_scalar::<_, String>("SELECT blob_key FROM attachments WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    let (keep_key, gone_key) = (key_of(keep).await, key_of(gone).await);
    // A blob with no row at all (e.g. an earlier cascade), fresh and old copies.
    for k in ["orphan-old", "orphan-fresh"] {
        sqlx::query("INSERT INTO blobs (key, content) VALUES ($1, 'x')")
            .bind(k)
            .execute(r.h.pool())
            .await
            .unwrap();
    }

    // The conversation goes away; the cascade removes the attachment row but not the blob.
    sqlx::query("DELETE FROM conversations WHERE id = $1")
        .bind(gone_conv)
        .execute(r.h.pool())
        .await
        .unwrap();
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM attachments WHERE id = $1")
        .bind(gone)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(rows, 0);
    let has = |k: String| {
        let pool = r.h.pool().clone();
        async move {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM blobs WHERE key = $1")
                .bind(k)
                .fetch_one(&pool)
                .await
                .unwrap()
                == 1
        }
    };
    assert!(has(gone_key.clone()).await, "cascade leaves the blob");

    // Younger than the age guard: nothing is removed, orphan or not.
    assert_eq!(
        aurora_api::jobs::sweep_attachments(r.h.pool())
            .await
            .unwrap(),
        0
    );
    assert!(has(gone_key.clone()).await);

    // Backdate everything past the guard: orphans go, the referenced blob stays.
    sqlx::query(
        "UPDATE blobs SET created_at = now() - interval '2 hours' WHERE key <> 'orphan-fresh'",
    )
    .execute(r.h.pool())
    .await
    .unwrap();
    assert_eq!(
        aurora_api::jobs::sweep_attachments(r.h.pool())
            .await
            .unwrap(),
        2
    );
    assert!(!has(gone_key).await);
    assert!(!has("orphan-old".into()).await);
    assert!(
        has("orphan-fresh".into()).await,
        "still inside the age guard"
    );
    assert!(has(keep_key).await, "referenced blob survives");
    // Idempotent.
    assert_eq!(
        aurora_api::jobs::sweep_attachments(r.h.pool())
            .await
            .unwrap(),
        0
    );
}
