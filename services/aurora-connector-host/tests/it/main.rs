#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Connector host against a stub API (docs/18 §Managed connectors, Host).

mod stub;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use aurora_connector_fake::FakeConnector;
use aurora_connector_host::{Built, CompiledType, Host, HostConfig, compiled};
use aurora_protocol::host::Assignment;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use serde_json::{Value, json};
use stub::Stub;
use tokio::sync::watch;
use tower::ServiceExt;

const TOKEN: &str = "auh_testtoken";

/// A compiled test type `id` whose callbacks answer `GET /ping` with its id.
fn ping_type(id: &str) -> CompiledType {
    let mut manifest = FakeConnector::catalogue_manifest();
    manifest.package.id = id.to_owned();
    let name = id.to_owned();
    CompiledType {
        type_id: id.to_owned(),
        manifest,
        signature: String::new(),
        build: Arc::new(move |_a, _api| {
            let c = FakeConnector::default().managed();
            let body = name.clone();
            Ok(Built {
                connector: Arc::new(c),
                callbacks: Some(Router::new().route("/ping", get(move || async move { body }))),
                test_control: None,
            })
        }),
    }
}

/// A type whose build always fails with a message that leaks the secret and a control character.
fn leaky_type(id: &str) -> CompiledType {
    let mut t = ping_type(id);
    t.build = Arc::new(|a, _api| {
        let secret = a.secrets.values().next().cloned().unwrap_or_default();
        Err(format!("bad key {secret}\u{7} {}", "x".repeat(400)))
    });
    t
}

fn assignment(id: &str, type_id: &str, settings: Value, secrets: &[(&str, &str)]) -> Assignment {
    Assignment {
        id: id.to_owned(),
        type_id: type_id.to_owned(),
        display_name: id.to_owned(),
        settings,
        secrets: secrets
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect::<BTreeMap<_, _>>(),
        status: "active".to_owned(),
    }
}

