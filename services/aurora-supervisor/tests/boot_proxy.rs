#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::net::SocketAddr;
mod common;

use aurora_supervisor::boot::{self, BootDocument};
use aurora_supervisor::proxy::{Credential, ProxyMode, spawn_proxy};
use base64::Engine as _;
use common::{decode_basic, stub_gateway};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const LO: &str = "127.0.0.1:0";

async fn raw(proxy: SocketAddr, req: &str) -> String {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out).await;
    String::from_utf8_lossy(&out).to_string()
}

#[tokio::test]
async fn credential_switches_boot_context_boot() {
    let (gw, seen) = stub_gateway().await;
    let cred = Credential::new("x", "aub_boot");
    let p = spawn_proxy(LO.parse().unwrap(), gw, cred.clone(), ProxyMode::Mitm)
        .await
        .unwrap();
    let req = "GET http://example.test/a HTTP/1.1\r\nHost: example.test\r\nProxy-Authorization: Basic Zm9yZ2Vk\r\n\r\n";

    assert!(raw(p.addr, req).await.ends_with("ok"));
    cred.set_context(Some("auc_turn".into()));
    assert!(raw(p.addr, req).await.ends_with("ok"));
    cred.set_context(None);
    assert!(raw(p.addr, req).await.ends_with("ok"));

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 3);
    // client-supplied header stripped: exactly one, ours
    assert!(seen.iter().all(|h| h.len() == 1));
    let creds: Vec<String> = seen.iter().map(|h| decode_basic(&h[0])).collect();
    assert_eq!(creds, ["x:aub_boot", "x:auc_turn", "x:aub_boot"]);
}

#[tokio::test]
async fn open_proxy_is_connect_only() {
    let (gw, seen) = stub_gateway().await;
    let p = spawn_proxy(
        LO.parse().unwrap(),
        gw,
        Credential::new("open", "aub_boot"),
        ProxyMode::ConnectOnly,
    )
    .await
    .unwrap();
    let r = raw(
        p.addr,
        "GET http://example.test/ HTTP/1.1\r\nHost: example.test\r\n\r\n",
    )
    .await;
    assert!(r.starts_with("HTTP/1.1 405"), "{r}");
    assert!(seen.lock().unwrap().is_empty());
    let r = raw(
        p.addr,
        "CONNECT example.test:443 HTTP/1.1\r\nHost: example.test:443\r\n\r\n",
    )
    .await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    assert_eq!(decode_basic(&seen.lock().unwrap()[0][0]), "open:aub_boot");
}

fn boot_json(dir: &std::path::Path) -> String {
    let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
    let doc = json!({
        "v": 1,
        "sandbox_id": "00000000-0000-0000-0000-000000000001",
        "boot_id": "00000000-0000-0000-0000-000000000002",
        "agent": {"id": "00000000-0000-0000-0000-000000000003", "handle": "daneel", "name": "Daneel"},
        "runner_url": "ws://aurora-runner:4105/v1/supervisor",
        "bootstrap_token": "aup_x",
        "gateway": {"proxy": "aurora-gateway:4102", "boot_token": "aub_x"},
        "ca_pem": "PEM",
        "files": [
            {"path": dir.join("a/new.conf"), "mode": "0600", "content_b64": b64("fresh"), "only_if_absent_or_sentinel": true},
            {"path": dir.join("a/user.conf"), "content_b64": b64("overwritten"), "only_if_absent_or_sentinel": true},
            {"path": dir.join("a/managed.conf"), "content_b64": b64("# aurora-managed\nv2"), "only_if_absent_or_sentinel": true}
        ],
        "harness": {"id": "fake", "capabilities_expected": ["resume"]},
        "timezone": "Europe/Zurich"
    });
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(doc.to_string())
}

#[test]
fn boot_document_files_env_and_dumpable() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = std::env::temp_dir().join(format!("aurora-sup-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(dir.join("a")).unwrap();
    std::fs::write(dir.join("a/user.conf"), "mine").unwrap();
    std::fs::write(dir.join("a/managed.conf"), "# aurora-managed\nv1").unwrap();

    let doc = BootDocument::parse(&boot_json(&dir)).unwrap();
    assert_eq!(doc.agent.handle, "daneel");
    let skipped = boot::write_boot_files(&doc, &dir.join("ca.pem")).unwrap();
    assert_eq!(skipped, [dir.join("a/user.conf")]);
    assert_eq!(
        std::fs::read_to_string(dir.join("a/user.conf")).unwrap(),
        "mine"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("a/new.conf")).unwrap(),
        "fresh"
    );
    assert_eq!(
        std::fs::metadata(dir.join("a/new.conf"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(
        std::fs::read_to_string(dir.join("a/managed.conf"))
            .unwrap()
            .ends_with("v2")
    );
    assert_eq!(std::fs::read_to_string(dir.join("ca.pem")).unwrap(), "PEM");

    assert!(BootDocument::parse("not base64!").is_err());

    // children never receive the boot document
    let mut cmd = std::process::Command::new("env");
    cmd.env(boot::BOOT_ENV, "secret");
    boot::scrub_command(&mut cmd);
    let out = String::from_utf8(cmd.output().unwrap().stdout).unwrap();
    assert!(!out.contains("AURORA_BOOT") && !out.contains("secret"));

    boot::make_non_dumpable().unwrap();
    assert!(!boot::is_dumpable().unwrap());
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    // `Dumpable:` is only exposed by newer kernels; when present it must be 0.
    if let Some(l) = status.lines().find(|l| l.starts_with("Dumpable:")) {
        assert!(l.trim_end().ends_with('0'));
    }
    std::fs::remove_dir_all(dir).ok();
}
