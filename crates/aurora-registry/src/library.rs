//! Skill-library fetch templates at runtime (09 §3.4 `library`, 06 §Library fetch; phase 4,
//! P4-GSL, D-149, SEC-84): parameter grammars, request building and response extractors.
//!
//! The block is **data**: three `GET` templates, a repository pattern and JSON-path extractors.
//! The gateway uses [`build`] to turn validated parameters into a request target (nothing else
//! can reach the request line); the API uses the extractors on what the gateway returned.
use base64::Engine;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use regex::Regex;
use serde_json::Value;

use crate::manifest::Library;

/// Longest repository or ref accepted.
pub const REPO_MAX: usize = 200;
pub const REF_MAX: usize = 200;
/// Largest page number the fetch route accepts.
pub const PAGE_MAX: u32 = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    ResolveRef,
    ListTree,
    GetBlob,
}

impl Step {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ResolveRef => "resolve_ref",
            Self::ListTree => "list_tree",
            Self::GetBlob => "get_blob",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Params {
    pub repo: String,
    pub reference: Option<String>,
    pub sha: Option<String>,
    pub blob: Option<String>,
    pub page: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("library parameter `{param}` rejected: {reason}")]
pub struct ParamError {
    pub param: &'static str,
    pub reason: &'static str,
}

fn bad(param: &'static str, reason: &'static str) -> ParamError {
    ParamError { param, reason }
}

fn safe_segments(s: &str, max: usize, param: &'static str) -> Result<(), ParamError> {
    if s.is_empty() || s.len() > max {
        return Err(bad(param, "length"));
    }
    if !s
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'-'))
    {
        return Err(bad(param, "characters"));
    }
    if s.starts_with('-') || s.starts_with('/') || s.ends_with('/') {
        return Err(bad(param, "shape"));
    }
    if s.split('/')
        .any(|seg| seg.is_empty() || seg == "." || seg == "..")
    {
        return Err(bad(param, "segments"));
    }
    Ok(())
}

/// A repository: matches the package's `repo_pattern` and the hard grammar (no empty, `.` or
/// `..` segment, only `[A-Za-z0-9._/-]`), whatever the pattern allows.
pub fn check_repo(lib: &Library, repo: &str) -> Result<(), ParamError> {
    safe_segments(repo, REPO_MAX, "repo")?;
    let re = Regex::new(&lib.repo_pattern).map_err(|_| bad("repo", "pattern"))?;
    if re.is_match(repo) {
        Ok(())
    } else {
        Err(bad("repo", "pattern"))
    }
}

/// A ref (branch, tag or sha): `[A-Za-z0-9._/-]{1,200}`, no `..`, no leading `-`, no `//`.
pub fn check_ref(r: &str) -> Result<(), ParamError> {
    safe_segments(r, REF_MAX, "ref")?;
    if r.contains("..") {
        return Err(bad("ref", "segments"));
    }
    Ok(())
}

/// A commit or blob id: lowercase hex, 40 to 64 characters.
pub fn check_hex_id(v: &str, param: &'static str) -> Result<(), ParamError> {
    if (40..=64).contains(&v.len()) && v.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        Ok(())
    } else {
        Err(bad(param, "shape"))
    }
}

const ENCODE: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.');

fn encode(s: &str) -> String {
    utf8_percent_encode(s, ENCODE).to_string()
}

/// Substitute the placeholders of `template` with already-checked values.
fn fill(template: &str, p: &Params) -> Result<String, ParamError> {
    let mut out = String::with_capacity(template.len() + 64);
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let Some(j) = rest[i..].find('}') else {
            return Err(bad("template", "unbalanced"));
        };
        let name = &rest[i + 1..i + j];
        rest = &rest[i + j + 1..];
        let value = match name {
            "repo" => p.repo.clone(),
            "repo|urlencode" => encode(&p.repo),
            "ref" => p.reference.clone().ok_or_else(|| bad("ref", "missing"))?,
            "ref|urlencode" => encode(
                p.reference
                    .as_deref()
                    .ok_or_else(|| bad("ref", "missing"))?,
            ),
            "sha" => p.sha.clone().ok_or_else(|| bad("sha", "missing"))?,
            "blob" => p.blob.clone().ok_or_else(|| bad("blob", "missing"))?,
            _ => return Err(bad("template", "placeholder")),
        };
        out.push_str(&value);
    }
    out.push_str(rest);
    Ok(out)
}

