#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn aurora(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aurora"))
        .args(args)
        .env("AURORA_GATEWAY_BIN", gateway())
        .output()
        .unwrap()
}

fn gateway() -> PathBuf {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_aurora"));
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
