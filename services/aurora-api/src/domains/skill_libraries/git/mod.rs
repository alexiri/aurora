//! The API's own git reader (phase 5, P5-SKLIB-1, D-448(b,c), C-1125, SEC-148).
//!
//! Reads a repository with the git smart-HTTP protocol (v2, else v0) on the API's
//! `http_instance` client (the destination guard at dial time; `gateway.allowed_private` is the
//! only way to a private address). No `git` binary, no libgit2, no checkout: refs, a depth-1
//! commit with its trees (`filter blob:none` when offered) and the blobs a read needs, parsed in
//! memory under the caps of C-1125 with every object id recomputed.
//!
//! The requests are built only from the stored, normalized URL (C-1123). Redirects are followed
//! only on discovery (at most 3, each https and guarded). The token is sent as HTTP Basic only to
//! the library URL's origin; nothing here logs, returns or stores it, and no reason text carries
//! a URL, a token or server text.

pub mod discover;
pub mod pack;
pub mod pktline;

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use aurora_core::Secret;
use aurora_core::limits;
use reqwest::header::{ACCEPT, CONTENT_TYPE, LOCATION, RETRY_AFTER, USER_AGENT};

use super::content::{ReadError, RefKind, RepoSource, Resolved, TreeEntry, throttle_secs};
use crate::domains::connect::env::{Env, classify_send_error};
use discover::{Discovery, FetchAsk, ProtoError};
use pack::{Kind, Object, Oid};

/// Fixed, sanitized reasons (C-1125; P5-SKLIB-1 §Part B 5). No URL, token or server text.
pub mod reason {
    pub const NEEDS_TOKEN: &str = "The repository needs an access token";
    pub const TOKEN_REFUSED: &str = "The access token was refused";
    pub const NOT_FOUND: &str = "The repository was not found, or the token cannot read it";
    pub const REF_NOT_FOUND: &str = "The branch or tag was not found";
    pub const ENTER_URL: &str = "Enter the repository's git address";
    pub const SHA256: &str = "SHA-256 repositories are not supported";
    pub const MOVED: &str = "the git host answered with a redirect (the repository may have moved)";
    pub const REDIRECT_REFUSED: &str = "The repository redirected to an address that is not allowed";
    pub const TOO_MANY_REDIRECTS: &str = "The repository redirected too many times";
    pub const NOT_GIT: &str = "The server does not answer as a git repository";
    pub const PROTOCOL: &str = "The git server's answer could not be read";
    pub const MISMATCH: &str = "The repository sent objects that do not match what was asked";
    pub const TOO_LARGE: &str = "The repository's answer is larger than 32 MiB";
    pub const ADDRESS_REFUSED: &str = "The repository's address is not allowed";
    pub const HOST_NOT_FOUND: &str = "The repository's host name was not found";
    pub const TIMEOUT: &str = "The git host did not answer in time";
    pub const TLS: &str = "The git host's certificate was not accepted";
    pub const UNREACHABLE: &str = "The git host could not be reached";
}

/// A library's access token as the reader uses it (opened once per read, never kept).
pub struct Token {
    pub username: String,
    pub token: Secret<String>,
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token(..)")
    }
}

const SERVICE: &str = "/info/refs?service=git-upload-pack";
const MAX_REDIRECTS: usize = 3;

fn unreachable(r: &str) -> ReadError {
    ReadError::Unreachable(r.to_owned())
}

fn proto(e: ProtoError) -> ReadError {
    match e {
        ProtoError::Sha256 => unreachable(reason::SHA256),
        ProtoError::Malformed | ProtoError::Refused => unreachable(reason::PROTOCOL),
    }
}

fn pack_err(e: pack::PackError) -> ReadError {
    match e {
        pack::PackError::Malformed(_) => unreachable(reason::PROTOCOL),
        pack::PackError::TooLarge(r) => unreachable(r),
    }
}

fn agent() -> String {
    format!("aurora-skill-library/{}", env!("CARGO_PKG_VERSION"))
}

#[derive(Default)]
struct State {
    /// Base URL after discovery redirects (this read only; never stored).
    base: Option<String>,
    disc: Option<Discovery>,
    /// Every object received in this read, by its recomputed id.
    objects: HashMap<Oid, Object>,
    /// The commit whose trees were fetched (the whole-pack fallback for blobs).
    commit: Option<Oid>,
}

