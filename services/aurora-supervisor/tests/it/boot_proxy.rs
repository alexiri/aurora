#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::net::SocketAddr;

use crate::common::{decode_basic, stub_gateway, stub_gateway_with, tls_fixture};
use aurora_supervisor::boot::{self, BootDocument};
use aurora_supervisor::proxy::GatewayUpstream;
use aurora_supervisor::proxy::{Credential, ProxyMode, spawn_proxy};
use base64::Engine as _;
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
        "gateway": {"proxy": "aurora-gateway:4102", "tls": true, "server_name": "aurora-gateway", "boot_token": "aub_x"},
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
    let skipped = boot::write_boot_files_in(&doc, &dir.join("ca.pem"), &dir).unwrap();
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
    let mut cmd = aurora_supervisor::spawn::command("env", [(boot::BOOT_ENV, "secret")]);
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

#[test]
fn boot_files_are_confined_to_the_home() {
    use std::os::unix::fs::PermissionsExt as _;
    let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
    let root = std::env::temp_dir().join(format!("aurora-sup-{}", uuid::Uuid::now_v7()));
    let home = root.join("home");
    let outside = root.join("outside");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, home.join("link")).unwrap();
    let doc = |path: &std::path::Path| {
        let d = json!({
            "v": 1,
            "sandbox_id": "00000000-0000-0000-0000-000000000001",
            "boot_id": "00000000-0000-0000-0000-000000000002",
            "agent": {"id": "00000000-0000-0000-0000-000000000003", "handle": "d", "name": "D"},
            "runner_url": "ws://r:4105/v1/supervisor", "bootstrap_token": "aup_x",
            "gateway": {"proxy": "g:4102", "tls": true, "boot_token": "aub_x"}, "ca_pem": "PEM",
            "files": [{"path": path, "mode": "0600", "content_b64": b64("x"), "only_if_absent_or_sentinel": true}],
            "harness": {"id": "fake"}
        });
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(d.to_string())
    };
    let ca = root.join("ca.pem");
    // inside the home: written with the mode, parents created
    let ok = BootDocument::parse(&doc(&home.join(".config/app/c.json"))).unwrap();
    boot::write_boot_files_in(&ok, &ca, &home).unwrap();
    let m = std::fs::metadata(home.join(".config/app/c.json")).unwrap();
    assert_eq!(m.permissions().mode() & 0o777, 0o600);
    // `..` is refused at parse time
    assert!(BootDocument::parse(&doc(&home.join("../outside/x"))).is_err());
    // an absolute path outside the home, and a symlinked directory leading out, are refused
    for p in [outside.join("x"), home.join("link/x")] {
        let d = BootDocument::parse(&doc(&p)).unwrap();
        assert!(boot::write_boot_files_in(&d, &ca, &home).is_err(), "{p:?}");
        assert!(!outside.join("x").exists());
    }
    // the default home is /workspace
    let d = BootDocument::parse(&doc(&outside.join("y"))).unwrap();
    assert!(boot::write_boot_files(&d, &ca).is_err());
}

const CONNECT: &str = "CONNECT example.test:443 HTTP/1.1\r\nHost: example.test:443\r\n\r\n";

