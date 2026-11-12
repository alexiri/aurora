//! The addon-repo manifest (`aurora-addons.yaml`, `api: 1`): strict parse and limits
//! (P5-ASRC, D-164; the format is documented in `docs/22-addon-repos.md`).

use aurora_core::limits::{ADDON_ADDONS_MAX, ADDON_MANIFEST_MAX, ADDON_VERSIONS_MAX, PACKAGE_MAX};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Why a manifest or its URL was refused.
#[derive(Debug)]
pub enum ManifestError {
    /// Non-https, credentials in the URL, IP-literal or non-public host.
    UrlRefused,
    /// Transport failure (DNS, connect, timeout, status >= 400) — sanitized reason.
    Unreachable(String),
    /// Body larger than [`ADDON_MANIFEST_MAX`].
    TooLarge,
    /// Not YAML, schema violation, unknown field, wrong `api`.
    Invalid(String),
}

impl ManifestError {
    /// One sanitized sentence for `check_failed_reason` (no URL query, no body text).
    #[must_use]
    pub fn reason(&self) -> String {
        match self {
            Self::UrlRefused => "the manifest URL was refused".to_owned(),
            Self::Unreachable(r) => format!("the source is unreachable: {r}"),
            Self::TooLarge => "the manifest is larger than 1 MiB".to_owned(),
            Self::Invalid(r) => format!("the manifest is invalid: {r}"),
        }
    }
}

/// One version of one addon.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct AddonVersion {
    /// https URL of the `.aurora-plugin` archive.
    pub package: String,
    /// sha256 of the archive bytes (hex, checked before parsing anything).
    pub sha256: String,
    /// Advisory size; the download enforces `PACKAGE_MAX` regardless.
    #[serde(default)]
    pub size_bytes: Option<u64>,
    /// The oldest core this version runs on (absent = no requirement).
    #[serde(default)]
    pub min_core: Option<String>,
}

/// One addon of the repo.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Addon {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// semver → version entry.
    pub versions: BTreeMap<String, AddonVersion>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
struct RepoInfo {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    contact: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
struct RawManifest {
    api: u32,
    repo: RepoInfo,
    publisher: String,
    signing_key: String,
    #[serde(default)]
    signing_key_description: Option<String>,
    addons: Vec<Addon>,
}

/// The parsed and validated manifest.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub api: u32,
    pub name: String,
    pub description: String,
    pub homepage: Option<String>,
    pub contact: Option<String>,
    pub publisher: String,
    /// Base64 ed25519 public key (the trust anchor of the source).
    pub signing_key: String,
    pub signing_key_description: Option<String>,
    pub addons: Vec<Addon>,
    /// sha256 of the raw manifest bytes (hex).
    pub digest: String,
}

/// The manifest contract this instance understands.
pub const API_VERSION: u32 = 1;

fn bad(m: impl Into<String>) -> ManifestError {
    ManifestError::Invalid(m.into())
}

/// The first line of an error message (parse errors list the whole document otherwise).
fn first_line(s: &str) -> &str {
    s.split('\n').next().unwrap_or(s)
}

fn check_url(kind: &str, raw: &str, allow_mailto: bool) -> Result<(), ManifestError> {
    if raw.is_empty() {
        return Err(bad(format!("{kind} must not be empty")));
    }
    if raw.len() > 2048 {
        return Err(bad(format!("{kind} is too long")));
    }
    let ok_scheme = raw.starts_with("https://") || (allow_mailto && raw.starts_with("mailto:"));
    if !ok_scheme {
        return Err(bad(format!("{kind} must be an https URL")));
    }
    Ok(())
}

