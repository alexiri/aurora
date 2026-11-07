//! Package builder: produces `.aurora-plugin` tar.gz bytes from a manifest and
//! assets, optionally signed. Used by tests and (later) `aurora plugin build`.
//! Also able to emit deliberately malformed archives for negative tests.
use std::io::Write;

use aurora_crypto::SigningKey;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use flate2::Compression;
use flate2::write::GzEncoder;
use tar::{EntryType, Header};

use crate::digest::digest;
use crate::package::Package;
use crate::signature::MAGIC;

/// Minimal PNG header stub (signature + IHDR only; enough for the validator's
/// dimension check, not a decodable image) of `w` x `h` pixels.
pub fn png_stub(w: u32, h: u32) -> Vec<u8> {
    let mut b = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13];
    b.extend_from_slice(b"IHDR");
    b.extend_from_slice(&w.to_be_bytes());
    b.extend_from_slice(&h.to_be_bytes());
    b.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
    b
}

struct Raw {
    path: String,
    ty: EntryType,
    data: Vec<u8>,
    link: Option<String>,
}

pub struct PackageBuilder {
    entries: Vec<Raw>,
}

impl PackageBuilder {
    /// Starts with `plugin.yaml`, a 64x64 icon and a plain guide.
    pub fn new(manifest_yaml: &str) -> Self {
        let mut b = Self {
            entries: Vec::new(),
        };
        b.set_file("plugin.yaml", manifest_yaml.as_bytes());
        b.set_file("assets/icon.png", &png_stub(64, 64));
        b.set_file("docs/setup.md", b"# Setup\n\nPaste your token.\n");
        b
    }

    /// Insert or replace a regular file.
    pub fn set_file(&mut self, path: &str, data: &[u8]) -> &mut Self {
        if let Some(e) = self.entries.iter_mut().find(|e| e.path == path) {
            e.data = data.to_vec();
        } else {
            self.push_raw(path, EntryType::Regular, data);
        }
        self
    }

    pub fn remove_file(&mut self, path: &str) -> &mut Self {
        self.entries.retain(|e| e.path != path);
        self
    }

    /// Append an entry verbatim (no path checks, duplicates allowed).
    pub fn push_raw(&mut self, path: &str, ty: EntryType, data: &[u8]) -> &mut Self {
        self.entries.push(Raw {
            path: path.to_owned(),
            ty,
            data: data.to_vec(),
            link: None,
        });
        self
    }

    pub fn push_symlink(&mut self, path: &str, target: &str) -> &mut Self {
        self.entries.push(Raw {
            path: path.to_owned(),
            ty: EntryType::Symlink,
            data: Vec::new(),
            link: Some(target.to_owned()),
        });
        self
    }

    /// Sign the *current* regular-file contents; later `set_file` calls
    /// therefore invalidate the signature (tamper tests).
    pub fn sign(&mut self, key: &SigningKey, key_id: &str) -> &mut Self {
        let mut pkg = Package {
            files: Default::default(),
            signature: None,
        };
        for e in self
            .entries
            .iter()
            .filter(|e| e.path != "SIGNATURE" && matches!(e.ty, EntryType::Regular))
        {
            pkg.files.insert(e.path.clone(), e.data.clone());
        }
        let sig = ed25519_dalek::Signer::sign(key, &digest(&pkg)).to_bytes();
        let text = format!("{MAGIC}\n{key_id}\n{}\n", STANDARD.encode(sig));
        self.set_file("SIGNATURE", text.as_bytes())
    }

    /// `SIGNATURE` text (`aurora-sig-v1\n<key_id>\n<b64>\n`) signing a raw 32-byte `digest`
    /// (used for connector manifests, whose digest is sha256 of the canonical JSON).
    pub fn sign_digest(key: &SigningKey, key_id: &str, digest: &[u8; 32]) -> String {
        let sig = ed25519_dalek::Signer::sign(key, digest).to_bytes();
        format!("{MAGIC}\n{key_id}\n{}\n", STANDARD.encode(sig))
    }

    /// Base64 public key id for `key`.
    pub fn key_id(key: &SigningKey) -> String {
        STANDARD.encode(key.verifying_key().to_bytes())
    }

    pub fn build(&self) -> std::io::Result<Vec<u8>> {
        let mut tar = tar::Builder::new(Vec::new());
        for e in &self.entries {
            let mut h = Header::new_gnu();
            let name = e.path.as_bytes();
            let n = name.len().min(100);
            if let Some(dst) = h.as_old_mut().name.get_mut(..n) {
                dst.copy_from_slice(name.get(..n).unwrap_or_default());
            }
            h.set_entry_type(e.ty);
            h.set_mode(0o644);
            h.set_size(e.data.len() as u64);
            if let Some(l) = &e.link {
                h.set_link_name(l)?;
            }
            h.set_cksum();
            tar.append(&h, e.data.as_slice())?;
        }
        let raw = tar.into_inner()?;
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(&raw)?;
        gz.finish()
    }
}
