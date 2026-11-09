//! `sqlx::migrate!("./migrations")` (src/lib.rs) reads the migration files at compile time
//! without telling rustc or cargo about them. Without this script a new or edited migration does
//! not rebuild the crate in an existing `target/`, and a compilation cache (sccache, used by the
//! lane wrapper and the release image builder) can serve an object that embeds the old set.
//! CONTRACT-001 hit exactly that: the dev `target/` kept the pre-drop migrations.
//!
//! So: re-run when the directory or any file changes, and pass a digest of every migration's name
//! and bytes to rustc as an environment variable that `lib.rs` reads with `env!`. The variable
//! name starts with `CARGO_` so cache keys that hash `CARGO_*` variables change with it, and the
//! `env!` use makes it a tracked input of the compilation either way.

use std::fs;
use std::path::PathBuf;

fn main() -> std::io::Result<()> {
    let dir = PathBuf::from("migrations");
    println!("cargo:rerun-if-changed={}", dir.display());

    let mut files: Vec<PathBuf> = fs::read_dir(&dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    files.sort();

    // FNV-1a over (file name, 0, bytes, 0) for every file, in sorted order.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |bytes: &[u8]| {
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for p in &files {
        println!("cargo:rerun-if-changed={}", p.display());
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        mix(name.as_bytes());
        mix(&[0]);
        mix(&fs::read(p)?);
        mix(&[0]);
    }
    println!(
        "cargo:rustc-env=CARGO_AURORA_MIGRATIONS_DIGEST={h:016x}-{}",
        files.len()
    );
    Ok(())
}
