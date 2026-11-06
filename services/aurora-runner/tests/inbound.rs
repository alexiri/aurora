//! S-11: the runner downloads inbound attachments and streams them as `file.in` frames, in order,
//! before the `turn.deliver` they belong to.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use aurora_protocol::runner::{WorkItem, WorkItemBody};
use aurora_runner::api::{ApiClient, RetryPolicy};
use aurora_runner::backends::fake::FakeBackend;
use aurora_runner::executor::{Executor, ExecutorSettings};
use aurora_runner::hub::StubHub;
use aurora_runner::reporter;
use base64::Engine as _;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::watch;
use uuid::Uuid;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "aur_testtoken";
const CHUNK: usize = 144 * 1024;

fn fast_retry() -> RetryPolicy {
    RetryPolicy {
        attempts: 2,
        initial: Duration::from_millis(5),
        max: Duration::from_millis(20),
    }
}

fn deliver_item(sandbox: Uuid, boot: Uuid, turn: Uuid, attachments: Value) -> WorkItem {
    let body = json!({
        "kind": "turn.deliver",
        "conversation_id": Uuid::new_v4(), "turn_id": turn, "session": {"epoch": 1},
        "model": {"provider": "p", "endpoint_id": "e", "base_url": "http://x",
                  "dialect": "anthropic_messages", "model": "m", "effort": "low", "client_env": []},
        "context_credential": "auc_secret",
        "input": {"text": "hi", "speaker_label": "Ann", "door": "web",
                  "recall": {"index": "", "snippets": []}, "notices": []},
        "attachments": attachments,
    });
    WorkItem {
        item_id: Uuid::new_v4(),
        sandbox_id: sandbox,
        boot_id: boot,
        body: serde_json::from_value::<WorkItemBody>(body).unwrap(),
    }
}

struct Rig {
    exec: Executor,
    hub: Arc<StubHub>,
    sandbox: Uuid,
    _server: MockServer,
}

async fn rig(server: MockServer, max_bytes: u64) -> Rig {
    let api = Arc::new(ApiClient::new(&server.uri(), TOKEN, fast_retry()).unwrap());
    let (_tx, rx) = watch::channel(false);
    let (sink, _task) = reporter::spawn(api.clone(), rx);
    let hub = Arc::new(StubHub::default());
    let exec = Executor::new(
        Arc::new(FakeBackend::new(4)),
        hub.clone(),
        sink,
        ExecutorSettings::default(),
    );
    exec.set_inbound_api(api, max_bytes);
    let sandbox = Uuid::new_v4();
    hub.set_connected(sandbox, true);
    Rig {
        exec,
        hub,
        sandbox,
        _server: server,
    }
}

async fn serve(server: &MockServer, id: Uuid, bytes: Vec<u8>) {
    Mock::given(method("GET"))
        .and(path(format!("/v1/runner/attachments/{id}")))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes))
        .mount(server)
        .await;
}

