//! A fake git host for the skill-library tests (docs/18 §Git skill library; phase 4, P4-GSL).
//!
//! Serves a GitHub-shaped API (`/repos/{owner}/{name}`, `…/commits/{ref}`,
//! `…/git/trees/{sha}?recursive=1`, `…/git/blobs/{blob}`) and a GitLab-shaped one
//! (`/api/v4/projects/{id}`, `…/repository/commits/{ref}`, `…/repository/tree` paginated by a
//! `Link` header, `…/repository/blobs/{blob}/raw`) from in-memory repositories: branches with
//! history, a moving head, renames and deletions, symlink and submodule entries, oversize files,
//! hostile names and descriptions, truncated trees, status overrides and rate-limit responses.
//! Every request is recorded (method, target, `Authorization`) so tests can assert exactly what
//! the gateway sent.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use percent_encoding::percent_decode_str;
use sha2::{Digest, Sha256};

/// One tree entry of a commit.
#[derive(Clone, Debug)]
pub struct Entry {
    pub content: Vec<u8>,
    /// `100644`, `100755`, `120000` (symlink) or `160000` (submodule).
    pub mode: String,
}

#[derive(Clone, Debug)]
pub enum Change {
    Put(String, Vec<u8>),
    Exec(String, Vec<u8>),
    Symlink(String, String),
    Submodule(String),
    Delete(String),
    /// Delete every path under a directory (a vanished skill).
    DeleteDir(String),
    /// Rename a path (same blob).
    Rename(String, String),
}

#[derive(Clone, Debug)]
struct Commit {
    files: BTreeMap<String, Entry>,
}

#[derive(Default, Clone, Debug)]
struct Repo {
    default_branch: String,
    /// sha -> commit
    commits: HashMap<String, Commit>,
    /// branch or tag name -> head sha
    refs: HashMap<String, String>,
}

/// A recorded request.
#[derive(Clone, Debug)]
pub struct Recorded {
    pub method: String,
    /// Path and query exactly as received.
    pub target: String,
    pub authorization: Option<String>,
    /// Every request header name (lowercase).
    pub header_names: Vec<String>,
}

#[derive(Default)]
struct Inner {
    repos: HashMap<String, Repo>,
    counter: u64,
    /// Expected `Authorization` header value; anything else is 401.
    expect_auth: Option<String>,
    /// Entries per tree page for the GitLab shape (and `link` pagination).
    page_size: usize,
    /// The GitHub-shaped tree answers `truncated: true`.
    truncated: bool,
    /// target substring -> status (answered instead of the content).
    overrides: Vec<(String, u16)>,
    /// target substring -> redirect location.
    redirects: Vec<(String, String)>,
    /// target substring -> response body repeated to this many bytes.
    oversize: Vec<(String, usize)>,
    log: Vec<Recorded>,
}

#[derive(Clone)]
pub struct FakeGitHost {
    inner: Arc<Mutex<Inner>>,
    addr: std::net::SocketAddr,
}

fn sha(seed: &str, n: u64) -> String {
    let mut h = Sha256::new();
    h.update(seed.as_bytes());
    h.update(n.to_be_bytes());
    hex(&h.finalize())[..40].to_owned()
}

fn blob_id(content: &[u8], mode: &str) -> String {
    let mut h = Sha256::new();
    h.update(mode.as_bytes());
    h.update(content);
    hex(&h.finalize())[..40].to_owned()
}