/// One read of one repository.
pub struct GitRepo<'a> {
    env: &'a Env,
    url: String,
    origin: url::Origin,
    token: Option<&'a Token>,
    timeout: Duration,
    state: tokio::sync::Mutex<State>,
}

impl<'a> GitRepo<'a> {
    /// `url` is a stored, normalized address (C-1123).
    pub fn new(
        env: &'a Env,
        url: &str,
        token: Option<&'a Token>,
        timeout: Duration,
    ) -> Result<Self, ReadError> {
        let parsed = url::Url::parse(url).map_err(|_| unreachable(reason::ENTER_URL))?;
        if parsed.scheme() != "https" {
            return Err(unreachable(reason::ENTER_URL));
        }
        Ok(Self {
            env,
            url: url.trim_end_matches('/').to_owned(),
            origin: parsed.origin(),
            token,
            timeout,
            state: tokio::sync::Mutex::new(State::default()),
        })
    }

    /// A redirect target or the library URL itself: https, no credentials, an IP literal only
    /// where the destination guard allows it (names are checked by the resolver at dial time).
    fn allowed(&self, u: &url::Url) -> bool {
        if u.scheme() != "https" || !u.username().is_empty() || u.password().is_some() {
            return false;
        }
        let Some(host) = u.host_str() else {
            return false;
        };
        match host.trim_matches(['[', ']']).parse::<std::net::IpAddr>() {
            Ok(ip) => self.env.allow.load().permits(ip, host),
            Err(_) => true,
        }
    }

    /// The address actually dialled: the logical one, or the test override's origin.
    fn dial(&self, logical: &url::Url) -> url::Url {
        match &self.env.override_origin {
            Some(o) => {
                let mut t = o.clone();
                t.set_path(logical.path());
                t.set_query(logical.query());
                t
            }
            None => logical.clone(),
        }
    }

    async fn send(
        &self,
        logical: &str,
        body: Option<Vec<u8>>,
        v2: bool,
    ) -> Result<(reqwest::Response, bool), ReadError> {
        let u = url::Url::parse(logical).map_err(|_| unreachable(reason::PROTOCOL))?;
        if !self.allowed(&u) {
            return Err(unreachable(reason::ADDRESS_REFUSED));
        }
        let http = &self.env.http_instance;
        let target = self.dial(&u);
        let mut rb = match &body {
            Some(_) => http
                .post(target)
                .header(CONTENT_TYPE, "application/x-git-upload-pack-request")
                .header(ACCEPT, "application/x-git-upload-pack-result"),
            None => http.get(target),
        };
        rb = rb
            .timeout(self.timeout)
            .header(USER_AGENT, format!("{} git/2.0", agent()));
        if v2 {
            rb = rb.header("Git-Protocol", "version=2");
        }
        // (SEC-148) Basic auth only to the library URL's own origin.
        let send_token = self.token.filter(|_| u.origin() == self.origin);
        if let Some(t) = send_token {
            rb = rb.basic_auth(&t.username, Some(t.token.expose()));
        }
        if let Some(b) = body {
            rb = rb.body(b);
        }
        let resp = rb.send().await.map_err(|e| {
            unreachable(match classify_send_error(&e) {
                "destination_refused" => reason::ADDRESS_REFUSED,
                "dns" => reason::HOST_NOT_FOUND,
                "timeout" => reason::TIMEOUT,
                "tls" => reason::TLS,
                _ => reason::UNREACHABLE,
            })
        })?;
        Ok((resp, send_token.is_some()))
    }

    fn status(&self, resp: &reqwest::Response, sent_token: bool) -> Result<(), ReadError> {
        let s = resp.status().as_u16();
        match s {
            200..=299 => Ok(()),
            401 | 403 if sent_token => Err(unreachable(reason::TOKEN_REFUSED)),
            401 | 403 => Err(unreachable(reason::NEEDS_TOKEN)),
            404 | 410 => Err(unreachable(reason::NOT_FOUND)),
            429 | 503 => Err(ReadError::Throttled(throttle_secs(
                resp.headers()
                    .get(RETRY_AFTER)
                    .and_then(|v| v.to_str().ok()),
            ))),
            300..=399 => Err(unreachable(reason::MOVED)),
            s => Err(ReadError::Unreachable(format!(
                "the git host answered with status {s}"
            ))),
        }
    }