/// The request target (`/path?query`) of one step. Every parameter is validated first; the
/// result contains nothing but the template's literals and checked values (SEC-84).
pub fn build(lib: &Library, step: Step, p: &Params) -> Result<String, ParamError> {
    check_repo(lib, &p.repo)?;
    if let Some(r) = &p.reference {
        check_ref(r)?;
    }
    if let Some(s) = &p.sha {
        check_hex_id(s, "sha")?;
    }
    if let Some(b) = &p.blob {
        check_hex_id(b, "blob")?;
    }
    if let Some(n) = p.page
        && !(2..=PAGE_MAX).contains(&n)
    {
        return Err(bad("page", "range"));
    }
    let template = match step {
        Step::ResolveRef if p.reference.is_some() => &lib.resolve_ref.path,
        Step::ResolveRef => &lib.resolve_ref.default_path,
        Step::ListTree => &lib.list_tree.path,
        Step::GetBlob => &lib.get_blob.path,
    };
    let mut target = fill(template, p)?;
    if let Some(n) = p.page {
        target.push(if target.contains('?') { '&' } else { '?' });
        target.push_str(&format!("page={n}"));
    }
    Ok(target)
}

// ---- JSON paths -------------------------------------------------------------------------

/// `$`, then `.name` or `[*]` segments (no recursive descent, no filters, at most three
/// wildcards, at most eight segments).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonPath(Vec<Seg>);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Seg {
    Key(String),
    Each,
}

impl JsonPath {
    pub fn parse(s: &str) -> Result<Self, &'static str> {
        let mut rest = s.strip_prefix('$').ok_or("must start with `$`")?;
        let mut segs = Vec::new();
        while !rest.is_empty() {
            if let Some(r) = rest.strip_prefix("[*]") {
                segs.push(Seg::Each);
                rest = r;
            } else if let Some(r) = rest.strip_prefix('.') {
                let end = r.find(['.', '[']).unwrap_or(r.len());
                let name = &r[..end];
                if name.is_empty()
                    || name.len() > 64
                    || !name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
                {
                    return Err("bad member name (no recursive descent, no filters)");
                }
                segs.push(Seg::Key(name.to_owned()));
                rest = &r[end..];
            } else {
                return Err("only `.name` and `[*]` segments are allowed");
            }
        }
        if segs.len() > 8 {
            return Err("more than 8 segments");
        }
        if segs.iter().filter(|s| matches!(s, Seg::Each)).count() > 3 {
            return Err("more than 3 wildcards");
        }
        Ok(Self(segs))
    }

    #[must_use]
    pub fn is_wildcard(&self) -> bool {
        self.0.iter().any(|s| matches!(s, Seg::Each))
    }

    /// Every value the path selects.
    #[must_use]
    pub fn select<'a>(&self, root: &'a Value) -> Vec<&'a Value> {
        let mut cur = vec![root];
        for seg in &self.0 {
            let mut next = Vec::new();
            for v in cur {
                match seg {
                    Seg::Key(k) => {
                        if let Some(c) = v.get(k) {
                            next.push(c);
                        }
                    }
                    Seg::Each => {
                        if let Some(a) = v.as_array() {
                            next.extend(a.iter());
                        }
                    }
                }
            }
            cur = next;
        }
        cur
    }

    #[must_use]
    pub fn first<'a>(&self, root: &'a Value) -> Option<&'a Value> {
        self.select(root).into_iter().next()
    }
}

fn path(s: &str) -> Result<JsonPath, ExtractError> {
    JsonPath::parse(s).map_err(|_| ExtractError::Shape("json path"))
}

