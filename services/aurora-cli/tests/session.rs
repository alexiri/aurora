#![allow(clippy::unwrap_used, clippy::expect_used)]
//! FIX-118 (PKG-6, PKG-7): the CLI refuses a cleartext server URL before any request, a login to
//! another server drops the stored agent tokens, and `logout` says what it did not revoke.
use std::path::Path;
use std::process::{Command, Output};

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A scratch config directory, removed on drop.
struct Tmp(std::path::PathBuf);

impl Tmp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("aurora-cli-session-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runtime path first (nextest sets `NEXTEST_BIN_EXE_aurora` when CI runs the shards from an
/// archive built elsewhere, where the compile-time path does not exist), compile-time path for
/// plain `cargo test` — as `tests/plugin.rs`.
fn aurora_bin() -> std::path::PathBuf {
    std::env::var_os("NEXTEST_BIN_EXE_aurora")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_BIN_EXE_aurora")))
}

fn aurora(dir: &Path, args: &[&str]) -> Output {
    Command::new(aurora_bin())
        .args(args)
        .env("AURORA_CONFIG_DIR", dir)
        .env_remove("AURORA_URL")
        .env("HOME", dir)
        .output()
        .expect("spawn aurora")
}

fn config(url: &str) -> String {
    format!(
        "url = \"{url}\"\napi_key = \"auk_old\"\n\n[workspace]\nid = \"w0\"\nslug = \"old\"\nname = \"Old\"\norg_id = \"o0\"\norg_slug = \"oldorg\"\n\n[agents.helper]\ntoken = \"agt_old\"\n"
    )
}

async fn device_server() -> MockServer {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/auth/cli"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "device_code": "dev", "user_code": "ABCD-EFGH",
            "verification_url": "http://127.0.0.1/verify", "interval": 1, "expires_in": 30
        })))
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/auth/cli/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "api_key": "auk_new",
            "workspace": {"id":"w1","slug":"main","name":"Main","org_id":"o1","org_slug":"acme"}
        })))
        .mount(&s)
        .await;
    s
}

#[test]
fn login_to_a_cleartext_url_fails_before_any_request() {
    let dir = Tmp::new();
    for url in [
        "http://example.org",
        "http://10.0.0.5:3000",
        "ftp://localhost",
    ] {
        let out = aurora(dir.path(), &["login", "--url", url]);
        assert!(!out.status.success(), "{url}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains("cleartext") || err.contains("not a valid server URL"),
            "{url}: {err}"
        );
        // Not a connection error: nothing was dialled.
        assert!(!err.contains("request failed"), "{url}: {err}");
    }
    // The stored URL is held to the same rule by every command.
    std::fs::write(dir.path().join("config.toml"), config("http://example.org")).unwrap();
    let out = aurora(dir.path(), &["whoami"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("cleartext"));
}

#[tokio::test(flavor = "multi_thread")]
async fn login_to_another_server_drops_stored_agent_tokens_the_same_server_keeps_them() {
    let s = device_server().await;
    let dir = Tmp::new();
    let cfg = dir.path().join("config.toml");

    // A previous server: its agent token must not follow the new login.
    std::fs::write(&cfg, config("https://old.example.org")).unwrap();
    let uri = s.uri();
    let d = dir.path().to_owned();
    let out = tokio::task::spawn_blocking(move || aurora(&d, &["login", "--url", &uri]))
        .await
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(&cfg).unwrap();
    assert!(
        text.contains("auk_new") && !text.contains("agt_old"),
        "{text}"
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("Dropped 1 stored agent token"));

    // The same server (trailing slash and case ignored): tokens are kept.
    std::fs::write(
        &cfg,
        config(&format!(
            "{}/",
            s.uri().to_uppercase().replace("HTTP", "http")
        )),
    )
    .unwrap();
    let (uri, d) = (s.uri(), dir.path().to_owned());
    let out = tokio::task::spawn_blocking(move || aurora(&d, &["login", "--url", &uri]))
        .await
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(std::fs::read_to_string(&cfg).unwrap().contains("agt_old"));
}

#[test]
fn logout_says_the_key_was_not_revoked() {
    let dir = Tmp::new();
    std::fs::write(
        dir.path().join("config.toml"),
        config("https://old.example.org"),
    )
    .unwrap();
    let out = aurora(dir.path(), &["logout"]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("NOT revoked"), "{text}");
    assert!(text.contains("1 stored agent token"), "{text}");
    assert!(!dir.path().join("config.toml").exists());
    let out = aurora(dir.path(), &["logout"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("Not logged in"));
}