struct Running {
    host: Arc<Host>,
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

impl Running {
    async fn shutdown(self) {
        let _ = self.stop.send(true);
        let _ = tokio::time::timeout(Duration::from_secs(15), self.task).await;
    }
}

fn start(stub: &Stub, types: Vec<CompiledType>) -> Running {
    let mut cfg = HostConfig::new(&stub.url, TOKEN);
    cfg.assignments_wait_secs = 1;
    cfg.runtime_tweak = Some(Arc::new(|rt| {
        rt.config_interval = Some(Duration::from_millis(100));
        rt.ops_wait_secs = Some(0);
    }));
    let host = Arc::new(Host::new(cfg, types).expect("host"));
    let (stop, rx) = watch::channel(false);
    let task = tokio::spawn({
        let host = host.clone();
        async move { host.run(rx).await }
    });
    Running { host, stop, task }
}

async fn eventually<F: Fn() -> bool>(what: &str, f: F) {
    for _ in 0..300 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for: {what}");
}

async fn get_status(router: &Router, path: &str) -> (StatusCode, String) {
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .expect("body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn registers_its_catalogue_and_names_itself_when_polling() {
    let stub = Stub::start().await;
    let run = start(&stub, compiled(false));
    eventually("assignments poll", || stub.assignment_polls() > 0).await;
    let reg = stub.registrations();
    assert_eq!(reg[0]["instance_id"], run.host.instance_id().to_string());
    let types: Vec<_> = reg[0]["types"]
        .as_array()
        .expect("types")
        .iter()
        .map(|t| t["type_id"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(
        types,
        ["mattermost"],
        "fake is announced only with test_types"
    );
    // C-456: the host passes its own instance id on every assignments call.
    assert!(
        stub.assignment_queries()
            .iter()
            .all(|q| q.contains(&run.host.instance_id().to_string()))
    );
    let want = format!("Bearer {TOKEN}");
    assert!(stub.host_tokens().iter().all(|t| *t == want));
    run.shutdown().await;
}

#[tokio::test]
async fn starts_restarts_and_stops_instances_per_assignment() {
    let stub = Stub::start().await;
    let run = start(&stub, vec![ping_type("alpha")]);
    eventually("first poll", || stub.assignment_polls() > 0).await;
    assert!(stub.connector_registrations("alpha").is_empty());

    // start
    stub.set_assignments(vec![assignment(
        "alpha",
        "alpha",
        json!({"n": 1}),
        &[("k", "s3cret-one")],
    )]);
    eventually("alpha registers", || {
        stub.connector_registrations("alpha").len() == 1
    })
    .await;
    let first = &stub.connector_registrations("alpha")[0];
    // Delegated authentication (C-458): host token plus the connector header.
    assert_eq!(first.authorization, format!("Bearer {TOKEN}"));
    assert_eq!(first.connector, "alpha");
    eventually("running reported", || {
        stub.last_state("alpha").as_deref() == Some("running")
    })
    .await;

    // unchanged assignment, new revision for another connector: no restart
    stub.set_assignments(vec![
        assignment("alpha", "alpha", json!({"n": 1}), &[("k", "s3cret-one")]),
        assignment("beta", "alpha", json!({"n": 1}), &[("k", "s3cret-two")]),
    ]);
    eventually("beta registers", || {
        stub.connector_registrations("beta").len() == 1
    })
    .await;
    assert_eq!(
        stub.connector_registrations("alpha").len(),
        1,
        "alpha must not restart"
    );

    // restart on a settings change
    stub.set_assignments(vec![
        assignment("alpha", "alpha", json!({"n": 2}), &[("k", "s3cret-one")]),
        assignment("beta", "alpha", json!({"n": 1}), &[("k", "s3cret-two")]),
    ]);
    eventually("alpha restarts", || {
        stub.connector_registrations("alpha").len() == 2
    })
    .await;
    eventually("old alpha retired", || stub.retired("alpha") >= 1).await;
    assert_eq!(stub.connector_registrations("beta").len(), 1);

    // restart on a secret change
    stub.set_assignments(vec![
        assignment("alpha", "alpha", json!({"n": 2}), &[("k", "rotated")]),
        assignment("beta", "alpha", json!({"n": 1}), &[("k", "s3cret-two")]),
    ]);
    eventually("alpha restarts again", || {
        stub.connector_registrations("alpha").len() == 3
    })
    .await;

    // stop when the assignment disappears (disabled or deleted)
    stub.set_assignments(vec![assignment(
        "beta",
        "alpha",
        json!({"n": 1}),
        &[("k", "s3cret-two")],
    )]);
    let retired = stub.retired("alpha");
    eventually("alpha retired", || stub.retired("alpha") >= 3).await;
    assert!(retired <= stub.retired("alpha"));
    let (code, _) = get_status(&run.host.callback_router(), "/connectors/alpha/ping").await;
    assert_eq!(
        code,
        StatusCode::NOT_FOUND,
        "a stopped instance serves no callbacks"
    );
    let (code, body) = get_status(&run.host.callback_router(), "/connectors/beta/ping").await;
    assert_eq!((code, body.as_str()), (StatusCode::OK, "alpha"));

    // graceful shutdown stops every instance
    let before = stub.retired("beta");
    run.shutdown().await;
    assert!(
        stub.retired("beta") > before,
        "shutdown retires the remaining instance"
    );
}

#[tokio::test]
async fn callbacks_of_two_connectors_are_served_under_their_prefixes() {
    let stub = Stub::start().await;
    let run = start(&stub, vec![ping_type("alpha"), ping_type("beta")]);
    stub.set_assignments(vec![
        assignment("alpha", "alpha", json!({}), &[]),
        assignment("beta", "beta", json!({}), &[]),
    ]);
    let router = run.host.callback_router();
    eventually("both serve", || {
        stub.connector_registrations("alpha").len() == 1
            && stub.connector_registrations("beta").len() == 1
    })
    .await;
    assert_eq!(
        get_status(&router, "/connectors/alpha/ping").await,
        (StatusCode::OK, "alpha".into())
    );
    assert_eq!(
        get_status(&router, "/connectors/beta/ping").await,
        (StatusCode::OK, "beta".into())
    );
    assert_eq!(
        get_status(&router, "/connectors/gamma/ping").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get_status(&router, "/connectors/alpha/nope").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(get_status(&router, "/ping").await.0, StatusCode::NOT_FOUND);
    // no test control without a test router entry
    assert_eq!(
        get_status(&run.host.test_router(), "/test/alpha/fake/status")
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    run.shutdown().await;
}

#[tokio::test]
async fn mattermost_callbacks_nest_under_connectors_id() {
    let stub = Stub::start().await;
    let run = start(&stub, compiled(false));
    let key = "k".repeat(44);
    stub.set_assignments(vec![assignment(
        "mattermost",
        "mattermost",
        json!({"public_url": "https://a.example/connectors/mattermost"}),
        &[("callback_hmac_key", key.as_str())],
    )]);
    eventually("mattermost registers", || {
        !stub.connector_registrations("mattermost").is_empty()
    })
    .await;
    // The registered manifest is the catalogue's, with instance_setup (digest the admin approved).
    let m = &stub.connector_registrations("mattermost")[0].body["manifest"];
    assert_eq!(m["package"]["id"], "mattermost");
    assert_eq!(m["instance_setup"]["fields"][1]["type"], "generated_secret");
    let router = run.host.callback_router();
    let (code, _) = get_status(&router, "/connectors/mattermost/mm/ready").await;
    assert_ne!(code, StatusCode::NOT_FOUND);
    run.shutdown().await;
}

#[tokio::test]
async fn only_compiled_types_run() {
    assert_eq!(
        compiled(false)
            .iter()
            .map(|t| t.type_id.as_str())
            .collect::<Vec<_>>(),
        ["mattermost"]
    );
    assert_eq!(
        compiled(true)
            .iter()
            .map(|t| t.type_id.as_str())
            .collect::<Vec<_>>(),
        ["mattermost", "fake"]
    );
    let stub = Stub::start().await;
    let run = start(&stub, compiled(false));
    // `fake` is compiled in but not enabled; `evil` exists nowhere. Neither may start, whatever
    // the assignment says (settings that look like an image or a command change nothing).
    stub.set_assignments(vec![
        assignment(
            "fake",
            "fake",
            json!({}),
            &[("api_key", "a"), ("signing_key", "b")],
        ),
        assignment(
            "evil",
            "evil",
            json!({"image": "docker.io/evil/x", "command": ["/bin/sh"], "binary": "/bin/sh"}),
            &[],
        ),
    ]);
    eventually("both reported as errors", || {
        stub.last_state("fake").as_deref() == Some("error")
            && stub.last_state("evil").as_deref() == Some("error")
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(stub.connector_registrations("fake").is_empty());
    assert!(stub.connector_registrations("evil").is_empty());
    assert!(
        stub.last_message("evil")
            .unwrap_or_default()
            .contains("not available")
    );
    run.shutdown().await;
}

#[tokio::test]
async fn status_messages_are_sanitized_and_never_carry_a_secret() {
    let stub = Stub::start().await;
    let run = start(&stub, vec![leaky_type("leaky")]);
    stub.set_assignments(vec![assignment(
        "leaky",
        "leaky",
        json!({}),
        &[("k", "hunter2-super-secret")],
    )]);
    eventually("error reported", || {
        stub.last_state("leaky").as_deref() == Some("error")
    })
    .await;
    let msg = stub.last_message("leaky").unwrap_or_default();
    assert!(
        !msg.contains("hunter2-super-secret"),
        "secret leaked: {msg}"
    );
    assert!(msg.contains("[redacted]"));
    assert!(
        !msg.chars().any(char::is_control),
        "control characters stripped"
    );
    assert!(msg.chars().count() <= 300);
    // no status request, in any form, contains the secret
    assert!(
        stub.status_bodies()
            .iter()
            .all(|b| !b.to_string().contains("hunter2-super-secret"))
    );
    run.shutdown().await;
}

#[tokio::test]
async fn invalid_mattermost_settings_report_an_error_naming_the_setting_not_the_value() {
    let stub = Stub::start().await;
    let run = start(&stub, compiled(false));
    stub.set_assignments(vec![assignment(
        "mattermost",
        "mattermost",
        json!({"catchup_window_secs": 0}),
        &[(
            "callback_hmac_key",
            "k-this-is-a-long-enough-secret-key-value-1234",
        )],
    )]);
    eventually("error reported", || {
        stub.last_state("mattermost").as_deref() == Some("error")
    })
    .await;
    let msg = stub.last_message("mattermost").unwrap_or_default();
    assert!(msg.contains("catchup_window_secs"));
    assert!(!msg.contains("long-enough-secret"));
    assert!(stub.connector_registrations("mattermost").is_empty());
    run.shutdown().await;
}

#[test]
fn the_fake_test_type_is_signed_with_the_ephemeral_test_key_and_never_by_the_project_key() {
    use aurora_connector_host::{catalogue, signing};
    use aurora_registry::signature::{Signature, TrustStore, verify_digest};
    let t = compiled(true).remove(1);
    assert_eq!(t.type_id, "fake");
    assert_eq!(t.manifest.package.publisher, catalogue::TEST_PUBLISHER);
    let info = catalogue::test_key_info();
    assert_eq!(info["publisher"], catalogue::TEST_PUBLISHER);
    let sig = Signature::parse(t.signature.as_bytes()).expect("signed");
    assert_eq!(info["key_id"], sig.key_id);
    // The project key (publisher `aurora`) is not involved: a trust store with only a project key
    // refuses it.
    let d = signing::digest(&t.manifest).expect("digest");
    let other = ed25519_dalek::SigningKey::from_bytes(&[9u8; 32]);
    let trust = TrustStore {
        project_key: Some(other.verifying_key()),
        keys: vec![],
    };
    assert!(
        verify_digest(
            t.signature.as_bytes(),
            catalogue::TEST_PUBLISHER,
            &d,
            &trust
        )
        .is_err()
    );
}

#[tokio::test]
async fn fake_type_runs_with_test_types_and_serves_its_test_control() {
    let stub = Stub::start().await;
    let run = start(&stub, compiled(true));
    stub.set_assignments(vec![assignment(
        "fake",
        "fake",
        json!({"nickname": "fake"}),
        &[("api_key", "ak"), ("signing_key", "sk")],
    )]);
    eventually("fake registers", || {
        !stub.connector_registrations("fake").is_empty()
    })
    .await;
    let (code, _) = get_status(&run.host.test_router(), "/test/fake/fake/status").await;
    assert_eq!(code, StatusCode::OK);
    run.shutdown().await;
}

#[test]
fn signed_manifests_verify_against_the_project_key_and_cover_production_types_only() {
    use aurora_connector_host::signing;
    use aurora_registry::signature::{Signer, TrustStore, verify_digest};
    let key = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
    let dir = std::env::temp_dir().join(format!("p4-mcon-2-sig-{}", uuid::Uuid::new_v4()));
    let done = signing::sign_all(&key, &dir).expect("sign");
    assert_eq!(done, ["mattermost"], "the fake test type is never signed");
    assert!(!dir.join("fake.sig").exists());
    let sig = std::fs::read_to_string(dir.join("mattermost.sig")).expect("sig file");
    let t = compiled(false).remove(0);
    let trust = TrustStore {
        project_key: Some(key.verifying_key()),
        keys: vec![],
    };
    let d = signing::digest(&t.manifest).expect("digest");
    assert_eq!(
        verify_digest(sig.as_bytes(), &t.manifest.package.publisher, &d, &trust).expect("verifies"),
        Signer::Project
    );
    // a changed manifest no longer verifies
    let mut other = t.manifest.clone();
    other.package.version = "9.9.9".into();
    let d2 = signing::digest(&other).expect("digest");
    assert!(verify_digest(sig.as_bytes(), "aurora", &d2, &trust).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn host_config_debug_redacts_the_token() {
    let cfg = HostConfig::new("http://api", "auh_supersecret");
    assert!(!format!("{cfg:?}").contains("supersecret"));
    let a = assignment("x", "x", json!({}), &[("k", "topsecret")]);
    assert!(!format!("{a:?}").contains("topsecret"));
}
