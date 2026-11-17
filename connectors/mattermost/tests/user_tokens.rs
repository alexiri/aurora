//! P4-UTOK (docs/20 §Operations, §Rendering; SEC-112, SEC-113): a person's token reaches Mattermost
//! only through `GET /api/v4/users/me` (verify) and `POST /api/v4/posts` (send); the post ends with
//! the footer, has no ephemeral part, file or card; a `401` answers `credential_dead` and touches
//! nothing else. The platform is a mock that records every request.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use aurora_connector_mattermost::api::NetPolicy;
use aurora_connector_mattermost::integ::{Integ, Settings};
use aurora_connector_mattermost::ops;
use aurora_connector_sdk::{CoreClient, IntegrationHandle, RetryPolicy, spawn_event_poster};
use aurora_protocol::connector::*;
use serde_json::{Value, json};
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const TEAM: &str = "team0000000000000000000001";
const CH: &str = "chan0000000000000000000001";
const ROOT: &str = "root0000000000000000000001";
const BOT: &str = "bot00000000000000000000001";
const PERSON: &str = "pers0000000000000000000001";
const POST: &str = "post0000000000000000000001";
const BOT_TOKEN: &str = "bot-token-not-a-person";
/// The canary: whatever else happens it must be on `users/me` and `posts` requests only.
const PERSON_TOKEN: &str = "person-pat-canary-7d1f0c2a9b8e4f3a";

struct AcceptAll;
impl Respond for AcceptAll {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let ev: EventsRequest = serde_json::from_slice(&req.body).unwrap();
        ResponseTemplate::new(200).set_body_json(EventsResponse {
            accepted: ev.events.iter().map(|e| e.event_id.clone()).collect(),
            duplicates: vec![],
            rejected: vec![],
        })
    }
}

struct Rig {
    integ: Arc<Integ>,
    mm: MockServer,
    _core: MockServer,
}

async fn rig() -> Rig {
    let core = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/connector/events"))
        .respond_with(AcceptAll)
        .mount(&core)
        .await;
    let mm = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v4/channels/{CH}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": CH, "team_id": TEAM, "type": "O", "name": "general", "display_name": "General"
        })))
        .mount(&mm)
        .await;
    let client =
        Arc::new(CoreClient::new(&core.uri(), "aun_test", RetryPolicy::default()).unwrap());
    let (tx, rx) = tokio::sync::watch::channel(false);
    let sink = spawn_event_poster(client, rx);
    std::mem::forget(tx);
    let settings = Settings {
        public_url: Some("https://connector.test".into()),
        hmac_key: b"k".to_vec(),
        catchup_window: Duration::from_secs(60),
        api_url: None,
        token_check: Duration::from_secs(3600),
        capabilities: None,
        test_control: false,
        net: NetPolicy::allow_all(),
    };
    let id = Uuid::from_u128(1);
    let integration = Integration {
        id,
        org_id: Uuid::nil(),
        tenant_ref: format!("mm.test/{TEAM}"),
        tenant_label: "T".into(),
        config: json!({"server_url": mm.uri()}),
        credentials: json!({"bot_token": BOT_TOKEN}),
        credentials_version: 1,
        shared_bot: Some(SharedBot {
            platform_user_ref: BOT.into(),
            handle: "aurora".into(),
            display_name: "Aurora".into(),
        }),
        status: "active".into(),
    };
    let declared: BTreeSet<String> = [
        "threads",
        "user_tokens",
        "ephemeral",
        "files",
        "edits",
        "reactions",
        "native_status",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    let integ = Integ::from_config(
        &integration,
        &[],
        &declared,
        sink,
        IntegrationHandle::new(id),
        settings,
    )
    .unwrap();
    Rig {
        integ: Arc::new(integ),
        mm,
        _core: core,
    }
}

impl Rig {
    async fn mock_me(&self, status: u16) {
        Mock::given(method("GET"))
            .and(path("/api/v4/users/me"))
            .respond_with(ResponseTemplate::new(status).set_body_json(
                json!({"id": PERSON, "username": "alice", "email": "alice@example.com"}),
            ))
            .mount(&self.mm)
            .await;
    }

    async fn mock_post(&self, status: u16) {
        Mock::given(method("POST"))
            .and(path("/api/v4/posts"))
            .respond_with(ResponseTemplate::new(status).set_body_json(json!({
                "id": POST, "channel_id": CH, "root_id": ROOT, "message": "x",
                "user_id": PERSON, "create_at": 1_700_000_000_000_i64
            })))
            .mount(&self.mm)
            .await;
    }

