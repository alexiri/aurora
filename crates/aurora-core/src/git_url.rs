//! Repository addresses of git skill libraries (phase 5, P5-SKLIB-1, D-448(a), C-1123).
//!
//! One parser used by create, patch and check; the dashboard never parses. Pure: no I/O. The
//! destination guard runs again when the reader connects (an IP-literal host is only *reported*
//! here, [`GitAddress::ip_literal`], so the caller can check it against `gateway.allowed_private`).
//!
//! Accepted: `https://<host>[:port]/<path>` with 1..20 segments of `[A-Za-z0-9._~-]` (percent
//! escapes decoded once, then the same charset), at most 500 characters. A repository's page URL
//! (`…/-/tree/<ref>/<dir>`, `…/tree/<ref>/<dir>`, `…/src/branch|tag|commit/<ref>/<dir>`,
//! `…/src/<ref>/<dir>`) gives the repository plus a ref and folder hint.

use std::net::IpAddr;

/// The longest address accepted (characters of the trimmed input).
pub const URL_MAX: usize = 500;
/// The most path segments of the repository part.
pub const SEGMENTS_MAX: usize = 20;

/// A normalized repository address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitAddress {
    /// The stored clone URL: `https://`, lowercased host, the port unless 443, the repository
    /// path without a trailing `/` (a trailing `.git` kept when given, never added).
    pub url: String,
    /// `host[:port]/path` without `.git` (what the dashboard shows and `skills.library_repo`
    /// stores).
    pub repo_display: String,
    /// The first segment after a page URL's marker (`tree`, `src/branch`, …).
    pub ref_hint: Option<String>,
    /// The folder after the ref in a page URL (`a/b`), unvalidated beyond the charset.
    pub path_hint: Option<String>,
}

impl GitAddress {
    /// The host when it is an IP literal (the caller checks it against the destination guard).
    #[must_use]
    pub fn ip_literal(&self) -> Option<IpAddr> {
        let rest = self.url.strip_prefix("https://")?;
        let authority = rest.split('/').next()?;
        let host = if let Some(v6) = authority.strip_prefix('[') {
            v6.split(']').next()?
        } else {
            authority.rsplit_once(':').map_or(authority, |(h, _)| h)
        };
        host.parse().ok()
    }

    /// The last repository segment without `.git` (the suggested label's source).
    #[must_use]
    pub fn last_segment(&self) -> &str {
        self.repo_display.rsplit('/').next().unwrap_or("")
    }
}

/// Why an address was refused (`details.reason` of 422 `library_url_invalid`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlError {
    /// Another scheme than `https` (`http:`, `git:`, `file:`, none at all).
    NotHttps,
    /// `ssh://…` or the scp-like `git@host:path`.
    SshUrl,
    /// A user name or password in the address.
    CredentialsInUrl,
    /// A `?query` or `#fragment`.
    QueryOrFragment,
    /// No repository path, an unusable host or segment, or too many segments.
    NotARepositoryPath,
    /// Longer than [`URL_MAX`].
    TooLong,
}

impl UrlError {
    #[must_use]
    pub fn reason(self) -> &'static str {
        match self {
            Self::NotHttps => "not_https",
            Self::SshUrl => "ssh_url",
            Self::CredentialsInUrl => "credentials_in_url",
            Self::QueryOrFragment => "query_or_fragment",
            Self::NotARepositoryPath => "not_a_repository_path",
            Self::TooLong => "too_long",
        }
    }

    /// A sentence for the error message (fixed text).
    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            Self::NotHttps => "Use the repository's https address.",
            Self::SshUrl => "SSH addresses are not supported; use the repository's https address.",
            Self::CredentialsInUrl => {
                "Remove the user name or password from the address; use the access token field."
            }
            Self::QueryOrFragment => "Remove the part after ? or # from the address.",
            Self::NotARepositoryPath => "This is not a repository address.",
            Self::TooLong => "The address is longer than 500 characters.",
        }
    }
}

impl std::fmt::Display for UrlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason())
    }
}

impl std::error::Error for UrlError {}

fn segment_ok(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'-'))
}