#[tokio::test]
async fn attachments_stream_as_file_in_frames_before_turn_deliver() {
    let server = MockServer::start().await;
    let (small, big, empty) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let small_bytes = b"hello inbox".to_vec();
    let big_bytes: Vec<u8> = (0..CHUNK * 2 + 17).map(|i| (i % 251) as u8).collect();
    serve(&server, small, small_bytes.clone()).await;
    serve(&server, big, big_bytes.clone()).await;
    serve(&server, empty, Vec::new()).await;
    let rig = rig(server, 25 * 1024 * 1024).await;
    let (turn, boot) = (Uuid::new_v4(), Uuid::new_v4());
    rig.exec.submit(deliver_item(
        rig.sandbox,
        boot,
        turn,
        json!([
            {"attachment_id": small, "filename": "../small.txt", "size": small_bytes.len()},
            {"attachment_id": big, "filename": "big.bin", "size": big_bytes.len()},
            {"attachment_id": empty, "filename": "empty", "size": 0},
        ]),
    ));
    assert!(rig.exec.drain(Duration::from_secs(10)).await);

    let sent = rig.hub.sent();
    let kinds: Vec<&str> = sent.iter().map(|(_, f)| f.kind.as_str()).collect();
    // 1 + 3 + 1 file.in frames, then the turn.
    assert_eq!(
        kinds,
        [
            "file.in",
            "file.in",
            "file.in",
            "file.in",
            "file.in",
            "turn.deliver"
        ]
    );
    let frames: Vec<&Value> = sent[..5].iter().map(|(_, f)| &f.body).collect();

    // Small file: one frame, untrusted filename passed through verbatim.
    assert_eq!(frames[0]["attachment_id"], json!(small));
    assert_eq!(frames[0]["turn_id"], json!(turn));
    assert_eq!(frames[0]["filename"], "../small.txt");
    assert_eq!(
        (frames[0]["index"].as_u64(), frames[0]["last"].as_bool()),
        (Some(0), Some(true))
    );
    assert_eq!(frames[0]["size"], small_bytes.len());
    assert_eq!(
        frames[0]["sha256"],
        hex::encode(Sha256::digest(&small_bytes))
    );

    // Big file: three chunks in index order; reassembly matches.
    let mut joined = Vec::new();
    for (i, f) in frames[1..4].iter().enumerate() {
        assert_eq!(f["attachment_id"], json!(big));
        assert_eq!(f["index"], i);
        assert_eq!(f["last"], i == 2);
        assert_eq!(f["size"], big_bytes.len());
        joined.extend(
            base64::engine::general_purpose::STANDARD
                .decode(f["data_b64"].as_str().unwrap())
                .unwrap(),
        );
    }
    assert_eq!(joined, big_bytes);
    assert_eq!(frames[1]["sha256"], hex::encode(Sha256::digest(&big_bytes)));

    // Empty file: a single empty last chunk.
    assert_eq!(frames[4]["size"], 0);
    assert_eq!(frames[4]["data_b64"], "");
    assert_eq!(frames[4]["last"], true);

    // Every frame validates against the protocol.
    for (_, f) in &sent[..5] {
        let fi: aurora_protocol::supervisor::FileIn =
            serde_json::from_value(f.body.clone()).unwrap();
        aurora_protocol::common::Validate::validate(&fi).unwrap();
    }
    assert_eq!(sent[5].1.body["turn_id"], json!(turn));
}

#[tokio::test]
async fn download_failure_still_forwards_turn_deliver() {
    let server = MockServer::start().await;
    let (gone, ok, huge) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    Mock::given(method("GET"))
        .and(path(format!("/v1/runner/attachments/{gone}")))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    serve(&server, ok, b"fine".to_vec()).await;
    serve(&server, huge, vec![7u8; 2048]).await;
    // Cap of 1 KiB: the 2 KiB body is refused.
    let rig = rig(server, 1024).await;
    let (turn, boot) = (Uuid::new_v4(), Uuid::new_v4());
    rig.exec.submit(deliver_item(
        rig.sandbox,
        boot,
        turn,
        json!([
            {"attachment_id": gone, "filename": "gone.txt", "size": 4},
            {"attachment_id": huge, "filename": "huge.bin", "size": 2048},
            {"attachment_id": ok, "filename": "ok.txt", "size": 4},
        ]),
    ));
    assert!(rig.exec.drain(Duration::from_secs(10)).await);
    let sent = rig.hub.sent();
    let kinds: Vec<&str> = sent.iter().map(|(_, f)| f.kind.as_str()).collect();
    assert_eq!(kinds, ["file.in", "turn.deliver"]);
    assert_eq!(sent[0].1.body["attachment_id"], json!(ok));
}

#[tokio::test]
async fn turn_without_attachments_is_forwarded_unchanged() {
    let server = MockServer::start().await;
    let rig = rig(server, 1024).await;
    rig.exec.submit(deliver_item(
        rig.sandbox,
        Uuid::new_v4(),
        Uuid::new_v4(),
        json!([]),
    ));
    assert!(rig.exec.drain(Duration::from_secs(5)).await);
    let kinds: Vec<String> = rig.hub.sent().into_iter().map(|(_, f)| f.kind).collect();
    assert_eq!(kinds, ["turn.deliver"]);
}