    /// Every request that carried the person's token, as `METHOD /path`.
    async fn person_requests(&self) -> Vec<(String, Value)> {
        self.mm
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|r| {
                r.headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| v.contains(PERSON_TOKEN))
            })
            .map(|r| {
                (
                    format!("{} {}", r.method, r.url.path()),
                    serde_json::from_slice(&r.body).unwrap_or(Value::Null),
                )
            })
            .collect()
    }

    /// No request of any kind carried the person's token anywhere but its headers.
    async fn assert_token_only_in_authorization(&self) {
        for r in self.mm.received_requests().await.unwrap() {
            let body = String::from_utf8_lossy(&r.body).to_string();
            assert!(!body.contains(PERSON_TOKEN), "the token is in a body");
            assert!(
                !r.url.as_str().contains(PERSON_TOKEN),
                "the token is in a URL"
            );
        }
    }
}

fn op(kind: OpKind, payload: Value, token: Option<&str>) -> Operation {
    Operation {
        op_id: Uuid::now_v7(),
        kind,
        integration_id: Some(Uuid::from_u128(1)),
        presence_id: None,
        attempt: 1,
        maybe_sent: false,
        payload,
        credentials: None,
        user_credential: token.map(str::to_owned),
    }
}

fn post_payload(extra: Value) -> Value {
    let mut p = json!({
        "target": {"space_ref": CH, "thread_ref": ROOT},
        "sender": {"user_token": Uuid::from_u128(9)},
        "content": {"text": "Hello team, the report is ready."},
        "op_marker": "marker-1",
        "user_footer": {"agent_name": "Report *Bot*", "owner_label": "Alice <@all>"},
    });
    p.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    p
}

fn code(e: &OpError) -> &str {
    e.code.as_str()
}

#[tokio::test]
async fn verify_calls_users_me_with_the_persons_token_only() {
    let r = rig().await;
    r.mock_me(200).await;
    let o = op(
        OpKind::UserTokenVerify,
        json!({"config": {}}),
        Some(PERSON_TOKEN),
    );
    let out = ops::execute(&r.integ, None, &o).await.unwrap();
    assert_eq!(out, json!({"platform_user_ref": PERSON, "handle": "alice"}));
    let seen = r.person_requests().await;
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].0, "GET /api/v4/users/me");
    // Nothing else: no email, no other profile field reaches the core.
    assert!(!out.to_string().contains("alice@example.com"));
    r.assert_token_only_in_authorization().await;
}

#[tokio::test]
async fn a_401_is_credential_dead_with_no_detail_and_the_presence_is_not_touched() {
    let r = rig().await;
    r.mock_me(401).await;
    r.mock_post(401).await;
    let verify = op(
        OpKind::UserTokenVerify,
        json!({"config": {}}),
        Some(PERSON_TOKEN),
    );
    let e = ops::execute(&r.integ, None, &verify).await.unwrap_err();
    assert_eq!(code(&e), "credential_dead");
    assert!(!e.message.contains(PERSON_TOKEN));
    let post = op(
        OpKind::MessagePost,
        post_payload(json!({})),
        Some(PERSON_TOKEN),
    );
    let e = ops::execute(&r.integ, None, &post).await.unwrap_err();
    assert_eq!(code(&e), "credential_dead");
    assert!(!e.message.contains(PERSON_TOKEN));
    // The bot's own token was never tried and nothing else was called with either token.
    let all = r.mm.received_requests().await.unwrap();
    assert!(all.iter().all(|q| {
        let a = q
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        a.contains(PERSON_TOKEN)
            == (q.url.path() == "/api/v4/users/me" || q.url.path() == "/api/v4/posts")
            || !a.contains(PERSON_TOKEN)
    }));
    r.assert_token_only_in_authorization().await;
}

#[tokio::test]
async fn a_post_goes_out_as_the_person_with_the_footer_as_its_last_line() {
    let r = rig().await;
    r.mock_post(201).await;
    let o = op(
        OpKind::MessagePost,
        post_payload(json!({})),
        Some(PERSON_TOKEN),
    );
    let out = ops::execute(&r.integ, None, &o).await.unwrap();
    assert_eq!(out["message_ref"], POST);
    let seen = r.person_requests().await;
    assert_eq!(seen.len(), 1, "only POST /posts: {seen:?}");
    assert_eq!(seen[0].0, "POST /api/v4/posts");
    let body = &seen[0].1;
    assert_eq!(body["channel_id"], CH);
    assert_eq!(body["root_id"], ROOT);
    let message = body["message"].as_str().unwrap();
    assert!(
        message.starts_with("Hello team, the report is ready."),
        "{message}"
    );
    let last = message.lines().last().unwrap();
    assert!(
        last.starts_with("_Sent by ") && last.ends_with('_'),
        "{last:?}"
    );
    assert!(
        last.contains("Report \\*Bot\\*"),
        "the agent name is escaped: {last:?}"
    );
    assert!(last.contains("Alice"), "{last:?}");
    // The owner label cannot ping: the broadcast token is broken.
    assert!(
        !last.contains("@all") && last.contains("@\u{200b}all"),
        "{last:?}"
    );
    // No ephemeral, no file, no attachments, no card on a person's post.
    assert!(body.get("file_ids").is_none(), "{body}");
    assert_eq!(body["props"]["attachments"], json!([]));
    assert_eq!(body["props"]["aurora_op"], "marker-1");
    assert!(body.get("user_id").is_none());
    r.assert_token_only_in_authorization().await;
    // The bot's token never went to /posts for this op.
    let posted_with_bot =
        r.mm.received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|q| q.url.path() == "/api/v4/posts")
            .count();
    assert_eq!(posted_with_bot, 1);
}

