//! The example repository as a fixture (P6-TPL-1): `aurora template check` on
//! `templates/examples` finds no problems, and packing each template twice is
//! byte-identical (C-1105, C-1144).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Runtime path first (nextest), compile-time path as the fallback.
fn aurora_bin() -> PathBuf {
    std::env::var_os("NEXTEST_BIN_EXE_aurora")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_aurora")))
}

fn aurora() -> Command {
    Command::new(aurora_bin())
}

fn examples() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/examples")
}

#[test]
fn check_examples_folder_has_no_problems() {
    let out = aurora()
        .args(["template", "check"])
        .arg(examples())
        .output()
        .expect("runs aurora template check");
    assert!(
        out.status.success(),
        "check failed:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("ok schedule-manager "),
        "missing schedule-manager in:\n{stdout}"
    );
    assert!(
        stdout.contains("ok service-monitor "),
        "missing service-monitor in:\n{stdout}"
    );
    assert_eq!(stdout.matches("ok ").count(), 2, "{stdout}");
}

#[test]
fn pack_examples_templates_twice_is_byte_identical() {
    let outdir =
        std::env::temp_dir().join(format!("aurora-examples-pack-{}", uuid::Uuid::now_v7()));
    fs::create_dir_all(&outdir).unwrap();
    for dir in ["schedule-manager", "service-monitor"] {
        let first = outdir.join("first.zip");
        let second = outdir.join("second.zip");
        for out in [&first, &second] {
            let r = aurora()
                .args(["template", "pack"])
                .arg(examples().join(dir))
                .arg("-o")
                .arg(out)
                .output()
                .expect("runs aurora template pack");
            assert!(
                r.status.success(),
                "pack {dir} failed:\n{}{}",
                String::from_utf8_lossy(&r.stdout),
                String::from_utf8_lossy(&r.stderr)
            );
        }
        let a = std::fs::read(&first).expect("first pack");
        let b = std::fs::read(&second).expect("second pack");
        assert_eq!(a, b, "{dir}: the two packs differ");
    }
    std::fs::remove_dir_all(&outdir).unwrap();
}