/// Parse and validate manifest bytes (already fully downloaded, ≤ [`ADDON_MANIFEST_MAX`]).
pub fn parse(bytes: &[u8]) -> Result<Manifest, ManifestError> {
    if bytes.len() > ADDON_MANIFEST_MAX {
        return Err(ManifestError::TooLarge);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| bad("not UTF-8"))?;
    if serde_yaml::from_str::<serde_yaml::Value>(text)
        .map_err(|e| bad(format!("not YAML: {}", first_line(&e.to_string()))))?
        .as_str()
        .is_some()
    {
        return Err(bad("not a mapping"));
    }
    let raw: RawManifest =
        serde_yaml::from_str(text).map_err(|e| bad(first_line(&e.to_string())))?;
    if raw.api != API_VERSION {
        return Err(bad(format!(
            "unsupported manifest api {0} (this instance understands api {API_VERSION})",
            raw.api
        )));
    }
    let name = raw.repo.name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(bad("repo.name must be 1-80 characters"));
    }
    if raw.repo.description.chars().count() > 280 {
        return Err(bad("repo.description must be at most 280 characters"));
    }
    if let Some(h) = &raw.repo.homepage {
        check_url("repo.homepage", h, false)?;
    }
    if let Some(c) = &raw.repo.contact {
        check_url("repo.contact", c, true)?;
    }
    if !aurora_registry::manifest::valid_id(&raw.publisher) {
        return Err(bad("publisher must match [a-z][a-z0-9_-]{1,47}"));
    }
    let key = raw.signing_key.trim();
    aurora_crypto::parse_public_key_b64(key)
        .map_err(|_| bad("signing_key must be a base64 ed25519 public key"))?;
    if let Some(d) = &raw.signing_key_description {
        if d.trim().is_empty() || d.chars().count() > 80 {
            return Err(bad("signing_key_description must be 1-80 characters"));
        }
    }
    if raw.addons.is_empty() {
        return Err(bad("addons must list at least one addon"));
    }
    if raw.addons.len() > ADDON_ADDONS_MAX {
        return Err(bad(format!(
            "addons must list at most {ADDON_ADDONS_MAX} addons"
        )));
    }
    let mut seen = std::collections::BTreeSet::new();
    for a in &raw.addons {
        if !aurora_registry::manifest::valid_id(&a.id) {
            return Err(bad(format!(
                "addon id {:?} must match [a-z][a-z0-9_-]{{1,47}}",
                a.id
            )));
        }
        if !seen.insert(a.id.clone()) {
            return Err(bad(format!("duplicate addon id {:?}", a.id)));
        }
        let name = a.name.trim();
        if name.is_empty() || name.chars().count() > 80 {
            return Err(bad(format!(
                "addon {:?} name must be 1-80 characters",
                a.id
            )));
        }
        if a.description.chars().count() > 280 {
            return Err(bad(format!(
                "addon {:?} description must be at most 280 characters",
                a.id
            )));
        }
        if a.versions.is_empty() {
            return Err(bad(format!("addon {:?} must list a version", a.id)));
        }
        if a.versions.len() > ADDON_VERSIONS_MAX {
            return Err(bad(format!(
                "addon {:?} must list at most {ADDON_VERSIONS_MAX} versions",
                a.id
            )));
        }
        for (v, ver) in &a.versions {
            if aurora_registry::manifest::parse_semver(v).is_none() {
                return Err(bad(format!(
                    "addon {:?} version {v:?} is not a semver version",
                    a.id
                )));
            }
            if !ver.package.starts_with("https://") || ver.package.len() > 2048 {
                return Err(bad(format!(
                    "addon {:?} version {v:?} package must be an https URL",
                    a.id
                )));
            }
            if ver.sha256.len() != 64 || !ver.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(bad(format!(
                    "addon {:?} version {v:?} sha256 must be 64 hex characters",
                    a.id
                )));
            }
            if let Some(n) = ver.size_bytes.filter(|n| *n > PACKAGE_MAX as u64) {
                return Err(bad(format!(
                    "addon {:?} version {v:?} is larger than PACKAGE_MAX ({n} bytes)",
                    a.id
                )));
            }
            if let Some(mc) = &ver.min_core {
                if aurora_registry::manifest::parse_semver(mc).is_none() {
                    return Err(bad(format!(
                        "addon {:?} version {v:?} min_core {mc:?} is not a semver version",
                        a.id
                    )));
                }
            }
        }
    }
    Ok(Manifest {
        api: raw.api,
        name: name.to_owned(),
        description: raw.repo.description,
        homepage: raw.repo.homepage,
        contact: raw.repo.contact,
        publisher: raw.publisher,
        signing_key: key.to_owned(),
        signing_key_description: raw.signing_key_description,
        addons: raw.addons,
        digest: hex(&Sha256::digest(bytes)),
    })
}

fn hex(b: &[u8]) -> String {
    use std::fmt::Write;
    b.iter()
        .fold(String::with_capacity(b.len() * 2), |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        })
}

/// `sha256:<12 hex>` — the fingerprint shown to admins (never the key itself beyond
/// what registration already stores).
#[must_use]
pub fn key_fingerprint(public_key_b64: &str) -> String {
    let d = Sha256::digest(public_key_b64.trim().as_bytes());
    let h = hex(&d);
    format!("sha256:{}", &h[..12])
}

