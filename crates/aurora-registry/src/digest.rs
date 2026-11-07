//! Canonical package digest (09 §2): `sha256` of the text made of
//! `<path>\0<sha256 hex of content>\n` for every file except `SIGNATURE`,
//! sorted bytewise by path (`BTreeMap<String,_>` order is bytewise).
use sha2::{Digest, Sha256};

use crate::package::Package;

pub fn digest(pkg: &Package) -> [u8; 32] {
    let mut h = Sha256::new();
    for (path, content) in &pkg.files {
        h.update(path.as_bytes());
        h.update([0u8]);
        h.update(hex(&Sha256::digest(content)));
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
