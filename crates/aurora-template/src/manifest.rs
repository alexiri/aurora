//! The repository manifest `aurora-templates.yaml` (C-1103): a strict parse and the rendering of
//! `packages.url`. No signing key and no publisher (D-435(b)): the admin's review is the gate.

use serde::Deserialize;

use aurora_core::limits;

use crate::yaml::parse_strict_yaml;

/// Why a manifest or a package URL was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestProblemCode {
    /// `template_manifest_invalid`: not readable, not format 1, a field out of range.
    ManifestInvalid,
    /// `template_manifest_too_large`: over `TEMPLATE_MANIFEST_MAX`.
    ManifestTooLarge,
    /// `template_package_url_invalid`: `packages.url` does not render to a usable https URL.
    PackageUrlInvalid,
}

impl ManifestProblemCode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ManifestInvalid => "template_manifest_invalid",
            Self::ManifestTooLarge => "template_manifest_too_large",
            Self::PackageUrlInvalid => "template_package_url_invalid",
        }
    }
}

impl std::fmt::Display for ManifestProblemCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One manifest refusal: the code and one fixed sentence (the message never echoes input).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestProblem {
    pub code: ManifestProblemCode,
    pub message: String,
}

impl ManifestProblem {
    fn new(code: ManifestProblemCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ManifestProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestRaw {
    api: serde_yaml::Value,
    repo: RepoRaw,
    packages: PackagesRaw,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepoRaw {
    name: String,
    description: String,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    contact: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PackagesRaw {
    url: String,
}

fn invalid(message: impl Into<String>) -> ManifestProblem {
    ManifestProblem::new(ManifestProblemCode::ManifestInvalid, message)
}

/// `parse_manifest`: `aurora-templates.yaml`, strictly (C-1103).
pub fn parse_manifest(bytes: &[u8]) -> Result<crate::model::TemplateManifest, ManifestProblem> {
    if bytes.len() > limits::TEMPLATE_MANIFEST_MAX {
        return Err(ManifestProblem::new(
            ManifestProblemCode::ManifestTooLarge,
            format!(
                "aurora-templates.yaml is larger than {} bytes",
                limits::TEMPLATE_MANIFEST_MAX
            ),
        ));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| invalid("aurora-templates.yaml is not valid UTF-8"))?;
    let value =
        parse_strict_yaml(text).map_err(|m| invalid(format!("aurora-templates.yaml: {m}")))?;
    let raw: ManifestRaw = serde_path_to_error::deserialize(value)
        .map_err(|_| invalid("a key is missing, unknown, or has the wrong type"))?;
    if raw.api.as_u64() != Some(1) {
        return Err(invalid("api must be 1"));
    }
    let name = clean_1(
        &raw.repo.name,
        80,
        "the repository name must be 1 to 80 characters",
    )?;
    let description = clean_0(
        &raw.repo.description,
        280,
        "the description must be at most 280 characters",
    )?;
    let homepage = raw
        .repo
        .homepage
        .map(|h| clean_0(&h, 280, "the homepage must be at most 280 characters"))
        .transpose()?;
    let contact = raw
        .repo
        .contact
        .map(|c| clean_0(&c, 280, "the contact must be at most 280 characters"))
        .transpose()?;
    let url = check_url_template(&raw.packages.url)?;
    Ok(crate::model::TemplateManifest {
        api: 1,
        repo: crate::model::RepoInfo {
            name,
            description,
            homepage,
            contact,
        },
        packages: crate::model::Packages { url },
    })
}

/// One-line cleaning then the 1..max length check.
fn clean_1(s: &str, max: usize, message: &str) -> Result<String, ManifestProblem> {
    let cleaned = aurora_core::sanitize::one_line(s, usize::MAX);
    let trimmed = cleaned.trim();
    let n = trimmed.chars().count();
    if n == 0 || n > max {
        return Err(invalid(message));
    }
    Ok(trimmed.to_owned())
}

/// One-line cleaning then the ≤ max length check (an empty value is allowed).
fn clean_0(s: &str, max: usize, message: &str) -> Result<String, ManifestProblem> {
    let cleaned = aurora_core::sanitize::one_line(s, usize::MAX);
    let trimmed = cleaned.trim();
    if trimmed.chars().count() > max {
        return Err(invalid(message));
    }
    Ok(trimmed.to_owned())
}

/// The `packages.url` template: https, no user info, query or fragment, and the only braces are
/// `{id}` and `{version}` (each at least once; C-1103).
fn check_url_template(url: &str) -> Result<String, ManifestProblem> {
    url_problems(url, true)
        .map(|()| url.to_owned())
        .map_err(|m| ManifestProblem::new(ManifestProblemCode::PackageUrlInvalid, m))
}

/// The checks a package URL (template or rendered) must pass; `template` also requires the
/// `{id}` and `{version}` placeholders.
fn url_problems(url: &str, template: bool) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err("packages.url must be an https URL".into());
    }
    if url.contains('?') {
        return Err("packages.url must not carry a query string".into());
    }
    if url.contains('#') {
        return Err("packages.url must not carry a fragment".into());
    }
    let host_end = url[8..].find('/').map_or(url.len(), |i| i + 8);
    if url[8..host_end].contains('@') {
        return Err("packages.url must not carry user info".into());
    }
    let mut saw_id = false;
    let mut saw_version = false;
    let bytes = url.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            let Some(end) = url[i..].find('}') else {
                return Err("packages.url has an unclosed {".into());
            };
            let token = &url[i + 1..i + end];
            match token {
                "id" => saw_id = true,
                "version" => saw_version = true,
                _ => {
                    return Err(
                        "packages.url may only use the placeholders {id} and {version}".into(),
                    );
                }
            }
            i += end + 1;
        } else {
            i += 1;
        }
    }
    if template && (!saw_id || !saw_version) {
        return Err("packages.url must use the placeholders {id} and {version}".into());
    }
    Ok(())
}

