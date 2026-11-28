#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-285: the lifecycle and default-store notices leave the request path, and a default-store
//! change the deployment makes at startup is told too.

use crate::common::*;
use aurora_api::domains::auth::mail::{Mail, MailError, Mailer};
use aurora_api::domains::lifecycle_notices;
use aurora_api::domains::settings::handle;
use http::Method;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Semaphore, mpsc};
use uuid::Uuid;

/// How long a thing that must happen may take. A failure bound, never a wait the test relies on.
const BOUND: Duration = Duration::from_secs(20);

/// A mail sender that is as slow as the test wants: every send waits for [`GatedMailer::open`],
/// then reports its recipient.
struct GatedMailer {
    gate: Semaphore,
    done: mpsc::UnboundedSender<String>,
}

impl GatedMailer {
    fn new() -> (Arc<Self>, mpsc::UnboundedReceiver<String>) {
        let (done, rx) = mpsc::unbounded_channel();
        let m = Arc::new(Self {
            gate: Semaphore::new(0),
            done,
        });
        (m, rx)
    }

    fn open(&self) {
        self.gate.add_permits(1_000);
    }
}

#[async_trait::async_trait]
impl Mailer for GatedMailer {
    async fn send(&self, m: Mail) -> Result<(), MailError> {
        let _open = self.gate.acquire().await.expect("the gate stays");
        let _ = self.done.send(m.to);
        Ok(())
    }
}

fn gate(h: &mut Harness) -> (Arc<GatedMailer>, mpsc::UnboundedReceiver<String>) {
    let (m, rx) = GatedMailer::new();
    h.state = h.state.clone().with_mailer(m.clone() as Arc<dyn Mailer>);
    h.app = test_router(h.state.clone());
    (m, rx)
}

async fn delivered(rx: &mut mpsc::UnboundedReceiver<String>) -> String {
    tokio::time::timeout(BOUND, rx.recv())
        .await
        .expect("the notice is delivered")
        .expect("the mailer is alive")
}

/// The request, which must answer while every mail send is still blocked.
async fn answers(h: &Harness, tok: &str, m: Method, uri: &str, body: Value) -> (u16, Value) {
    tokio::time::timeout(BOUND, h.call_tok(tok, m, uri, Some(body)))
        .await
        .unwrap_or_else(|_| panic!("{uri} waited for the mail sender"))
}

