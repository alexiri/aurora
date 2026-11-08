#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Runtime path first (nextest sets `NEXTEST_BIN_EXE_aurora` when run from an archive that was
/// built elsewhere), compile-time path as the fallback for plain `cargo test`.
fn aurora_bin() -> PathBuf {
    std::env::var_os("NEXTEST_BIN_EXE_aurora")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_aurora")))
}

fn aurora(args: &[&str]) -> Output {
    Command::new(aurora_bin())
        .args(args)
        .env("AURORA_GATEWAY_BIN", gateway())
        .output()
        .unwrap()
}

fn gateway() -> PathBuf {
    if let Some(p) = std::env::var_os("AURORA_GATEWAY_BIN") {
        return PathBuf::from(p);
    }
    let exe = aurora_bin();
    exe.parent().unwrap().join("aurora-gateway")
}

fn tmp() -> PathBuf {
    let d = std::env::temp_dir().join(format!("aurora-cli-test-{}", uuid::Uuid::now_v7().simple()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

fn ok(o: &Output) -> String {
    assert!(
        o.status.success(),
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn keygen_sign_validate_round_trip_and_compile() {
    let d = tmp();
    ok(&aurora(&["plugin", "new", "demo", "--dir", s(&d)]));
    let src = d.join("demo");
    let arch = d.join("demo.aurora-plugin");
    ok(&aurora(&["plugin", "build", s(&src), "--out", s(&arch)]));
    // Unsigned archive is refused, accepted with --allow-unsigned.
    let o = aurora(&["plugin", "validate", s(&arch)]);
    assert!(!o.status.success());
    ok(&aurora(&[
        "plugin",
        "validate",
        s(&arch),
        "--allow-unsigned",
    ]));
    let key = d.join("k");
    ok(&aurora(&["plugin", "keygen", "--out", s(&key)]));
    assert!(
        !aurora(&["plugin", "keygen", "--out", s(&key)])
            .status
            .success()
    );
    ok(&aurora(&["plugin", "sign", s(&arch), "--key", s(&key)]));
    let pubk = d.join("k.pub");
    ok(&aurora(&[
        "plugin",
        "validate",
        s(&arch),
        "--pubkey",
        s(&pubk),
    ]));
    // Signature by an untrusted key fails.
    assert!(!aurora(&["plugin", "validate", s(&arch)]).status.success());
    let frag: serde_json::Value =
        serde_json::from_str(&ok(&aurora(&["plugin", "compile", s(&arch)]))).unwrap();
    assert_eq!(frag["hosts"][0]["host"], "api.example.com");
    let docs = ok(&aurora(&["plugin", "docs", s(&arch)]));
    assert!(docs.contains("## Permissions"));
}

#[test]
fn test_runs_fixtures_through_gateway() {
    let d = tmp();
    ok(&aurora(&["plugin", "new", "demo", "--dir", s(&d)]));
    let src = d.join("demo");
    let out: serde_json::Value =
        serde_json::from_str(&ok(&aurora(&["plugin", "test", s(&src)]))).unwrap();
    assert_eq!(out["ok"], true);
    assert_eq!(out["fixtures"]["passed"], 2);
    // Break an expectation: the run fails and reports it.
    let f = src.join("tests/basic.yaml");
    let t = std::fs::read_to_string(&f)
        .unwrap()
        .replace("Bearer tok", "Bearer nope");
    std::fs::write(&f, t).unwrap();
    let o = aurora(&["plugin", "test", s(&src)]);
    assert!(!o.status.success());
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["fixtures"]["failed"], 1);
}

#[test]
fn build_requires_hooks_wasm_when_hooks_declared() {
    let d = tmp();
    ok(&aurora(&["plugin", "new", "demo", "--dir", s(&d)]));
    let src = d.join("demo");
    let y = std::fs::read_to_string(src.join("plugin.yaml")).unwrap();
    std::fs::write(
        src.join("plugin.yaml"),
        format!("{y}permissions: {{ hooks: [approval.summarize] }}\n"),
    )
    .unwrap();
    let o = aurora(&[
        "plugin",
        "build",
        s(&src),
        "--out",
        s(&d.join("x.aurora-plugin")),
    ]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("hooks.wasm"));
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dst = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &dst);
        } else {
            std::fs::copy(e.path(), dst).unwrap();
        }
    }
}

/// A fresh copy of the built-in GitLab package (its recorded token exchanges are the subject).
fn gitlab_copy() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/builtin/gitlab");
    let dst = tmp().join("gitlab");
    copy_dir(&root, &dst);
    dst
}

