//! Canonical package digest (09 §2): `sha256` of the text made of
//! `<path>\0<sha256 hex of content>\n` for every file except `SIGNATURE`,
//! sorted bytewise by path (`BTreeMap<String,_>` order is bytewise).
//!
//! (P5-APPSKILL-1, C-1131) An entry under `skills/` with the owner-execute bit has the line
//! `<path>\0<sha256 hex>\0x\n`; every other line is unchanged, so the digest of a package
//! without `skills/` is byte for byte what it always was. This is the one function: build, sign,
//! verify and API ingest all call it.
use sha2::{Digest, Sha256};

use crate::package::Package;

pub fn digest(pkg: &Package) -> [u8; 32] {
    let mut h = Sha256::new();
    for (path, content) in &pkg.files {
        h.update(path.as_bytes());
        h.update([0u8]);
        h.update(hex(&Sha256::digest(content)));
        if pkg.executable.contains(path) && path.starts_with(crate::package::SKILLS_DIR) {
            h.update(b"\0x");
        }
        h.update(b"\n");
    }
    h.finalize().into()
}

fn hex(b: &[u8]) -> String {
    use std::fmt::Write;
    b.iter()
        .fold(String::with_capacity(b.len() * 2), |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        })
}