/// Decodes `%XX` once; a malformed escape or a non-UTF-8 result is `None`.
fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Whether `s` looks like the scp-like ssh form `user@host:path`.
fn scp_like(s: &str) -> bool {
    let Some((user_host, path)) = s.split_once(':') else {
        return false;
    };
    !path.starts_with("//") && user_host.contains('@') && !user_host.contains('/')
}

/// Parses and normalizes a repository address (C-1123).
pub fn parse(input: &str) -> Result<GitAddress, UrlError> {
    let s = input.trim();
    if s.chars().count() > URL_MAX {
        return Err(UrlError::TooLong);
    }
    let Some((scheme, rest)) = s.split_once("://") else {
        return Err(if scp_like(s) {
            UrlError::SshUrl
        } else {
            UrlError::NotHttps
        });
    };
    match scheme.to_ascii_lowercase().as_str() {
        "https" => {}
        "ssh" | "git+ssh" | "ssh+git" => return Err(UrlError::SshUrl),
        _ => return Err(UrlError::NotHttps),
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    if authority.contains('@') {
        return Err(UrlError::CredentialsInUrl);
    }
    if tail.contains(['?', '#']) {
        return Err(UrlError::QueryOrFragment);
    }
    if authority.is_empty() || authority.contains(['\\', ' ', '%']) {
        return Err(UrlError::NotARepositoryPath);
    }
    // The host through the URL standard (IDNA, IPv4/IPv6 forms, port range).
    let base = url::Url::parse(&format!("https://{authority}/"))
        .map_err(|_| UrlError::NotARepositoryPath)?;
    let host = match base.host() {
        Some(url::Host::Domain(d)) => d.to_ascii_lowercase(),
        Some(url::Host::Ipv4(ip)) => ip.to_string(),
        Some(url::Host::Ipv6(ip)) => format!("[{ip}]"),
        None => return Err(UrlError::NotARepositoryPath),
    };
    if host.is_empty() || host.starts_with('.') || host.contains("..") {
        return Err(UrlError::NotARepositoryPath);
    }
    let origin = match base.port() {
        Some(p) => format!("{host}:{p}"),
        None => host,
    };
    let path = tail.strip_prefix('/').unwrap_or(tail);
    let path = path.strip_suffix('/').unwrap_or(path);
    if path.is_empty() {
        return Err(UrlError::NotARepositoryPath);
    }
    let mut segs = Vec::new();
    for raw in path.split('/') {
        let d = percent_decode(raw).ok_or(UrlError::NotARepositoryPath)?;
        if !segment_ok(&d) {
            return Err(UrlError::NotARepositoryPath);
        }
        segs.push(d);
    }
    let (repo, ref_hint, dir): (&[String], Option<&String>, &[String]) =
        if let Some(i) = segs.iter().position(|s| s == "-") {
            // GitLab: `<repo>/-/tree|blob/<ref>[/<dir>]`.
            match (segs.get(i + 1).map(String::as_str), segs.get(i + 2)) {
                (Some("tree" | "blob"), Some(r)) if i >= 1 => (&segs[..i], Some(r), &segs[i + 3..]),
                _ => return Err(UrlError::NotARepositoryPath),
            }
        } else if segs.len() >= 4 && segs[2] == "tree" {
            (&segs[..2], Some(&segs[3]), &segs[4..])
        } else if segs.len() >= 4 && segs[2] == "src" {
            if segs.len() >= 5 && matches!(segs[3].as_str(), "branch" | "tag" | "commit") {
                (&segs[..2], Some(&segs[4]), &segs[5..])
            } else {
                (&segs[..2], Some(&segs[3]), &segs[4..])
            }
        } else {
            (&segs[..], None, &[][..])
        };
    if repo.is_empty() || repo.len() > SEGMENTS_MAX {
        return Err(UrlError::NotARepositoryPath);
    }
    let repo_path = repo.join("/");
    let display_path = repo_path.strip_suffix(".git").unwrap_or(&repo_path);
    if display_path.is_empty() || display_path.ends_with('/') {
        return Err(UrlError::NotARepositoryPath);
    }
    Ok(GitAddress {
        url: format!("https://{origin}/{repo_path}"),
        repo_display: format!("{origin}/{display_path}"),
        ref_hint: ref_hint.cloned(),
        path_hint: (!dir.is_empty()).then(|| dir.join("/")),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn ok(input: &str, url: &str, display: &str, r: Option<&str>, p: Option<&str>) {
        let a = parse(input).unwrap_or_else(|e| panic!("{input}: {e}"));
        assert_eq!(a.url, url, "{input}");
        assert_eq!(a.repo_display, display, "{input}");
        assert_eq!(a.ref_hint.as_deref(), r, "{input}");
        assert_eq!(a.path_hint.as_deref(), p, "{input}");
    }

    fn err(input: &str, e: UrlError) {
        assert_eq!(parse(input), Err(e), "{input}");
    }

    #[test]
    fn accepted_addresses_and_page_urls() {
        // GitHub
        ok(
            "https://github.com/acme/skills",
            "https://github.com/acme/skills",
            "github.com/acme/skills",
            None,
            None,
        );
        ok(
            "https://github.com/acme/skills.git",
            "https://github.com/acme/skills.git",
            "github.com/acme/skills",
            None,
            None,
        );
        ok(
            "  https://GitHub.com/acme/skills/  ",
            "https://github.com/acme/skills",
            "github.com/acme/skills",
            None,
            None,
        );
        ok(
            "https://github.com/acme/skills/tree/main",
            "https://github.com/acme/skills",
            "github.com/acme/skills",
            Some("main"),
            None,
        );
        ok(
            "https://github.com/acme/skills/tree/v1.2/team/ops",
            "https://github.com/acme/skills",
            "github.com/acme/skills",
            Some("v1.2"),
            Some("team/ops"),
        );
        ok(
            "HTTPS://github.com:443/acme/skills",
            "https://github.com/acme/skills",
            "github.com/acme/skills",
            None,
            None,
        );
        // GitLab with subgroups, gitlab.cern.ch
        ok(
            "https://gitlab.com/group/sub/proj",
            "https://gitlab.com/group/sub/proj",
            "gitlab.com/group/sub/proj",
            None,
            None,
        );
        ok(
            "https://gitlab.com/group/sub/proj/-/tree/main/skills",
            "https://gitlab.com/group/sub/proj",
            "gitlab.com/group/sub/proj",
            Some("main"),
            Some("skills"),
        );
        ok(
            "https://gitlab.cern.ch/it/agents/skills.git",
            "https://gitlab.cern.ch/it/agents/skills.git",
            "gitlab.cern.ch/it/agents/skills",
            None,
            None,
        );
        ok(
            "https://gitlab.cern.ch/it/agents/skills/-/blob/dev/a/SKILL.md",
            "https://gitlab.cern.ch/it/agents/skills",
            "gitlab.cern.ch/it/agents/skills",
            Some("dev"),
            Some("a/SKILL.md"),
        );
        ok(
            "https://gitlab.cern.ch/it/agents/skills/-/tree/main",
            "https://gitlab.cern.ch/it/agents/skills",
            "gitlab.cern.ch/it/agents/skills",
            Some("main"),
            None,
        );
        // Gitea, Forgejo, Codeberg
        ok(
            "https://gitea.com/owner/repo/src/branch/main/skills",
            "https://gitea.com/owner/repo",
            "gitea.com/owner/repo",
            Some("main"),
            Some("skills"),
        );
        ok(
            "https://codeberg.org/owner/repo/src/tag/v2",
            "https://codeberg.org/owner/repo",
            "codeberg.org/owner/repo",
            Some("v2"),
            None,
        );
        ok(
            "https://codeberg.org/owner/repo/src/commit/0123456789abcdef0123456789abcdef01234567/x",
            "https://codeberg.org/owner/repo",
            "codeberg.org/owner/repo",
            Some("0123456789abcdef0123456789abcdef01234567"),
            Some("x"),
        );
        ok(
            "https://codeberg.org/owner/repo.git",
            "https://codeberg.org/owner/repo.git",
            "codeberg.org/owner/repo",
            None,
            None,
        );
        // Bitbucket
        ok(
            "https://bitbucket.org/team/repo/src/main/skills/",
            "https://bitbucket.org/team/repo",
            "bitbucket.org/team/repo",
            Some("main"),
            Some("skills"),
        );
        // A bare self-hosted server, a port, one segment, percent escapes, IDNA
        ok(
            "https://git.example.org/team/skills.git",
            "https://git.example.org/team/skills.git",
            "git.example.org/team/skills",
            None,
            None,
        );
        ok(
            "https://git.example.org:8443/skills",
            "https://git.example.org:8443/skills",
            "git.example.org:8443/skills",
            None,
            None,
        );
        ok(
            "https://git.example.org/team%2Dx/skills",
            "https://git.example.org/team-x/skills",
            "git.example.org/team-x/skills",
            None,
            None,
        );
        ok(
            "https://bücher.example/a/b",
            "https://xn--bcher-kva.example/a/b",
            "xn--bcher-kva.example/a/b",
            None,
            None,
        );
        let ip = parse("https://10.1.2.3/a/b").unwrap();
        assert_eq!(ip.ip_literal(), Some("10.1.2.3".parse().unwrap()));
        let ip6 = parse("https://[fd00::1]:8443/a").unwrap();
        assert_eq!(ip6.url, "https://[fd00::1]:8443/a");
        assert_eq!(ip6.ip_literal(), Some("fd00::1".parse().unwrap()));
        assert_eq!(parse("https://git.example/a").unwrap().ip_literal(), None);
        assert_eq!(parse("https://g.example/a/skills.git").unwrap().last_segment(), "skills");
    }

    #[test]
    fn refused_forms_each_give_their_reason() {
        err("http://github.com/acme/skills", UrlError::NotHttps);
        err("git://github.com/acme/skills", UrlError::NotHttps);
        err("file:///srv/git/skills", UrlError::NotHttps);
        err("ftp://example.org/x", UrlError::NotHttps);
        err("github.com/acme/skills", UrlError::NotHttps);
        err("", UrlError::NotHttps);
        err("ssh://git@github.com/acme/skills.git", UrlError::SshUrl);
        err("git+ssh://github.com/acme/skills", UrlError::SshUrl);
        err("git@github.com:acme/skills.git", UrlError::SshUrl);
        err("https://user:pw@github.com/acme/skills", UrlError::CredentialsInUrl);
        err("https://token@github.com/acme/skills", UrlError::CredentialsInUrl);
        err("https://github.com/acme/skills?ref=main", UrlError::QueryOrFragment);
        err("https://github.com/acme/skills#readme", UrlError::QueryOrFragment);
        err("https://github.com?x", UrlError::QueryOrFragment);
        err("https://github.com", UrlError::NotARepositoryPath);
        err("https://github.com/", UrlError::NotARepositoryPath);
        err("https://github.com//acme", UrlError::NotARepositoryPath);
        err("https://github.com/acme/../etc", UrlError::NotARepositoryPath);
        err("https://github.com/acme/%2e%2e/x", UrlError::NotARepositoryPath);
        err("https://github.com/acme/a%2Fb", UrlError::NotARepositoryPath);
        err("https://github.com/acme/sk ills", UrlError::NotARepositoryPath);
        err("https://github.com/acme/%zz", UrlError::NotARepositoryPath);
        err("https://github.com/acme/ski\u{0430}lls", UrlError::NotARepositoryPath);
        err("https://gitlab.com/g/p/-/merge_requests", UrlError::NotARepositoryPath);
        err("https://gitlab.com/-/tree/main", UrlError::NotARepositoryPath);
        err("https://host:99999/a", UrlError::NotARepositoryPath);
        err("https:///a/b", UrlError::NotARepositoryPath);
        err("https://.git", UrlError::NotARepositoryPath);
        err("https://g.example/.git", UrlError::NotARepositoryPath);
        let deep = format!("https://g.example/{}", vec!["a"; 21].join("/"));
        err(&deep, UrlError::NotARepositoryPath);
        let long = format!("https://g.example/{}", "a".repeat(495));
        err(&long, UrlError::TooLong);
        assert!(parse(&format!("https://g.example/{}", vec!["a"; 20].join("/"))).is_ok());
        for e in [
            UrlError::NotHttps,
            UrlError::SshUrl,
            UrlError::CredentialsInUrl,
            UrlError::QueryOrFragment,
            UrlError::NotARepositoryPath,
            UrlError::TooLong,
        ] {
            assert!(!e.message().is_empty());
        }
    }
}
