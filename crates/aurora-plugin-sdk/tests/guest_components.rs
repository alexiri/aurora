//! Builds the example guests with `cargo build --target wasm32-wasip2` and runs
//! the registry validator on the resulting components. Skips (loudly) when the
//! target is not installed.
use std::path::{Path, PathBuf};
use std::process::Command;

fn target_installed() -> bool {
    let Ok(out) = Command::new("rustc").args(["--print", "sysroot"]).output() else {
        return false;
    };
    let sysroot = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Path::new(&sysroot)
        .join("lib/rustlib/wasm32-wasip2")
        .is_dir()
}

fn build_guest(name: &str) -> Option<Vec<u8>> {
    if !target_installed() {
        eprintln!("SKIP: wasm32-wasip2 target not installed (rustup target add wasm32-wasip2)");
        return None;
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target_dir = root.join("../../target/hook-guests");
    let status = Command::new(env!("CARGO"))
        .args(["build", "--release", "--target", "wasm32-wasip2"])
        .current_dir(root.join("tests/guests").join(name))
        .env("CARGO_TARGET_DIR", &target_dir)
        .env_remove("RUSTFLAGS")
        .status()
        .expect("spawn cargo");
    assert!(status.success(), "guest `{name}` failed to build");
    let wasm = target_dir.join(format!("wasm32-wasip2/release/guest_{name}.wasm"));
    Some(std::fs::read(wasm).expect("read wasm"))
}

#[test]
fn good_guest_is_accepted() {
    let Some(bytes) = build_guest("good") else {
        return;
    };
    let v = aurora_registry::validate::hooks_component("hooks.wasm", &bytes);
    assert!(v.is_empty(), "unexpected violations: {v:?}");
}

#[test]
fn fs_guest_is_rejected() {
    let Some(bytes) = build_guest("fs") else {
        return;
    };
    let v = aurora_registry::validate::hooks_component("hooks.wasm", &bytes);
    assert!(
        v.iter()
            .any(|x| x.code == "hooks_wasm_import_forbidden"
                && x.message.contains("wasi:filesystem")),
        "expected wasi:filesystem rejection, got: {v:?}"
    );
}

#[test]
fn core_module_and_garbage_rejected() {
    let core = b"\0asm\x01\0\0\0";
    let v = aurora_registry::validate::hooks_component("hooks.wasm", core);
    assert_eq!(v[0].code, "hooks_wasm_not_component");
    let v = aurora_registry::validate::hooks_component("hooks.wasm", b"nope");
    assert_eq!(v[0].code, "hooks_wasm_invalid");
}