// SEC-40: the forwarder verifies the gateway certificate and sends no credential otherwise.
#[tokio::test]
async fn forwarder_refuses_a_wrong_server_name_without_sending_a_credential() {
    let (ca, acceptor) = tls_fixture(&["localhost"]);
    let (addr, seen) = stub_gateway_with(acceptor).await;
    let up = GatewayUpstream::new(addr, &ca, "not-the-gateway.example").unwrap();
    let p = spawn_proxy(
        LO.parse().unwrap(),
        up,
        Credential::new("x", "aub_boot"),
        ProxyMode::Mitm,
    )
    .await
    .unwrap();
    let r = raw(p.addr, CONNECT).await;
    assert!(r.starts_with("HTTP/1.1 502"), "{r}");
    assert!(r.contains("gateway_tls_failed"), "{r}");
    assert!(seen.lock().unwrap().is_empty(), "no credential may be sent");
    let r = raw(
        p.addr,
        "GET http://example.test/ HTTP/1.1\r\nHost: example.test\r\n\r\n",
    )
    .await;
    assert!(r.contains("gateway_tls_failed"), "{r}");
    assert!(seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn forwarder_refuses_an_untrusted_ca() {
    let (_, acceptor) = tls_fixture(&["localhost"]);
    let (other_ca, _) = tls_fixture(&["localhost"]);
    let (addr, seen) = stub_gateway_with(acceptor).await;
    let up = GatewayUpstream::new(addr, &other_ca, "localhost").unwrap();
    let p = spawn_proxy(
        LO.parse().unwrap(),
        up,
        Credential::new("x", "aub_boot"),
        ProxyMode::Mitm,
    )
    .await
    .unwrap();
    let r = raw(p.addr, CONNECT).await;
    assert!(r.contains("gateway_tls_failed"), "{r}");
    assert!(seen.lock().unwrap().is_empty());
}

// A plaintext gateway is never spoken to in cleartext: the forwarder only ever offers a
// ClientHello, so the credential never reaches the wire.
#[tokio::test]
async fn forwarder_never_falls_back_to_plaintext() {
    use tokio::io::AsyncReadExt as _;
    let l = tokio::net::TcpListener::bind(LO).await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let got = tokio::spawn(async move {
        let (mut c, _) = l.accept().await.unwrap();
        let mut buf = vec![0u8; 4096];
        let n = tokio::time::timeout(std::time::Duration::from_secs(2), c.read(&mut buf))
            .await
            .unwrap_or(Ok(0))
            .unwrap_or(0);
        buf.truncate(n);
        buf
    });
    let (ca, _) = tls_fixture(&["localhost"]);
    let up = GatewayUpstream::new(addr, &ca, "localhost").unwrap();
    let p = spawn_proxy(
        LO.parse().unwrap(),
        up,
        Credential::new("x", "aub_boot"),
        ProxyMode::Mitm,
    )
    .await
    .unwrap();
    let r = raw(p.addr, CONNECT).await;
    assert!(r.starts_with("HTTP/1.1 502"), "{r}");
    let seen = got.await.unwrap();
    assert_eq!(
        seen.first(),
        Some(&0x16),
        "first byte is a TLS handshake record"
    );
    assert!(!String::from_utf8_lossy(&seen).contains("Proxy-Authorization"));
}

#[test]
fn boot_document_requires_tls_true() {
    let dir = std::env::temp_dir().join("aurora-sup-tls-flag");
    let ok = boot_json(&dir);
    let decode = |s: &str| -> serde_json::Value {
        serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(s)
                .unwrap(),
        )
        .unwrap()
    };
    let encode = |v: &serde_json::Value| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string())
    };
    let doc = BootDocument::parse(&ok).unwrap();
    assert!(doc.gateway.tls);
    assert_eq!(doc.gateway.verify_name(), "aurora-gateway");
    let mut v = decode(&ok);
    v["gateway"]["tls"] = json!(false);
    assert!(BootDocument::parse(&encode(&v)).is_err());
    v["gateway"].as_object_mut().unwrap().remove("tls");
    assert!(BootDocument::parse(&encode(&v)).is_err());
    // server_name defaults to the host part of proxy
    let mut v = decode(&ok);
    v["gateway"].as_object_mut().unwrap().remove("server_name");
    let d = BootDocument::parse(&encode(&v)).unwrap();
    assert_eq!(d.gateway.verify_name(), "aurora-gateway");
}

/// (RFE-003) The `browser` capability gates the open proxy: absent (older documents) = none.
#[test]
fn boot_document_capabilities_gate_the_open_proxy() {
    let dir = std::env::temp_dir().join(format!("aurora-sup-{}", uuid::Uuid::now_v7()));
    let without = BootDocument::parse(&boot_json(&dir)).unwrap();
    assert!(without.capabilities.is_empty());
    assert!(!without.has_capability("browser"));

    let raw = boot_json(&dir);
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw)
        .unwrap();
    let mut v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    v["capabilities"] = json!(["browser"]);
    let with = BootDocument::parse(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string()),
    )
    .unwrap();
    assert!(with.has_capability("browser"));
    assert!(!with.has_capability("other"));
}