    /// The body, read chunk by chunk and stopped at `LIBRARY_PACK_MAX`.
    async fn body(&self, mut resp: reqwest::Response) -> Result<Vec<u8>, ReadError> {
        let cap = limits::LIBRARY_PACK_MAX;
        if resp.content_length().is_some_and(|n| n > cap as u64) {
            return Err(unreachable(reason::TOO_LARGE));
        }
        let mut out = Vec::new();
        loop {
            let chunk = resp.chunk().await.map_err(|e| {
                unreachable(if e.is_timeout() {
                    reason::TIMEOUT
                } else {
                    reason::UNREACHABLE
                })
            })?;
            let Some(c) = chunk else { break };
            if out.len() + c.len() > cap {
                return Err(unreachable(reason::TOO_LARGE));
            }
            out.extend_from_slice(&c);
        }
        Ok(out)
    }

    /// Discovery (cached for the read): the base URL after redirects and what the server speaks.
    async fn discovery(&self) -> Result<(String, Discovery), ReadError> {
        {
            let st = self.state.lock().await;
            if let (Some(b), Some(d)) = (&st.base, &st.disc) {
                return Ok((b.clone(), d.clone()));
            }
        }
        let mut cur = format!("{}{SERVICE}", self.url);
        for hop in 0..=MAX_REDIRECTS {
            let (resp, sent) = self.send(&cur, None, true).await?;
            if resp.status().is_redirection() {
                if hop == MAX_REDIRECTS {
                    return Err(unreachable(reason::TOO_MANY_REDIRECTS));
                }
                let loc = resp
                    .headers()
                    .get(LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| unreachable(reason::MOVED))?;
                let next = url::Url::parse(&cur)
                    .and_then(|c| c.join(loc))
                    .map_err(|_| unreachable(reason::REDIRECT_REFUSED))?;
                if !self.allowed(&next) || !next.as_str().ends_with(SERVICE) {
                    return Err(unreachable(reason::REDIRECT_REFUSED));
                }
                cur = next.to_string();
                continue;
            }
            self.status(&resp, sent)?;
            let ct = resp
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_owned();
            if !ct.starts_with("application/x-git-upload-pack-advertisement") {
                return Err(unreachable(reason::NOT_GIT));
            }
            let body = self.body(resp).await?;
            let disc = discover::parse_discovery(&body).map_err(|e| match e {
                ProtoError::Sha256 => unreachable(reason::SHA256),
                _ => unreachable(reason::NOT_GIT),
            })?;
            let base = cur
                .strip_suffix(SERVICE)
                .ok_or_else(|| unreachable(reason::MOVED))?
                .to_owned();
            let mut st = self.state.lock().await;
            st.base = Some(base.clone());
            st.disc = Some(disc.clone());
            return Ok((base, disc));
        }
        Err(unreachable(reason::TOO_MANY_REDIRECTS))
    }

    /// One POST to `git-upload-pack` (never follows a redirect).
    async fn upload_pack(&self, body: Vec<u8>) -> Result<Vec<u8>, ReadError> {
        let (base, disc) = self.discovery().await?;
        let v2 = matches!(disc, Discovery::V2 { .. });
        let (resp, sent) = self
            .send(&format!("{base}/git-upload-pack"), Some(body), v2)
            .await?;
        self.status(&resp, sent)?;
        self.body(resp).await
    }