/// Normalize a pasted repo URL or manifest URL to the manifest URL
/// (`https://host/path/aurora-addons.yaml`). Refuses anything but https.
pub fn normalize_url(raw: &str) -> Result<String, ManifestError> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 2048 {
        return Err(ManifestError::UrlRefused);
    }
    let lower = raw.to_ascii_lowercase();
    if !lower.starts_with("https://") {
        return Err(ManifestError::UrlRefused);
    }
    if raw.rsplit('@').next().is_some_and(|a| a.contains(':'))
        && !raw[8..].split('/').next().unwrap_or("").contains(']')
        && raw[8..].split('/').next().unwrap_or("").contains('@')
    {
        return Err(ManifestError::UrlRefused);
    }
    if let Some(auth) = raw[8..].split('/').next() {
        if auth.contains('@') {
            return Err(ManifestError::UrlRefused);
        }
    }
    if lower.ends_with("/aurora-addons.yaml") {
        return Ok(raw.trim_end_matches('/').to_owned());
    }
    Ok(format!("{}/aurora-addons.yaml", raw.trim_end_matches('/')))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "H+zP/3e/AvDIupz+XgxH31uErU7l4ySFFV9lv9qiOpk=";

    fn manifest_yaml() -> String {
        format!(
            "api: 1\nrepo:\n  name: CERN addons\n  description: Test\n  homepage: https://cern.example\npublisher: cern\nsigning_key: {KEY}\naddons:\n  - id: acme\n    name: Acme\n    description: d\n    versions:\n      \"1.0.0\":\n        package: https://cdn.example/acme-1.0.0.aurora-plugin\n        sha256: {}\n        size_bytes: 10\n",
            "a".repeat(64)
        )
    }

    #[test]
    fn parses_a_valid_manifest() {
        let m = parse(manifest_yaml().as_bytes()).unwrap();
        assert_eq!(m.name, "CERN addons");
        assert_eq!(m.publisher, "cern");
        assert_eq!(m.addons.len(), 1);
        assert_eq!(m.digest.len(), 64);
    }

    #[test]
    fn rejects_wrong_api_unknown_fields_and_bad_keys() {
        assert!(matches!(
            parse(manifest_yaml().replace("api: 1", "api: 2").as_bytes()),
            Err(ManifestError::Invalid(_))
        ));
        assert!(matches!(
            parse(
                manifest_yaml()
                    .replace("signing_key:", "signing_key_x:")
                    .as_bytes()
            ),
            Err(ManifestError::Invalid(_))
        ));
        assert!(matches!(
            parse(manifest_yaml().replace(KEY, "not-a-key").as_bytes()),
            Err(ManifestError::Invalid(_))
        ));
    }

    #[test]
    fn rejects_bad_version_and_digest_fields() {
        assert!(matches!(
            parse(manifest_yaml().replace("\"1.0.0\"", "1.0").as_bytes()),
            Err(ManifestError::Invalid(_))
        ));
        assert!(matches!(
            parse(manifest_yaml().replace(&"a".repeat(64), "zz").as_bytes()),
            Err(ManifestError::Invalid(_))
        ));
    }

    #[test]
    fn normalizes_urls() {
        assert_eq!(
            normalize_url("https://github.com/cern/addons").unwrap(),
            "https://github.com/cern/addons/aurora-addons.yaml"
        );
        assert_eq!(
            normalize_url("https://github.com/cern/addons/").unwrap(),
            "https://github.com/cern/addons/aurora-addons.yaml"
        );
        assert_eq!(
            normalize_url("https://raw.example/cern/addons/aurora-addons.yaml").unwrap(),
            "https://raw.example/cern/addons/aurora-addons.yaml"
        );
        for u in ["http://x.example/a", "https://u:p@x.example/a", "ftp://x"] {
            assert!(
                matches!(normalize_url(u), Err(ManifestError::UrlRefused)),
                "{u}"
            );
        }
    }

    #[test]
    fn fingerprints_are_short_and_stable() {
        let f = key_fingerprint(KEY);
        assert!(f.starts_with("sha256:"));
        assert_eq!(f.len(), "sha256:".len() + 12);
        assert_eq!(f, key_fingerprint(&format!("{KEY}\n")));
    }
}