fn token_report(src: &Path) -> (bool, serde_json::Value) {
    let o = aurora(&["plugin", "test", s(src)]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    (o.status.success(), v)
}

/// P2-CAT-T: `aurora plugin test` replays refresh / re-auth / unavailable against the recorded
/// token endpoint, and a deliberately broken recorded exchange fails the case.
#[test]
fn token_lifecycle_replays_and_a_broken_exchange_fails() {
    let src = gitlab_copy();
    let (ok_run, v) = token_report(&src);
    assert!(ok_run, "{v}");
    let results = v["fixtures"]["results"].as_array().unwrap();
    let lifecycle: Vec<_> = results
        .iter()
        .filter(|r| r["observed"]["token"].is_object())
        .collect();
    assert_eq!(lifecycle.len(), 6, "{v}");
    let outcomes: Vec<&str> = lifecycle
        .iter()
        .map(|r| r["observed"]["token"]["outcome"].as_str().unwrap())
        .collect();
    assert_eq!(outcomes.iter().filter(|o| **o == "refreshed").count(), 2);
    assert_eq!(
        outcomes.iter().filter(|o| **o == "reauth_required").count(),
        2
    );
    assert_eq!(outcomes.iter().filter(|o| **o == "unavailable").count(), 2);
    // The retried request carries the new token; the refresh token never left its instance.
    assert!(lifecycle.iter().any(
        |r| r["observed"]["token"]["retried"]["headers"]["authorization"]
            == "Bearer tok-fixture-new"
    ));
    // Credentials of the refresh are not echoed into the report.
    assert!(!v.to_string().contains("refresh-fixture-2"));

    // Broken recording 1: the instance's token endpoint recorded under gitlab.com (the case would
    // pass only if the request had been built for gitlab.com: SEC-59).
    let f = src.join("tests/token.yaml");
    let good = std::fs::read_to_string(&f).unwrap();
    let wrong_host = good.replace(
        "method: POST, url: \"https://gitlab.example.com/oauth/token\"",
        "method: POST, url: \"https://gitlab.com/oauth/token\"",
    );
    assert_ne!(wrong_host, good);
    std::fs::write(&f, wrong_host).unwrap();
    let (ok_run, v) = token_report(&src);
    assert!(!ok_run);
    assert!(v["fixtures"]["failed"].as_u64().unwrap() >= 2, "{v}");
    assert!(v.to_string().contains("unused cassette entry"), "{v}");

    // Broken recording 2: the recorded request does not match the refresh the gateway builds.
    let wrong_form = good.replace(
        "form: { grant_type: refresh_token, refresh_token: \"refresh-fixture-1\", client_id: \"client-fixture\" }",
        "form: { grant_type: refresh_token, refresh_token: \"someone-else\" }",
    );
    assert_ne!(wrong_form, good);
    std::fs::write(&f, wrong_form).unwrap();
    let (ok_run, v) = token_report(&src);
    assert!(!ok_run);
    assert!(v["fixtures"]["failed"].as_u64().unwrap() >= 2, "{v}");

    // Broken recording 3: a stale answer the package does not declare is not refreshed.
    let not_stale = good.replace(
        "upstream: { status: 401, body: '{\"error\":\"invalid_token\"}' }",
        "upstream: { status: 500, body: '{\"error\":\"invalid_token\"}' }",
    );
    assert_ne!(not_stale, good);
    std::fs::write(&f, not_stale).unwrap();
    let (ok_run, v) = token_report(&src);
    assert!(!ok_run);
    assert!(v.to_string().contains("stale_codes"), "{v}");
}