    /// Fetches a pack for `wants`; every object goes into the read's store.
    async fn fetch(&self, wants: &[String], deepen: bool, filter: bool) -> Result<(), ReadError> {
        let (_, disc) = self.discovery().await?;
        let caps = match &disc {
            Discovery::V2 { caps, .. } => caps.clone(),
            Discovery::V0(a) => a.caps.clone(),
        };
        let ask = FetchAsk {
            wants,
            deepen: deepen && caps.shallow,
            filter: filter && caps.filter,
        };
        let req = match disc {
            Discovery::V2 { .. } => discover::fetch_v2(&agent(), &ask),
            Discovery::V0(_) => discover::fetch_v0(&agent(), &ask),
        };
        let raw = self.upload_pack(req).await?;
        let bytes = match discover::demux_pack(&raw) {
            Ok(b) => b,
            Err(ProtoError::Refused) => return Err(ReadError::Unreachable(REFUSED.into())),
            Err(e) => return Err(proto(e)),
        };
        let objects = pack::parse(&bytes, pack::Limits::standard()).map_err(pack_err)?;
        let mut st = self.state.lock().await;
        st.objects.extend(objects);
        Ok(())
    }

    async fn refs(&self, prefixes: &[String]) -> Result<Vec<discover::RefLine>, ReadError> {
        let (_, disc) = self.discovery().await?;
        match disc {
            Discovery::V2 { ls_refs: true, .. } => {
                let raw = self
                    .upload_pack(discover::ls_refs_request(&agent(), prefixes))
                    .await?;
                discover::parse_ls_refs(&raw).map_err(proto)
            }
            Discovery::V2 { .. } => Err(unreachable(reason::PROTOCOL)),
            Discovery::V0(a) => {
                let mut out = Vec::new();
                for (name, id) in &a.refs {
                    out.push(discover::RefLine {
                        name: name.clone(),
                        id: *id,
                        symref_target: (name == "HEAD")
                            .then(|| a.head_target.clone())
                            .flatten(),
                        peeled: a.peeled.get(name).copied(),
                    });
                }
                Ok(out)
            }
        }
    }
}

/// Internal marker: the server refused what was wanted (an `ERR` answer, e.g. "not our ref").
const REFUSED: &str = "\u{0}refused";