/// An organization with its creator (the actor) and one more owner, `peer@x.io`.
async fn org_with_peer(h: &Harness) -> (String, Uuid, Uuid, String) {
    let actor = h.user("owner@x.io", true).await;
    let (_, tok) = h.good_session(actor).await;
    let (s, v) = h
        .call_tok(
            &tok,
            Method::POST,
            "/v1/orgs",
            Some(json!({"name": "Acme"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let org: Uuid = v["org"]["id"].as_str().unwrap().parse().unwrap();
    let ws: Uuid = v["workspace"]["id"].as_str().unwrap().parse().unwrap();
    let slug = v["org"]["slug"].as_str().unwrap().to_owned();
    let peer = h.user("peer@x.io", false).await;
    h.member(org, peer, "owner", "active").await;
    (tok, org, ws, slug)
}

#[tokio::test]
async fn a_freeze_and_an_unfreeze_answer_before_a_slow_mail_sender_and_still_send() {
    let mut h = Harness::new().await;
    let (tok, org, _, _) = org_with_peer(&h).await;
    let (mailer, mut sent) = gate(&mut h);
    let (s, v) = answers(
        &h,
        &tok,
        Method::POST,
        &format!("/v1/orgs/{org}/freeze"),
        json!({"reason": "incident"}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = answers(
        &h,
        &tok,
        Method::POST,
        &format!("/v1/orgs/{org}/unfreeze"),
        json!({}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert!(sent.try_recv().is_err(), "the sender is still blocked");
    mailer.open();
    assert_eq!(delivered(&mut sent).await, "peer@x.io");
    assert_eq!(delivered(&mut sent).await, "peer@x.io");
}

#[tokio::test]
async fn the_deletes_answer_before_a_slow_mail_sender_and_still_send() {
    let mut h = Harness::new().await;
    let (tok, org, ws, slug) = org_with_peer(&h).await;
    let (mailer, mut sent) = gate(&mut h);
    let (s, v) = answers(
        &h,
        &tok,
        Method::DELETE,
        &format!("/v1/workspaces/{ws}"),
        json!({"confirm": "default"}),
    )
    .await;
    assert_eq!(s, 204, "{v}");
    let (s, v) = answers(
        &h,
        &tok,
        Method::DELETE,
        &format!("/v1/orgs/{org}"),
        json!({"confirm": slug}),
    )
    .await;
    assert_eq!(s, 204, "{v}");
    assert!(sent.try_recv().is_err(), "the sender is still blocked");
    mailer.open();
    assert_eq!(delivered(&mut sent).await, "peer@x.io");
    assert_eq!(delivered(&mut sent).await, "peer@x.io");
}

#[tokio::test]
async fn a_default_store_change_answers_before_a_slow_mail_sender_and_still_sends() {
    let mut h = Harness::new().await.with_helm("");
    let (tok, _, _, _) = org_with_peer(&h).await;
    // A second instance administrator changes the setting, so both owners are told.
    let (s, v) = h
        .call_tok(&tok, Method::GET, "/v1/admin/secret-stores", None)
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = h
        .call_tok(
            &tok,
            Method::PUT,
            "/v1/admin/secret-stores/mem",
            Some(json!({"generation": v["generation"], "store": {"kind": "memory"}})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (mailer, mut sent) = gate(&mut h);
    let (s, v) = answers(
        &h,
        &tok,
        Method::PUT,
        "/v1/admin/secret-store-default",
        json!({"generation": v["generation"], "store_id": "mem"}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    mailer.open();
    // The administrators' own settings mail goes through the same sender.
    while delivered(&mut sent).await != "peer@x.io" {}
}

const VAULT_DEFAULT: &str = "[secrets]\ndefault_store = \"helmvault\"\n\
     [[secrets.stores]]\nid = \"helmvault\"\nkind = \"memory\"\n";
const INLINE_DEFAULT: &str = "[secrets]\ndefault_store = \"inline\"\n\
     [[secrets.stores]]\nid = \"helmvault\"\nkind = \"memory\"\n";

/// One start of the API with `helm`: the mirroring, then what `main` does for its notice.
/// Returns whether there was a change to tell.
async fn start(h: &Harness) -> bool {
    handle::start(&h.state.settings, h.pool()).await.unwrap();
    lifecycle_notices::send_mirrored_default_store(&h.state)
}

fn peer_mail(h: &Harness) -> Vec<Mail> {
    h.mail
        .sent()
        .into_iter()
        .filter(|m| m.to == "peer@x.io")
        .collect()
}

/// The notices a start dispatched have been sent.
async fn settle(h: &Harness) -> Vec<Mail> {
    tokio::time::timeout(BOUND, h.state.notices.idle())
        .await
        .expect("the notices are sent");
    peer_mail(h)
}

#[tokio::test]
async fn a_default_store_change_by_the_deployment_is_told_once_per_change() {
    let h = Harness::new().await;
    org_with_peer(&h).await;

    // The deployment names the built-in store, which is what is in force already: no change.
    let h = h.with_helm(INLINE_DEFAULT);
    assert!(!start(&h).await, "nothing changed");

    // The deployment moves the default.
    let h = h.with_helm(VAULT_DEFAULT);
    assert!(start(&h).await, "a change to tell");
    let mail = settle(&h).await;
    assert_eq!(mail.len(), 1, "{mail:?}");
    assert!(
        mail[0]
            .body
            .contains("the deployment configuration changed the instance's default secret store")
            && mail[0]
                .body
                .contains("go to helmvault instead of the built-in store"),
        "{mail:?}"
    );

    // A restart with the same configuration tells nothing.
    let h = h.with_helm(VAULT_DEFAULT);
    assert!(!start(&h).await, "a restart with no change");
    assert_eq!(settle(&h).await.len(), 1);

    // And back again is a change again, this time sent by the watcher `main` spawns.
    let h = h.with_helm(INLINE_DEFAULT);
    lifecycle_notices::spawn_mirrored_default_store(h.state.clone());
    handle::start(&h.state.settings, h.pool()).await.unwrap();
    tokio::time::timeout(BOUND, async {
        while peer_mail(&h).len() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the watcher sends the notice");
    let mail = peer_mail(&h);
    assert!(
        mail[1]
            .body
            .contains("go to the built-in store instead of helmvault"),
        "{mail:?}"
    );
}