fn text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExtractError {
    #[error("unexpected response shape: {0}")]
    Shape(&'static str),
    #[error("response is not valid JSON")]
    NotJson,
    #[error("the commit id in the response is not a valid id")]
    BadSha,
    #[error("the default branch in the response is not a valid ref")]
    BadBranch,
    #[error("the file content could not be decoded")]
    BadContent,
}

fn json(body: &[u8]) -> Result<Value, ExtractError> {
    serde_json::from_slice(body).map_err(|_| ExtractError::NotJson)
}

/// The commit sha of a `resolve_ref` response (the `path` template).
pub fn resolved_sha(lib: &Library, body: &[u8]) -> Result<String, ExtractError> {
    let v = json(body)?;
    let sha = path(&lib.resolve_ref.sha)?
        .first(&v)
        .and_then(text)
        .ok_or(ExtractError::Shape("commit sha"))?;
    check_hex_id(&sha, "sha").map_err(|_| ExtractError::BadSha)?;
    Ok(sha)
}

/// The default branch of a `default_path` response.
pub fn default_branch(lib: &Library, body: &[u8]) -> Result<String, ExtractError> {
    let v = json(body)?;
    let b = path(&lib.resolve_ref.default_branch)?
        .first(&v)
        .and_then(text)
        .ok_or(ExtractError::Shape("default branch"))?;
    check_ref(&b).map_err(|_| ExtractError::BadBranch)?;
    Ok(b)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    pub path: String,
    /// The entry's `type` field (`blob`, `tree`, `commit`).
    pub kind: String,
    pub mode: String,
    pub blob: String,
    pub size: Option<u64>,
}

impl TreeEntry {
    #[must_use]
    pub fn is_file_kind(&self, lib: &Library) -> bool {
        self.kind == lib.list_tree.file_type
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreePage {
    pub entries: Vec<TreeEntry>,
    pub truncated: bool,
}

/// One page of a `list_tree` response. Entries without a path or an id are dropped (a
/// directory entry has an id, a submodule's is the pinned commit).
pub fn tree_page(lib: &Library, body: &[u8]) -> Result<TreePage, ExtractError> {
    let v = json(body)?;
    let f = &lib.list_tree.fields;
    let (pp, kp, mp, bp) = (
        path(&f.path)?,
        path(&f.kind)?,
        path(&f.mode)?,
        path(&f.blob)?,
    );
    let sp = f.size.as_deref().map(path).transpose()?;
    let mut out = TreePage::default();
    for e in path(&lib.list_tree.entries)?.select(&v) {
        let (Some(p), Some(kind), Some(blob)) = (
            pp.first(e).and_then(text),
            kp.first(e).and_then(text),
            bp.first(e).and_then(text),
        ) else {
            continue;
        };
        out.entries.push(TreeEntry {
            path: p,
            kind,
            mode: mp.first(e).and_then(text).unwrap_or_default(),
            blob,
            size: sp.as_ref().and_then(|s| s.first(e)).and_then(Value::as_u64),
        });
    }
    if let Some(t) = &lib.list_tree.truncated {
        out.truncated = path(t)?.first(&v).and_then(Value::as_bool).unwrap_or(false);
    }
    Ok(out)
}

/// The page number of the `rel="next"` entry of a `Link` header, if any. Only the integer
/// `page` query value is read; the URL itself is never followed (SEC-84).
#[must_use]
pub fn next_page(link: &str) -> Option<u32> {
    for part in link.split(',') {
        let mut bits = part.split(';');
        let url = bits
            .next()?
            .trim()
            .trim_start_matches('<')
            .trim_end_matches('>');
        if !bits.any(|b| {
            let b = b.trim().replace(' ', "");
            b == "rel=\"next\"" || b == "rel=next"
        }) {
            continue;
        }
        let query = url.split_once('?')?.1;
        for kv in query.split('&') {
            if let Some(n) = kv.strip_prefix("page=") {
                return n.parse().ok();
            }
        }
    }
    None
}

/// The bytes of a `get_blob` response.
pub fn blob_bytes(lib: &Library, body: &[u8]) -> Result<Vec<u8>, ExtractError> {
    let enc = lib.get_blob.encoding.as_str();
    if enc == "raw" {
        return Ok(body.to_vec());
    }
    let v = json(body)?;
    let content = lib
        .get_blob
        .content
        .as_deref()
        .map(path)
        .transpose()?
        .and_then(|p| p.first(&v).and_then(Value::as_str).map(str::to_owned))
        .ok_or(ExtractError::Shape("content"))?;
    let encoding = if enc.starts_with('$') {
        path(enc)?
            .first(&v)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or(ExtractError::Shape("encoding"))?
    } else {
        enc.to_owned()
    };
    match encoding.as_str() {
        "base64" => {
            let clean: String = content.chars().filter(|c| !c.is_whitespace()).collect();
            base64::engine::general_purpose::STANDARD
                .decode(clean)
                .map_err(|_| ExtractError::BadContent)
        }
        "utf-8" | "utf8" | "raw" => Ok(content.into_bytes()),
        _ => Err(ExtractError::BadContent),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn json_paths() {
        assert!(JsonPath::parse("$.tree[*].path").is_ok());
        assert!(JsonPath::parse("$[*]").is_ok());
        for bad in [
            "tree",
            "$..a",
            "$.a[?(@.b)]",
            "$.a[0]",
            "$.[*]",
            "$.a[*][*][*][*]",
            "$.a b",
        ] {
            assert!(JsonPath::parse(bad).is_err(), "{bad}");
        }
        let v: Value = serde_json::from_str(r#"{"a":[{"b":1},{"b":2}]}"#).unwrap();
        let sel = JsonPath::parse("$.a[*].b").unwrap().select(&v);
        assert_eq!(sel.len(), 2);
    }

    #[test]
    fn next_link() {
        let l = "<https://gitlab.example/api/v4/x?page=3&per_page=100>; rel=\"next\", <https://gitlab.example/api/v4/x?page=9>; rel=\"last\"";
        assert_eq!(next_page(l), Some(3));
        assert_eq!(next_page("<https://x/y?page=2>; rel=\"prev\""), None);
        assert_eq!(next_page("garbage"), None);
    }
}