fn walk(
    objects: &HashMap<Oid, Object>,
    commit: &Oid,
    path: &str,
) -> Result<Vec<TreeEntry>, ReadError> {
    let mismatch = || unreachable(reason::MISMATCH);
    let c = objects.get(commit).ok_or_else(mismatch)?;
    if c.kind != Kind::Commit {
        return Err(unreachable(reason::REF_NOT_FOUND));
    }
    let tree_of = |id: &Oid| -> Result<Vec<pack::RawTreeEntry>, ReadError> {
        let o = objects.get(id).ok_or_else(mismatch)?;
        if o.kind != Kind::Tree {
            return Err(mismatch());
        }
        pack::tree_entries(&o.data).map_err(pack_err)
    };
    let mut root = pack::commit_tree(&c.data).map_err(pack_err)?;
    let prefix = path.trim_matches('/');
    if !prefix.is_empty() {
        for seg in prefix.split('/') {
            let entries = tree_of(&root)?;
            match entries.iter().find(|e| e.name == seg && e.mode == "40000") {
                Some(e) => root = e.id,
                // The folder is not in this commit: nothing under it.
                None => return Ok(Vec::new()),
            }
        }
    }
    let mut out = Vec::new();
    let mut stack = vec![(prefix.to_owned(), root)];
    while let Some((dir, id)) = stack.pop() {
        for e in tree_of(&id)? {
            let full = if dir.is_empty() {
                e.name.clone()
            } else {
                format!("{dir}/{}", e.name)
            };
            let kind = match e.mode.as_str() {
                "40000" => "tree",
                "160000" => "commit",
                _ => "blob",
            };
            if kind == "tree" {
                stack.push((full.clone(), e.id));
            }
            let size = objects
                .get(&e.id)
                .filter(|o| o.kind == Kind::Blob)
                .map(|o| o.data.len() as u64);
            out.push(TreeEntry {
                path: full,
                kind: kind.to_owned(),
                mode: e.mode,
                blob: pack::hex(&e.id),
                size,
            });
            if out.len() > limits::LIBRARY_TREE_MAX {
                return Err(ReadError::TreeTooLarge);
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

#[async_trait]
impl RepoSource for GitRepo<'_> {
    async fn resolve(&self, reference: Option<&str>) -> Result<Resolved, ReadError> {
        let not_found = || unreachable(reason::REF_NOT_FOUND);
        let mut prefixes = vec!["HEAD".to_owned()];
        let candidates: Vec<(String, RefKind)> = match reference {
            None => Vec::new(),
            Some(r) if r.starts_with("refs/heads/") => vec![(r.to_owned(), RefKind::Branch)],
            Some(r) if r.starts_with("refs/tags/") => vec![(r.to_owned(), RefKind::Tag)],
            Some(r) => vec![
                (format!("refs/heads/{r}"), RefKind::Branch),
                (format!("refs/tags/{r}"), RefKind::Tag),
            ],
        };
        prefixes.extend(candidates.iter().map(|(n, _)| n.clone()));
        let refs = self.refs(&prefixes).await?;
        let head = refs.iter().find(|r| r.name == "HEAD");
        let head_target = head.and_then(|h| h.symref_target.clone());
        let short = |n: &str| {
            n.strip_prefix("refs/heads/")
                .or_else(|| n.strip_prefix("refs/tags/"))
                .unwrap_or(n)
                .to_owned()
        };
        let Some(r) = reference else {
            let h = head.ok_or_else(not_found)?;
            let name = head_target.as_deref().map_or_else(|| "HEAD".to_owned(), short);
            return Ok(Resolved {
                sha: pack::hex(&h.id),
                kind: RefKind::Branch,
                name,
                default: true,
            });
        };
        for (full, kind) in &candidates {
            if let Some(found) = refs.iter().find(|x| &x.name == full) {
                let id = found.peeled.unwrap_or(found.id);
                return Ok(Resolved {
                    sha: pack::hex(&id),
                    kind: *kind,
                    name: short(full),
                    default: *kind == RefKind::Branch && head_target.as_deref() == Some(full),
                });
            }
        }
        let lower = r.to_ascii_lowercase();
        if pack::parse_hex(&lower).is_some() {
            return Ok(Resolved {
                sha: lower.clone(),
                kind: RefKind::Commit,
                name: lower,
                default: false,
            });
        }
        if lower.len() == 64 && lower.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(unreachable(reason::SHA256));
        }
        Err(not_found())
    }

    async fn tree(&self, sha: &str, path: &str) -> Result<Vec<TreeEntry>, ReadError> {
        let id = pack::parse_hex(sha).ok_or_else(|| unreachable(reason::REF_NOT_FOUND))?;
        let have = {
            let st = self.state.lock().await;
            st.objects.get(&id).is_some_and(|o| o.kind == Kind::Commit)
        };
        if !have {
            match self.fetch(&[sha.to_owned()], true, true).await {
                Ok(()) => {}
                Err(ReadError::Unreachable(r)) if r == REFUSED => {
                    return Err(unreachable(reason::REF_NOT_FOUND));
                }
                Err(e) => return Err(e),
            }
        }
        let mut st = self.state.lock().await;
        st.commit = Some(id);
        walk(&st.objects, &id, path)
    }

    async fn blobs(&self, ids: &[String]) -> Result<HashMap<String, Vec<u8>>, ReadError> {
        let mut want: Vec<String> = Vec::new();
        {
            let st = self.state.lock().await;
            for i in ids {
                let oid = pack::parse_hex(i).ok_or_else(|| unreachable(reason::MISMATCH))?;
                if !st.objects.get(&oid).is_some_and(|o| o.kind == Kind::Blob)
                    && !want.contains(i)
                {
                    want.push(i.clone());
                }
            }
        }
        for chunk in want.chunks(limits::LIBRARY_WANTS_MAX) {
            match self.fetch(chunk, false, false).await {
                Ok(()) => {}
                // (C-1125) A server that refuses wanting a blob by id: the whole depth-1 pack.
                Err(ReadError::Unreachable(r)) if r == REFUSED => {
                    let commit = self.state.lock().await.commit;
                    let Some(c) = commit else {
                        return Err(unreachable(reason::PROTOCOL));
                    };
                    match self.fetch(&[pack::hex(&c)], true, false).await {
                        Ok(()) => break,
                        Err(ReadError::Unreachable(r)) if r == REFUSED => {
                            return Err(unreachable(reason::PROTOCOL));
                        }
                        Err(e) => return Err(e),
                    }
                }
                Err(e) => return Err(e),
            }
        }
        let st = self.state.lock().await;
        let mut out = HashMap::with_capacity(ids.len());
        for i in ids {
            let oid = pack::parse_hex(i).ok_or_else(|| unreachable(reason::MISMATCH))?;
            match st.objects.get(&oid) {
                Some(o) if o.kind == Kind::Blob => {
                    out.insert(i.clone(), o.data.clone());
                }
                _ => return Err(unreachable(reason::MISMATCH)),
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use pack::tests::{E, build};

    fn tree(entries: &[(&str, &str, Oid)]) -> Vec<u8> {
        let mut t = Vec::new();
        for (mode, name, id) in entries {
            t.extend_from_slice(format!("{mode} {name}\0").as_bytes());
            t.extend_from_slice(id);
        }
        t
    }

    #[test]
    fn walk_lists_the_folder_recursively_and_refuses_missing_objects() {
        let blob = b"x".to_vec();
        let b = pack::object_id(Kind::Blob, &blob);
        let inner = tree(&[("100644", "SKILL.md", b), ("120000", "l", b)]);
        let inner_id = pack::object_id(Kind::Tree, &inner);
        let skills = tree(&[("40000", "alpha", inner_id), ("160000", "sub", b)]);
        let skills_id = pack::object_id(Kind::Tree, &skills);
        let root = tree(&[("40000", "skills", skills_id), ("100644", "README", b)]);
        let root_id = pack::object_id(Kind::Tree, &root);
        let commit = format!("tree {}\nauthor x\n\nm\n", pack::hex(&root_id)).into_bytes();
        let commit_id = pack::object_id(Kind::Commit, &commit);
        let p = build(&[
            E::Plain(Kind::Commit, commit),
            E::Plain(Kind::Tree, root),
            E::Plain(Kind::Tree, skills),
            E::Plain(Kind::Tree, inner),
        ]);
        let objects = pack::parse(&p, pack::Limits::standard()).unwrap();
        let all = walk(&objects, &commit_id, "").unwrap();
        let paths: Vec<&str> = all.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            ["README", "skills", "skills/alpha", "skills/alpha/SKILL.md", "skills/alpha/l", "skills/sub"]
        );
        let under = walk(&objects, &commit_id, "skills").unwrap();
        assert_eq!(under.len(), 4);
        assert_eq!(under[0].kind, "tree");
        assert_eq!(under[3].kind, "commit");
        assert_eq!(under[1].size, None);
        assert!(walk(&objects, &commit_id, "nope").unwrap().is_empty());
        // a listed tree that is not in the pack fails the read
        let p = build(&[
            E::Plain(Kind::Commit, format!("tree {}\n\n", pack::hex(&root_id)).into_bytes()),
        ]);
        let objects = pack::parse(&p, pack::Limits::standard()).unwrap();
        let only = objects.keys().next().copied().unwrap();
        assert_eq!(
            walk(&objects, &only, ""),
            Err(ReadError::Unreachable(reason::MISMATCH.into()))
        );
    }

    #[tokio::test]
    async fn a_private_address_needs_allowed_private_and_http_is_never_dialled() {
        let ring = aurora_crypto::SecretRing::default();
        let strict = Env::new("http://hooks.example:4104", &ring, &[], None).unwrap();
        let r = GitRepo::new(&strict, "https://localhost:1/acme/skills", None, Duration::from_secs(2))
            .unwrap();
        assert_eq!(
            r.resolve(None).await,
            Err(ReadError::Unreachable(reason::ADDRESS_REFUSED.into()))
        );
        let r = GitRepo::new(&strict, "https://10.1.2.3/acme/skills", None, Duration::from_secs(2))
            .unwrap();
        assert_eq!(
            r.resolve(None).await,
            Err(ReadError::Unreachable(reason::ADDRESS_REFUSED.into()))
        );
        let open = Env::new(
            "http://hooks.example:4104",
            &ring,
            &["localhost".to_owned()],
            None,
        )
        .unwrap();
        let r = GitRepo::new(&open, "https://localhost:1/acme/skills", None, Duration::from_secs(2))
            .unwrap();
        assert_eq!(
            r.resolve(None).await,
            Err(ReadError::Unreachable(reason::UNREACHABLE.into()))
        );
        assert!(GitRepo::new(&open, "http://localhost:1/acme", None, Duration::from_secs(1)).is_err());
    }
}