/// `render_package_url`: substitutes `{id}` and `{version}` and checks the rendered URL
/// (at most 500 characters, https, no user info, query or fragment; C-1103).
pub fn render_package_url(
    manifest: &crate::model::TemplateManifest,
    id: &str,
    version: &str,
) -> Result<String, ManifestProblem> {
    if id.is_empty() || version.is_empty() {
        return Err(ManifestProblem::new(
            ManifestProblemCode::PackageUrlInvalid,
            "a template id and version are needed to render packages.url",
        ));
    }
    let rendered = manifest
        .packages
        .url
        .replace("{id}", id)
        .replace("{version}", version);
    if rendered.len() > 500 {
        return Err(ManifestProblem::new(
            ManifestProblemCode::PackageUrlInvalid,
            "the rendered packages.url is longer than 500 characters",
        ));
    }
    url_problems(&rendered, false)
        .map(|_| rendered)
        .map_err(|m| ManifestProblem::new(ManifestProblemCode::PackageUrlInvalid, m))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const OK: &str = "api: 1\nrepo:\n  name: CERN agent templates\n  description: Templates maintained by the CERN agent team.\n  homepage: https://example.org/agents\n  contact: mailto:agents@example.org\npackages:\n  url: https://github.com/cern/agent-templates/releases/download/{id}-v{version}/{id}-{version}.zip\n";

    #[test]
    fn ok_manifest() {
        let m = parse_manifest(OK.as_bytes()).unwrap();
        assert_eq!(m.api, 1);
        assert_eq!(m.repo.name, "CERN agent templates");
        assert_eq!(
            m.packages.url,
            "https://github.com/cern/agent-templates/releases/download/{id}-v{version}/{id}-{version}.zip"
        );
        let rendered = render_package_url(&m, "service-monitor", "1.2.3").unwrap();
        assert_eq!(
            rendered,
            "https://github.com/cern/agent-templates/releases/download/service-monitor-v1.2.3/service-monitor-1.2.3.zip"
        );
    }

    #[test]
    fn gitlab_shape() {
        let yaml = "api: 1\nrepo:\n  name: n\n  description: d\npackages:\n  url: https://gitlab.example.org/api/v4/projects/42/packages/generic/{id}/{version}/{id}-{version}.zip\n";
        let m = parse_manifest(yaml.as_bytes()).unwrap();
        assert_eq!(
            render_package_url(&m, "a", "0.1.0").unwrap(),
            "https://gitlab.example.org/api/v4/projects/42/packages/generic/a/0.1.0/a-0.1.0.zip"
        );
    }

    #[test]
    fn refusals() {
        for (yaml, code) in [
            (
                "api: 2\nrepo:\n  name: n\n  description: d\npackages:\n  url: https://x/{id}/{version}\n",
                ManifestProblemCode::ManifestInvalid,
            ),
            (
                "repo:\n  name: n\n  description: d\npackages:\n  url: https://x/{id}/{version}\n",
                ManifestProblemCode::ManifestInvalid,
            ),
            (
                "api: 1\nrepo:\n  name: n\n  description: d\npackages:\n  url: http://x/{id}-{version}.zip\n",
                ManifestProblemCode::PackageUrlInvalid,
            ),
            (
                "api: 1\nrepo:\n  name: n\n  description: d\npackages:\n  url: https://user:pw@x/{id}/{version}\n",
                ManifestProblemCode::PackageUrlInvalid,
            ),
            (
                "api: 1\nrepo:\n  name: n\n  description: d\npackages:\n  url: https://x/{id}/{version}?token=a\n",
                ManifestProblemCode::PackageUrlInvalid,
            ),
            (
                "api: 1\nrepo:\n  name: n\n  description: d\npackages:\n  url: https://x/{id}\n",
                ManifestProblemCode::PackageUrlInvalid,
            ),
            (
                "api: 1\nrepo:\n  name: n\n  description: d\npackages:\n  url: https://x/{id}/{version}/{other}\n",
                ManifestProblemCode::PackageUrlInvalid,
            ),
            (
                "api: 1\nrepo:\n  name: n\n  description: d\n  homepage: https://x\n  contact: mailto:a@b.c\n  extra: 1\npackages:\n  url: https://x/{id}/{version}\n",
                ManifestProblemCode::ManifestInvalid,
            ),
            (
                "api: &a 1\nrepo:\n  name: n\n  description: d\npackages:\n  url: https://x/{id}/{version}\n",
                ManifestProblemCode::ManifestInvalid,
            ),
        ] {
            let err = parse_manifest(yaml.as_bytes()).unwrap_err();
            assert_eq!(err.code, code, "{yaml}");
        }
        let empty_name = "api: 1\nrepo:\n  name: \"\"\n  description: d\npackages:\n  url: https://x/{id}/{version}\n";
        assert_eq!(
            parse_manifest(empty_name.as_bytes()).unwrap_err().code,
            ManifestProblemCode::ManifestInvalid
        );
        let long = format!(
            "api: 1\nrepo:\n  name: {}\n  description: d\npackages:\n  url: https://x/{{id}}/{{version}}\n",
            "n".repeat(81)
        );
        assert_eq!(
            parse_manifest(long.as_bytes()).unwrap_err().code,
            ManifestProblemCode::ManifestInvalid
        );
        let big = vec![b' '; limits::TEMPLATE_MANIFEST_MAX + 1];
        assert_eq!(
            parse_manifest(&big).unwrap_err().code,
            ManifestProblemCode::ManifestTooLarge
        );
    }

    #[test]
    fn rendered_url_checks() {
        let m = parse_manifest(OK.as_bytes()).unwrap();
        // A very long id pushes the rendered URL over 500 characters.
        let long_id = format!("{}-{}", "a".repeat(64), "b".repeat(430));
        assert_eq!(
            render_package_url(&m, &long_id, "1.0.0").unwrap_err().code,
            ManifestProblemCode::PackageUrlInvalid
        );
        assert_eq!(
            render_package_url(&m, "", "1.0.0").unwrap_err().code,
            ManifestProblemCode::PackageUrlInvalid
        );
    }
}