fn hex(b: &[u8]) -> String {
    use std::fmt::Write as _;
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

impl FakeGitHost {
    pub async fn start() -> Self {
        let inner = Arc::new(Mutex::new(Inner {
            page_size: 100,
            ..Inner::default()
        }));
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let st = inner.clone();
        let app = Router::new().fallback(move |req: Request<Body>| {
            let st = st.clone();
            async move { handle(st, req) }
        });
        tokio::spawn(async move {
            let _ = axum::serve(l, app).await;
        });
        Self { inner, addr }
    }

    /// `127.0.0.1:PORT`.
    pub fn addr(&self) -> std::net::SocketAddr {
        self.addr
    }

    pub fn base(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Requires exactly this `Authorization` header value (`Bearer <token>`); others get 401.
    pub fn expect_authorization(&self, value: &str) {
        self.inner.lock().unwrap().expect_auth = Some(value.to_owned());
    }

    /// Creates a repository whose default branch is `main` with an initial empty commit.
    pub fn add_repo(&self, name: &str) {
        let mut g = self.inner.lock().unwrap();
        g.counter += 1;
        let c = sha(name, g.counter);
        let repo = g.repos.entry(name.to_owned()).or_default();
        repo.default_branch = "main".into();
        repo.commits.insert(
            c.clone(),
            Commit {
                files: BTreeMap::new(),
            },
        );
        repo.refs.insert("main".into(), c);
    }

    pub fn set_default_branch(&self, repo: &str, branch: &str) {
        self.inner
            .lock()
            .unwrap()
            .repos
            .get_mut(repo)
            .unwrap()
            .default_branch = branch.to_owned();
    }

    /// A new commit on `branch` (created from the default branch's head when new) applying
    /// `changes`; returns its sha. The branch head moves.
    pub fn commit(&self, repo: &str, branch: &str, changes: &[Change]) -> String {
        let mut g = self.inner.lock().unwrap();
        g.counter += 1;
        let n = g.counter;
        let r = g.repos.get_mut(repo).expect("repo exists");
        let base = r
            .refs
            .get(branch)
            .or_else(|| r.refs.get(&r.default_branch))
            .cloned()
            .expect("base ref");
        let mut files = r.commits[&base].files.clone();
        for c in changes {
            match c {
                Change::Put(p, b) => {
                    files.insert(
                        p.clone(),
                        Entry {
                            content: b.clone(),
                            mode: "100644".into(),
                        },
                    );
                }
                Change::Exec(p, b) => {
                    files.insert(
                        p.clone(),
                        Entry {
                            content: b.clone(),
                            mode: "100755".into(),
                        },
                    );
                }
                Change::Symlink(p, t) => {
                    files.insert(
                        p.clone(),
                        Entry {
                            content: t.clone().into_bytes(),
                            mode: "120000".into(),
                        },
                    );
                }
                Change::Submodule(p) => {
                    files.insert(
                        p.clone(),
                        Entry {
                            content: b"submodule".to_vec(),
                            mode: "160000".into(),
                        },
                    );
                }
                Change::Delete(p) => {
                    files.remove(p);
                }
                Change::DeleteDir(d) => {
                    let pre = format!("{}/", d.trim_end_matches('/'));
                    files.retain(|k, _| !k.starts_with(&pre));
                }
                Change::Rename(a, b) => {
                    if let Some(e) = files.remove(a) {
                        files.insert(b.clone(), e);
                    }
                }
            }
        }
        let s = sha(repo, n);
        r.commits.insert(s.clone(), Commit { files });
        r.refs.insert(branch.to_owned(), s.clone());
        s
    }

    pub fn head(&self, repo: &str, branch: &str) -> String {
        self.inner.lock().unwrap().repos[repo].refs[branch].clone()
    }

    pub fn set_page_size(&self, n: usize) {
        self.inner.lock().unwrap().page_size = n.max(1);
    }

    pub fn set_truncated(&self, on: bool) {
        self.inner.lock().unwrap().truncated = on;
    }

    /// Answers `status` (empty body) for every request whose target contains `needle`.
    pub fn override_status(&self, needle: &str, status: u16) {
        self.inner
            .lock()
            .unwrap()
            .overrides
            .push((needle.to_owned(), status));
    }

    pub fn redirect(&self, needle: &str, to: &str) {
        self.inner
            .lock()
            .unwrap()
            .redirects
            .push((needle.to_owned(), to.to_owned()));
    }

    /// Answers a body of `bytes` bytes for requests containing `needle` (size-cap tests).
    pub fn oversize(&self, needle: &str, bytes: usize) {
        self.inner
            .lock()
            .unwrap()
            .oversize
            .push((needle.to_owned(), bytes));
    }

    pub fn clear_overrides(&self) {
        let mut g = self.inner.lock().unwrap();
        g.overrides.clear();
        g.redirects.clear();
        g.oversize.clear();
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.inner.lock().unwrap().log.clone()
    }

    pub fn clear_requests(&self) {
        self.inner.lock().unwrap().log.clear();
    }

    /// Requests whose target contains `needle`.
    pub fn count(&self, needle: &str) -> usize {
        self.inner
            .lock()
            .unwrap()
            .log
            .iter()
            .filter(|r| r.target.contains(needle))
            .count()
    }
}

fn json(status: StatusCode, v: serde_json::Value) -> Response {
    let mut r = (status, v.to_string()).into_response();
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    r
}

fn not_found() -> Response {
    json(
        StatusCode::NOT_FOUND,
        serde_json::json!({"message": "Not Found"}),
    )
}

fn handle(st: Arc<Mutex<Inner>>, req: Request<Body>) -> Response {
    let target = req
        .uri()
        .path_and_query()
        .map_or_else(|| req.uri().path().to_owned(), ToString::to_string);
    let headers: &HeaderMap = req.headers();
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let mut g = st.lock().unwrap();
    g.log.push(Recorded {
        method: req.method().to_string(),
        target: target.clone(),
        authorization: auth.clone(),
        header_names: headers.keys().map(|k| k.as_str().to_owned()).collect(),
    });
    if let Some(want) = &g.expect_auth
        && auth.as_deref() != Some(want.as_str())
    {
        return json(
            StatusCode::UNAUTHORIZED,
            serde_json::json!({"message": "Bad credentials"}),
        );
    }
    if let Some((_, to)) = g
        .redirects
        .iter()
        .find(|(n, _)| target.contains(n.as_str()))
    {
        let mut r = StatusCode::FOUND.into_response();
        r.headers_mut()
            .insert(header::LOCATION, HeaderValue::from_str(to).unwrap());
        return r;
    }
    if let Some((_, s)) = g
        .overrides
        .iter()
        .find(|(n, _)| target.contains(n.as_str()))
    {
        let mut r = json(
            StatusCode::from_u16(*s).unwrap(),
            serde_json::json!({"message": "overridden"}),
        );
        if *s == 429 || *s == 403 {
            r.headers_mut()
                .insert("retry-after", HeaderValue::from_static("30"));
        }
        return r;
    }
    if let Some((_, n)) = g
        .oversize
        .iter()
        .find(|(needle, _)| target.contains(needle.as_str()))
    {
        return (StatusCode::OK, vec![b'x'; *n]).into_response();
    }
    if req.method() != axum::http::Method::GET {
        return json(
            StatusCode::METHOD_NOT_ALLOWED,
            serde_json::json!({"message": "GET only"}),
        );
    }
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let segs: Vec<String> = path
        .trim_start_matches('/')
        .split('/')
        .map(|s| percent_decode_str(s).decode_utf8_lossy().into_owned())
        .collect();
    let q = |k: &str| {
        query.split('&').find_map(|kv| {
            let (a, b) = kv.split_once('=')?;
            (a == k).then(|| percent_decode_str(b).decode_utf8_lossy().into_owned())
        })
    };
    if segs.first().is_some_and(|s| s == "repos") {
        return github(&g, &segs, query);
    }
    if segs.len() > 3 && segs[0] == "api" && segs[1] == "v4" && segs[2] == "projects" {
        return gitlab(&g, &segs, &q);
    }
    not_found()
}

fn resolve(r: &Repo, reference: &str) -> Option<String> {
    // `HEAD` is the default branch's head on both hosts.
    let reference = if reference == "HEAD" {
        r.default_branch.as_str()
    } else {
        reference
    };
    r.refs.get(reference).cloned().or_else(|| {
        r.commits
            .contains_key(reference)
            .then(|| reference.to_owned())
    })
}

fn blob_of(r: &Repo, id: &str) -> Option<Vec<u8>> {
    r.commits
        .values()
        .flat_map(|c| c.files.values())
        .find(|e| blob_id(&e.content, &e.mode) == id)
        .map(|e| e.content.clone())
}

fn github(g: &Inner, segs: &[String], query: &str) -> Response {
    if segs.len() < 3 {
        return not_found();
    }
    let name = format!("{}/{}", segs[1], segs[2]);
    let Some(r) = g.repos.get(&name) else {
        return not_found();
    };
    match segs.get(3).map(String::as_str) {
        None => json(
            StatusCode::OK,
            serde_json::json!({"full_name": name, "default_branch": r.default_branch}),
        ),
        Some("commits") => {
            let reference = segs[4..].join("/");
            match resolve(r, &reference) {
                Some(s) => json(StatusCode::OK, serde_json::json!({"sha": s})),
                None => json(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    serde_json::json!({"message": "No commit found"}),
                ),
            }
        }
        Some("git") if segs.get(4).is_some_and(|s| s == "trees") => {
            let Some(c) = segs.get(5).and_then(|s| r.commits.get(s)) else {
                return not_found();
            };
            if !query.contains("recursive=1") {
                return json(
                    StatusCode::BAD_REQUEST,
                    serde_json::json!({"message": "recursive needed"}),
                );
            }
            let mut tree: Vec<serde_json::Value> = Vec::new();
            let mut dirs = std::collections::BTreeSet::new();
            for (p, e) in &c.files {
                let mut acc = String::new();
                for seg in p.split('/').take(p.split('/').count() - 1) {
                    if !acc.is_empty() {
                        acc.push('/');
                    }
                    acc.push_str(seg);
                    dirs.insert(acc.clone());
                }
                let ty = if e.mode == "160000" { "commit" } else { "blob" };
                tree.push(serde_json::json!({"path": p, "mode": e.mode, "type": ty,
                    "sha": blob_id(&e.content, &e.mode), "size": e.content.len()}));
            }
            for d in dirs {
                tree.push(
                    serde_json::json!({"path": d, "mode": "040000", "type": "tree",
                    "sha": blob_id(d.as_bytes(), "040000")}),
                );
            }
            json(
                StatusCode::OK,
                serde_json::json!({"sha": segs[5], "truncated": g.truncated, "tree": tree}),
            )
        }
        Some("git") if segs.get(4).is_some_and(|s| s == "blobs") => {
            match segs.get(5).and_then(|id| blob_of(r, id)) {
                Some(b) => {
                    let enc = base64::engine::general_purpose::STANDARD.encode(&b);
                    // GitHub wraps base64 at 60 columns.
                    let wrapped: Vec<&str> = enc
                        .as_bytes()
                        .chunks(60)
                        .map(|c| std::str::from_utf8(c).unwrap())
                        .collect();
                    json(
                        StatusCode::OK,
                        serde_json::json!({"content": wrapped.join("\n") + "\n", "encoding": "base64", "size": b.len()}),
                    )
                }
                None => not_found(),
            }
        }
        _ => not_found(),
    }
}

fn gitlab(g: &Inner, segs: &[String], q: &dyn Fn(&str) -> Option<String>) -> Response {
    let name = segs[3].clone();
    let Some(r) = g.repos.get(&name) else {
        return not_found();
    };
    match (
        segs.get(4).map(String::as_str),
        segs.get(5).map(String::as_str),
    ) {
        (None, _) => json(
            StatusCode::OK,
            serde_json::json!({"path_with_namespace": name, "default_branch": r.default_branch}),
        ),
        (Some("repository"), Some("commits")) => {
            let reference = segs[6..].join("/");
            match resolve(r, &reference) {
                Some(s) => json(StatusCode::OK, serde_json::json!({"id": s})),
                None => not_found(),
            }
        }
        (Some("repository"), Some("tree")) => {
            let Some(c) = q("ref").and_then(|s| r.commits.get(&s)) else {
                return not_found();
            };
            let mut all: Vec<serde_json::Value> = Vec::new();
            let mut dirs = std::collections::BTreeSet::new();
            for (p, e) in &c.files {
                let mut acc = String::new();
                for seg in p.split('/').take(p.split('/').count() - 1) {
                    if !acc.is_empty() {
                        acc.push('/');
                    }
                    acc.push_str(seg);
                    dirs.insert(acc.clone());
                }
                let ty = if e.mode == "160000" { "commit" } else { "blob" };
                all.push(serde_json::json!({"id": blob_id(&e.content, &e.mode), "name": p.rsplit('/').next(),
                    "type": ty, "path": p, "mode": e.mode}));
            }
            for d in dirs {
                all.push(serde_json::json!({"id": blob_id(d.as_bytes(), "040000"),
                    "name": d.rsplit('/').next(), "type": "tree", "path": d, "mode": "040000"}));
            }
            let page: usize = q("page").and_then(|p| p.parse().ok()).unwrap_or(1).max(1);
            let size = g.page_size;
            let slice: Vec<_> = all
                .iter()
                .skip((page - 1) * size)
                .take(size)
                .cloned()
                .collect();
            let mut resp = json(StatusCode::OK, serde_json::Value::Array(slice));
            if page * size < all.len() {
                let next = format!(
                    "<http://example.invalid/api/v4/projects/x/repository/tree?page={}&per_page={size}>; rel=\"next\"",
                    page + 1
                );
                resp.headers_mut()
                    .insert("link", HeaderValue::from_str(&next).unwrap());
            }
            resp
        }
        (Some("repository"), Some("blobs")) if segs.get(7).is_some_and(|s| s == "raw") => {
            match segs.get(6).and_then(|id| blob_of(r, id)) {
                Some(b) => (StatusCode::OK, b).into_response(),
                None => not_found(),
            }
        }
        _ => not_found(),
    }
}
