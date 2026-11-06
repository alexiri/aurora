//! Connector manifest handling (docs/13 §3): canonical digest, string/asset validation and the
//! widening diff against the last approved manifest.
//!
//! The stored `connectors.manifest` jsonb holds the announced manifest plus two private envelope
//! keys that never leave this module's API:
//! * `_approved` — the last approved manifest (icon stripped), the baseline of the widening diff;
//! * `_widening` — the human-readable reasons the announced manifest needs approval.

use aurora_core::{FieldError, sanitize};
use aurora_protocol::connector::ConnectorManifest;
use aurora_registry::package::{check_guide, check_png};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

const APPROVED: &str = "_approved";
const WIDENING: &str = "_widening";

/// `sha256(canonical JSON of the manifest)`: the bytes that are signed and the approval digest.
pub fn digest(m: &ConnectorManifest) -> Result<[u8; 32], serde_json::Error> {
    let v = serde_json::to_value(m)?;
    Ok(Sha256::digest(aurora_registry::snapshot::canonical_bytes(&v)).into())
}

pub fn hex(d: &[u8]) -> String {
    use std::fmt::Write;
    d.iter()
        .fold(String::with_capacity(d.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// The manifest as announced (envelope keys removed).
pub fn public(stored: &Value) -> Value {
    let mut v = stored.clone();
    if let Some(o) = v.as_object_mut() {
        o.remove(APPROVED);
        o.remove(WIDENING);
    }
    v
}

/// Reasons recorded when the stored manifest was announced (empty once approved).
pub fn stored_widening(stored: &Value) -> Vec<String> {
    stored
        .get(WIDENING)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// The last approved manifest, if any.
pub fn baseline(stored: &Value) -> Option<ConnectorManifest> {
    serde_json::from_value(stored.get(APPROVED)?.clone()).ok()
}

fn without_icon(m: &Value) -> Value {
    let mut v = m.clone();
    if let Some(about) = v.get_mut("about").and_then(Value::as_object_mut) {
        about.remove("icon_png_b64");
    }
    v
}

/// Stored form of a freshly announced manifest. `approved` is the baseline to keep.
pub fn to_stored(
    m: &ConnectorManifest,
    approved: Option<&ConnectorManifest>,
    widening: &[String],
) -> Result<Value, serde_json::Error> {
    let mut v = serde_json::to_value(m)?;
    if let Some(o) = v.as_object_mut() {
        if let Some(a) = approved {
            o.insert(APPROVED.to_owned(), without_icon(&serde_json::to_value(a)?));
        }
        if !widening.is_empty() {
            o.insert(WIDENING.to_owned(), json!(widening));
        }
    }
    Ok(v)
}

/// Stored form after approval: the announced manifest becomes the baseline.
pub fn to_approved(stored: &Value) -> Value {
    let mut v = public(stored);
    let base = without_icon(&v);
    if let Some(o) = v.as_object_mut() {
        o.insert(APPROVED.to_owned(), base);
    }
    v
}

fn enabled(m: &ConnectorManifest) -> BTreeSet<&str> {
    m.enabled_capabilities().into_iter().collect()
}

fn field_key(f: &Value) -> String {
    let k = f
        .get("name")
        .or_else(|| f.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("?");
    sanitize::one_line(k, 64)
}

/// Form fields declared in `integration_setup.fields` and `presence_setup.dedicated.fields`.
fn setup_fields(m: &ConnectorManifest) -> Vec<(&'static str, Value)> {
    let mut out = Vec::new();
    let arr = |v: Option<&Value>| v.and_then(Value::as_array).cloned().unwrap_or_default();
    for f in arr(m.integration_setup.get("fields")) {
        out.push(("integration", f));
    }
    for f in arr(m
        .presence_setup
        .get("dedicated")
        .and_then(|d| d.get("fields")))
    {
        out.push(("presence", f));
    }
    out
}

/// Reasons `new` needs approval relative to the approved baseline; empty = accept
/// automatically (unchanged or narrowed). Without a baseline every registration waits.
pub fn widening(base: Option<&ConnectorManifest>, new: &ConnectorManifest) -> Vec<String> {
    let Some(base) = base else {
        return vec!["Awaiting first approval of this connector.".to_owned()];
    };
    let mut out = Vec::new();
    let was = enabled(base);
    for c in enabled(new) {
        if !was.contains(c) {
            out.push(format!("New capability: {}.", sanitize::one_line(c, 64)));
        }
    }
    let had: BTreeSet<(&str, String)> = setup_fields(base)
        .into_iter()
        .map(|(k, f)| (k, f.to_string()))
        .collect();
    for (kind, f) in setup_fields(new) {
        if !had.contains(&(kind, f.to_string())) {
            out.push(format!(
                "New or changed {kind} setup field: {}.",
                field_key(&f)
            ));
        }
    }
    let cb: BTreeSet<&str> = base.public_callbacks.iter().map(String::as_str).collect();
    for c in &new.public_callbacks {
        if !cb.contains(c.as_str()) {
            out.push(format!(
                "New public callback: {}.",
                sanitize::one_line(c, 64)
            ));
        }
    }
    if base.package.publisher != new.package.publisher {
        out.push(format!(
            "Publisher changed to {}.",
            sanitize::one_line(&new.package.publisher, 64)
        ));
    }
    out
}

fn clean(field: &str, s: &str, max: usize, errs: &mut Vec<FieldError>) {
    if s.is_empty() || sanitize::one_line(s, max) != s {
        errs.push(FieldError::new(
            field,
            "must be non-empty single-line text without control characters",
        ));
    }
}

/// String and asset rules (docs/13 §3: "strings are sanitized; the icon and guide follow
/// app-package asset rules"). Values are validated, never rewritten: the signature covers them.
pub fn validate(m: &ConnectorManifest) -> Vec<FieldError> {
    let mut errs = Vec::new();
    clean(
        "/manifest/package/version",
        &m.package.version,
        64,
        &mut errs,
    );
    clean(
        "/manifest/package/publisher",
        &m.package.publisher,
        64,
        &mut errs,
    );
    clean("/manifest/about/name", &m.about.name, 120, &mut errs);
    for (i, c) in m.public_callbacks.iter().enumerate() {
        clean(&format!("/manifest/public_callbacks/{i}"), c, 64, &mut errs);
    }
    for name in m.capabilities.keys() {
        clean(
            &format!("/manifest/capabilities/{name}"),
            name,
            64,
            &mut errs,
        );
    }
    let limits = aurora_registry::Limits::default();
    let mut vio = Vec::new();
    if let Some(icon) = &m.about.icon_png_b64 {
        match STANDARD.decode(icon) {
            Ok(bytes) => check_png("about.icon_png_b64", &bytes, limits.icon_max, &mut vio),
            Err(_) => errs.push(FieldError::new(
                "/manifest/about/icon_png_b64",
                "must be base64",
            )),
        }
    }
    if let Some(g) = &m.about.setup_guide_md {
        check_guide(g.as_bytes(), limits.guide_max, &mut vio);
    }
    errs.extend(vio.into_iter().map(|v| {
        FieldError::new(
            format!(
                "/manifest/about/{}",
                v.path.rsplit('.').next().unwrap_or("")
            ),
            format!("{}: {}", v.code, v.message),
        )
    }));
    errs
}

/// Decoded icon bytes of a stored manifest.
pub fn icon_bytes(stored: &Value) -> Option<Vec<u8>> {
    let b64 = stored.get("about")?.get("icon_png_b64")?.as_str()?;
    STANDARD.decode(b64).ok()
}