#[tokio::test]
async fn ephemeral_files_cards_direct_targets_and_a_missing_token_are_refused_before_any_call() {
    let r = rig().await;
    r.mock_post(201).await;
    let cases = [
        post_payload(json!({"ephemeral_to": PERSON})),
        post_payload(
            json!({"files": [{"attachment_id": Uuid::nil(), "name": "a", "download_url": "u"}]}),
        ),
        post_payload(json!({"target": {"user_ref": PERSON}})),
        post_payload(json!({"content": {"text": "hi", "card": {"title": "t", "body": []}}})),
        post_payload(json!({"user_footer": null})),
    ];
    for p in cases {
        let o = op(OpKind::MessagePost, p.clone(), Some(PERSON_TOKEN));
        let e = ops::execute(&r.integ, None, &o).await.unwrap_err();
        assert_eq!(code(&e), "invalid", "{p}: {e:?}");
    }
    let o = op(OpKind::MessagePost, post_payload(json!({})), None);
    let e = ops::execute(&r.integ, None, &o).await.unwrap_err();
    assert_eq!(code(&e), "invalid");
    assert!(
        r.person_requests().await.is_empty(),
        "no request carried the token"
    );
    let posts =
        r.mm.received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|q| q.url.path() == "/api/v4/posts")
            .count();
    assert_eq!(posts, 0, "nothing reached /posts");
}

#[tokio::test]
async fn a_person_token_cannot_edit_delete_react_or_type() {
    let r = rig().await;
    let sender = json!({"user_token": Uuid::from_u128(9)});
    let cases = [
        (
            OpKind::MessageUpdate,
            json!({"space_ref": CH, "message_ref": POST, "sender": sender,
                   "content": {"text": "x"}}),
        ),
        (
            OpKind::MessageDelete,
            json!({"space_ref": CH, "message_ref": POST, "sender": sender}),
        ),
        (
            OpKind::ReactionAdd,
            json!({"space_ref": CH, "message_ref": POST, "sender": sender,
                   "emoji": "thumbsup", "avatar_url": null}),
        ),
        (
            OpKind::StatusSet,
            json!({"space_ref": CH, "sender": sender}),
        ),
    ];
    for (k, p) in cases {
        let o = op(k, p, Some(PERSON_TOKEN));
        let e = ops::execute(&r.integ, None, &o).await.unwrap_err();
        assert_eq!(code(&e), "invalid", "{e:?}");
    }
    assert!(
        r.mm.received_requests()
            .await
            .unwrap()
            .iter()
            .all(|q| q.url.path() != "/api/v4/posts")
    );
    assert!(r.person_requests().await.is_empty());
}

#[tokio::test]
async fn a_reclaimed_post_looks_for_its_marker_with_the_bot_and_never_repeats() {
    let r = rig().await;
    r.mock_post(201).await;
    // The first attempt already posted: the bot's search finds the marker, so nothing is posted.
    Mock::given(method("GET"))
        .and(path(format!("/api/v4/channels/{CH}/posts")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "order": [POST],
            "posts": {POST: {"id": POST, "channel_id": CH, "root_id": ROOT, "user_id": PERSON,
                              "message": "x", "create_at": 1, "props": {"aurora_op": "marker-1"}}}
        })))
        .mount(&r.mm)
        .await;
    let mut o = op(
        OpKind::MessagePost,
        post_payload(json!({})),
        Some(PERSON_TOKEN),
    );
    o.maybe_sent = true;
    let out = ops::execute(&r.integ, None, &o).await.unwrap();
    assert_eq!(out["message_ref"], POST);
    assert!(
        r.person_requests().await.is_empty(),
        "no second post as the person"
    );
}

#[test]
fn the_operation_debug_redacts_the_token() {
    let o = op(OpKind::UserTokenVerify, json!({}), Some(PERSON_TOKEN));
    let s = format!("{o:?}");
    assert!(!s.contains(PERSON_TOKEN), "{s}");
    assert!(s.contains("[redacted]"));
}
