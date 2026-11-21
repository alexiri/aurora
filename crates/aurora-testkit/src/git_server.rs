//! A real git server for the skill-library tests (P5-SKLIB-1, C-1125; docs/18 §Git skill
//! library): bare repositories built with the `git` CLI in a temporary directory and served by
//! **`git http-backend`** (CGI) behind a small axum server, so the API's own git reader is tested
//! against git itself.
//!
//! Needs `git` on the machine (CI runners and dev boxes have it); [`GitServer::start`] panics with
//! a clear message when it is missing, it never skips.
//!
//! Switches: protocol v2 off (the `Git-Protocol` header is not passed on), `uploadpack.allowFilter`
//! and `uploadpack.allowAnySHA1InWant` off, Basic auth required with a given token, an injected
//! status (with `Retry-After`), redirects, a slow answer, an oversize fetch answer, a fetch
//! answered with a pack of other objects; and a log of every request with its `Authorization`.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::{BTreeMap, HashMap};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderValue, Request, StatusCode, header};
use axum::response::Response;
use base64::Engine as _;

/// One change of [`GitServer::commit`] (the same vocabulary as the old fake host).
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

/// A recorded request.
#[derive(Clone, Debug)]
pub struct Recorded {
    pub method: String,
    /// Path and query exactly as received.
    pub target: String,
    pub authorization: Option<String>,
    pub git_protocol: Option<String>,
    /// The request body (upload-pack requests are short pkt-line texts).
    pub body: Vec<u8>,
}

#[derive(Clone, Debug)]
struct Entry {
    mode: &'static str,
    id: String,
}

#[derive(Default)]
struct Inner {
    /// repo -> commit sha -> files.
    commits: HashMap<String, HashMap<String, BTreeMap<String, Entry>>>,
    counter: u64,
    v2: bool,
    allow_filter: bool,
    allow_any_sha1: bool,
    /// `(user, token)` required as Basic auth.
    auth: Option<(String, String)>,
    /// target substring -> (status, Retry-After).
    statuses: Vec<(String, u16, Option<String>)>,
    /// target substring -> Location.
    redirects: Vec<(String, String)>,
    /// target substring -> delay.
    slow: Vec<(String, Duration)>,
    /// Bytes of a streamed answer to fetch requests (no length header).
    oversize: Option<usize>,
    /// Fetch requests get a valid pack of an unrelated object.
    wrong_pack: bool,
    log: Vec<Recorded>,
}

struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone)]
pub struct GitServer {
    inner: Arc<Mutex<Inner>>,
    root: Arc<Dir>,
    addr: std::net::SocketAddr,
}

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

impl GitServer {
    /// Starts the server. Panics when `git` is not installed.
    pub async fn start() -> Self {
        assert!(
            git_available(),
            "aurora-testkit GitServer needs the `git` command (it serves repositories with \
             `git http-backend`); install git to run the skill-library tests"
        );
        let root = std::env::temp_dir().join(format!("aurora-git-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        let inner = Arc::new(Mutex::new(Inner {
            v2: true,
            allow_filter: true,
            allow_any_sha1: true,
            ..Inner::default()
        }));
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let st = inner.clone();
        let dir = root.clone();
        fn send<F: std::future::Future<Output = Response> + Send>(f: F) -> F {
            f
        }
        let app = Router::new().fallback(move |req: Request<Body>| {
            let st = st.clone();
            let dir = dir.clone();
            send(handle(st, dir, req))
        });
        tokio::spawn(async move {
            let _ = axum::serve(l, app).await;
        });
        Self {
            inner,
            root: Arc::new(Dir(root)),
            addr,
        }
    }

    /// `http://127.0.0.1:PORT` (the API's test override maps every https origin here).
    pub fn base(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn dir(&self, repo: &str) -> PathBuf {
        self.root.0.join(repo)
    }

    fn git(&self, repo: &str, args: &[&str], stdin: Option<&[u8]>, index: Option<&Path>) -> String {
        let mut c = Command::new("git");
        c.args(args)
            .env("GIT_DIR", self.dir(repo))
            .env("HOME", &self.root.0)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let n = {
            let mut g = self.inner.lock().unwrap();
            g.counter += 1;
            g.counter
        };
        let date = format!("{} +0000", 1_700_000_000 + n);
        c.env("GIT_AUTHOR_DATE", &date).env("GIT_COMMITTER_DATE", &date);
        if let Some(i) = index {
            c.env("GIT_INDEX_FILE", i);
        }
        let mut child = c.spawn().expect("git runs");
        if let Some(b) = stdin {
            child.stdin.take().unwrap().write_all(b).unwrap();
        }
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    fn init(&self, name: &str, format: &str) {
        let d = self.dir(name);
        std::fs::create_dir_all(&d).unwrap();
        let out = Command::new("git")
            .args(["init", "--bare", "-q", "-b", "main", "--object-format", format])
            .arg(&d)
            .env("HOME", &self.root.0)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        self.inner
            .lock()
            .unwrap()
            .commits
            .insert(name.to_owned(), HashMap::new());
        self.write_commit(name, "main", None, BTreeMap::new());
    }

    /// A repository whose default branch is `main` with an initial empty commit.
    pub fn add_repo(&self, name: &str) {
        self.init(name, "sha1");
    }

    /// The same with SHA-256 object ids (refused by the reader).
    pub fn add_repo_sha256(&self, name: &str) {
        self.init(name, "sha256");
    }

    fn write_commit(
        &self,
        repo: &str,
        branch: &str,
        parent: Option<&str>,
        files: BTreeMap<String, Entry>,
    ) -> String {
        let index = self
            .root
            .0
            .join(format!("index-{}", uuid::Uuid::now_v7()));
        self.git(repo, &["read-tree", "--empty"], None, Some(&index));
        let mut info = String::new();
        for (p, e) in &files {
            info.push_str(&format!("{} {}\t{p}\n", e.mode, e.id));
        }
        if !info.is_empty() {
            self.git(
                repo,
                &["update-index", "--index-info"],
                Some(info.as_bytes()),
                Some(&index),
            );
        }
        let tree = self.git(repo, &["write-tree"], None, Some(&index));
        let _ = std::fs::remove_file(&index);
        let mut args = vec!["commit-tree", tree.as_str(), "-m", "c"];
        if let Some(p) = parent {
            args.extend(["-p", p]);
        }
        let sha = self.git(repo, &args, None, None);
        self.git(
            repo,
            &["update-ref", &format!("refs/heads/{branch}"), &sha],
            None,
            None,
        );
        self.inner
            .lock()
            .unwrap()
            .commits
            .get_mut(repo)
            .expect("repo exists")
            .insert(sha.clone(), files);
        sha
    }

    fn blob(&self, repo: &str, content: &[u8]) -> String {
        self.git(repo, &["hash-object", "-w", "--stdin"], Some(content), None)
    }

    /// A new commit on `branch` (created from `main`'s head when new) applying `changes`;
    /// returns its id. The branch head moves.
    pub fn commit(&self, repo: &str, branch: &str, changes: &[Change]) -> String {
        let base = self
            .try_head(repo, branch)
            .or_else(|| self.try_head(repo, "main"))
            .expect("base ref");
        let mut files = self.inner.lock().unwrap().commits[repo][&base].clone();
        for c in changes {
            match c {
                Change::Put(p, b) => {
                    let id = self.blob(repo, b);
                    files.insert(p.clone(), Entry { mode: "100644", id });
                }
                Change::Exec(p, b) => {
                    let id = self.blob(repo, b);
                    files.insert(p.clone(), Entry { mode: "100755", id });
                }
                Change::Symlink(p, t) => {
                    let id = self.blob(repo, t.as_bytes());
                    files.insert(p.clone(), Entry { mode: "120000", id });
                }
                Change::Submodule(p) => {
                    files.insert(
                        p.clone(),
                        Entry {
                            mode: "160000",
                            id: base.clone(),
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
        self.write_commit(repo, branch, Some(&base), files)
    }

    fn try_head(&self, repo: &str, branch: &str) -> Option<String> {
        let out = Command::new("git")
            .args(["rev-parse", "--verify", "-q", &format!("refs/heads/{branch}")])
            .env("GIT_DIR", self.dir(repo))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        out.status
            .success()
            .then(|| String::from_utf8(out.stdout).unwrap().trim().to_owned())
    }

    /// The head of `branch`.
    pub fn head(&self, repo: &str, branch: &str) -> String {
        self.try_head(repo, branch).expect("branch exists")
    }

    /// The blob id of `path` at the head of `branch` (git's own id).
    pub fn blob_id(&self, repo: &str, branch: &str, path: &str) -> String {
        let head = self.head(repo, branch);
        self.inner.lock().unwrap().commits[repo][&head][path]
            .id
            .clone()
    }

    /// A tag on `target` (a commit id): annotated (a tag object) or lightweight.
    pub fn tag(&self, repo: &str, name: &str, target: &str, annotated: bool) {
        if annotated {
            self.git(repo, &["tag", "-a", name, "-m", "t", target], None, None);
        } else {
            self.git(
                repo,
                &["update-ref", &format!("refs/tags/{name}"), target],
                None,
                None,
            );
        }
    }

    /// Points `HEAD` (the default branch) at another branch.
    pub fn set_default_branch(&self, repo: &str, branch: &str) {
        self.git(
            repo,
            &["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")],
            None,
            None,
        );
    }

    /// Protocol v2 on (default) or off (the `Git-Protocol` header is dropped: v0 answers).
    pub fn set_v2(&self, on: bool) {
        self.inner.lock().unwrap().v2 = on;
    }

    /// `uploadpack.allowFilter` (default on).
    pub fn set_allow_filter(&self, on: bool) {
        self.inner.lock().unwrap().allow_filter = on;
    }

    /// `uploadpack.allowAnySHA1InWant` (default on; v0 only: protocol v2 always allows it).
    pub fn set_allow_any_sha1(&self, on: bool) {
        self.inner.lock().unwrap().allow_any_sha1 = on;
    }

    /// Requires `Authorization: Basic base64(user:token)`; anything else is 401.
    pub fn require_token(&self, user: &str, token: &str) {
        self.inner.lock().unwrap().auth = Some((user.to_owned(), token.to_owned()));
    }

    /// Answers `status` (with `Retry-After` when given) to requests whose target contains `needle`.
    pub fn override_status(&self, needle: &str, status: u16, retry_after: Option<&str>) {
        self.inner.lock().unwrap().statuses.push((
            needle.to_owned(),
            status,
            retry_after.map(str::to_owned),
        ));
    }

    /// Answers 302 `Location: to` to requests whose target contains `needle`.
    pub fn redirect(&self, needle: &str, to: &str) {
        self.inner
            .lock()
            .unwrap()
            .redirects
            .push((needle.to_owned(), to.to_owned()));
    }

    /// Delays answers to requests whose target contains `needle`.
    pub fn slow(&self, needle: &str, by: Duration) {
        self.inner
            .lock()
            .unwrap()
            .slow
            .push((needle.to_owned(), by));
    }

    /// Fetch requests get a streamed answer of `bytes` bytes without a length header.
    pub fn oversize(&self, bytes: Option<usize>) {
        self.inner.lock().unwrap().oversize = bytes;
    }

    /// Fetch requests get a valid pack of an unrelated object (ids that were not asked).
    pub fn wrong_pack(&self, on: bool) {
        self.inner.lock().unwrap().wrong_pack = on;
    }

    pub fn clear_overrides(&self) {
        let mut g = self.inner.lock().unwrap();
        g.statuses.clear();
        g.redirects.clear();
        g.slow.clear();
        g.oversize = None;
        g.wrong_pack = false;
        g.auth = None;
        g.v2 = true;
        g.allow_filter = true;
        g.allow_any_sha1 = true;
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

    /// Upload-pack requests that are fetches (not `ls-refs`).
    pub fn fetches(&self) -> usize {
        self.inner
            .lock()
            .unwrap()
            .log
            .iter()
            .filter(|r| is_fetch(&r.body))
            .count()
    }
}

fn is_fetch(body: &[u8]) -> bool {
    let has = |n: &[u8]| body.windows(n.len()).any(|w| w == n);
    has(b"command=fetch") || (has(b"want ") && !has(b"command=ls-refs"))
}

fn pkt(out: &mut Vec<u8>, data: &[u8]) {
    out.extend_from_slice(format!("{:04x}", data.len() + 4).as_bytes());
    out.extend_from_slice(data);
}

fn plain(status: u16) -> Response {
    let mut r = Response::new(Body::empty());
    *r.status_mut() = StatusCode::from_u16(status).unwrap();
    r
}

async fn handle(inner: Arc<Mutex<Inner>>, root: PathBuf, req: Request<Body>) -> Response {
    let (parts, req_body) = req.into_parts();
    let method = parts.method.to_string();
    let target = parts
        .uri
        .path_and_query()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_default();
    let get = |n: &str| {
        parts
            .headers
            .get(n)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let authorization = get("authorization");
    let git_protocol = get("git-protocol");
    let content_type = get("content-type");
    let path = parts.uri.path().to_owned();
    let query = parts.uri.query().unwrap_or("").to_owned();
    drop(parts);
    let body = axum::body::to_bytes(req_body, 1 << 20)
        .await
        .unwrap_or_default()
        .to_vec();
    let (status, redirect, slow, auth, v2, filter, any, oversize, wrong) = {
        let mut g = inner.lock().unwrap();
        g.log.push(Recorded {
            method: method.clone(),
            target: target.clone(),
            authorization: authorization.clone(),
            git_protocol: git_protocol.clone(),
            body: body.clone(),
        });
        (
            g.statuses
                .iter()
                .find(|(n, ..)| target.contains(n.as_str()))
                .cloned(),
            g.redirects
                .iter()
                .find(|(n, _)| target.contains(n.as_str()))
                .map(|(_, l)| l.clone()),
            g.slow
                .iter()
                .find(|(n, _)| target.contains(n.as_str()))
                .map(|(_, d)| *d),
            g.auth.clone(),
            g.v2,
            g.allow_filter,
            g.allow_any_sha1,
            g.oversize,
            g.wrong_pack,
        )
    };
    if let Some(d) = slow {
        tokio::time::sleep(d).await;
    }
    if let Some((_, s, retry)) = status {
        let mut r = plain(s);
        if let Some(ra) = retry {
            r.headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_str(&ra).unwrap());
        }
        return r;
    }
    if let Some(loc) = redirect {
        let mut r = plain(302);
        r.headers_mut()
            .insert(header::LOCATION, HeaderValue::from_str(&loc).unwrap());
        return r;
    }
    if let Some((user, token)) = auth {
        let want = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("{user}:{token}"))
        );
        if authorization.as_deref() != Some(want.as_str()) {
            let mut r = plain(401);
            r.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static("Basic realm=\"git\""),
            );
            return r;
        }
    }
    let fetch = method == "POST" && is_fetch(&body);
    let v2_req = v2 && git_protocol.as_deref().is_some_and(|p| p.contains("version=2"));
    if fetch && let Some(n) = oversize {
        let chunk = vec![b'0'; 64 * 1024];
        let chunks = n.div_ceil(chunk.len());
        let stream = futures::stream::iter(
            (0..chunks).map(move |_| Ok::<_, std::io::Error>(bytes::Bytes::from(chunk.clone()))),
        );
        let mut r = Response::new(Body::from_stream(stream));
        r.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/x-git-upload-pack-result"),
        );
        return r;
    }
    if fetch && wrong {
        return wrong_pack_answer(&root, &path, v2_req);
    }
    cgi(&root, &method, &path, &query, content_type, git_protocol.filter(|_| v2), filter, any, body)
        .await
}

/// A fetch answer carrying a valid pack of one unrelated blob.
fn wrong_pack_answer(root: &Path, path: &str, v2: bool) -> Response {
    let repo = path
        .trim_start_matches('/')
        .trim_end_matches("/git-upload-pack")
        .to_owned();
    let gd = root.join(&repo);
    let run = |args: &[&str], input: &[u8]| {
        let mut c = Command::new("git")
            .args(args)
            .env("GIT_DIR", &gd)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        c.stdin.take().unwrap().write_all(input).unwrap();
        c.wait_with_output().unwrap().stdout
    };
    let id = run(&["hash-object", "-w", "--stdin"], b"an unrelated object\n");
    let pack = run(&["pack-objects", "--stdout", "-q"], &id);
    let mut out = Vec::new();
    if v2 {
        pkt(&mut out, b"packfile\n");
    } else {
        pkt(&mut out, b"NAK\n");
    }
    for c in pack.chunks(60_000) {
        let mut d = vec![1u8];
        d.extend_from_slice(c);
        pkt(&mut out, &d);
    }
    out.extend_from_slice(b"0000");
    let mut r = Response::new(Body::from(out));
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-git-upload-pack-result"),
    );
    r
}

#[allow(clippy::too_many_arguments)]
async fn cgi(
    root: &Path,
    method: &str,
    path: &str,
    query: &str,
    content_type: Option<String>,
    git_protocol: Option<String>,
    filter: bool,
    any: bool,
    body: Vec<u8>,
) -> Response {
    let mut c = tokio::process::Command::new("git");
    c.arg("http-backend")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_PROJECT_ROOT", root)
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("REQUEST_METHOD", method)
        .env("PATH_INFO", path)
        .env("QUERY_STRING", query)
        .env("REMOTE_ADDR", "127.0.0.1")
        .env("CONTENT_LENGTH", body.len().to_string())
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "uploadpack.allowFilter")
        .env("GIT_CONFIG_VALUE_0", if filter { "true" } else { "false" })
        .env("GIT_CONFIG_KEY_1", "uploadpack.allowAnySHA1InWant")
        .env("GIT_CONFIG_VALUE_1", if any { "true" } else { "false" })
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(ct) = content_type {
        c.env("CONTENT_TYPE", ct);
    }
    if let Some(p) = git_protocol {
        c.env("HTTP_GIT_PROTOCOL", p);
    }
    let mut child = c.spawn().expect("git http-backend runs");
    {
        use tokio::io::AsyncWriteExt as _;
        let mut stdin = child.stdin.take().unwrap();
        let _ = stdin.write_all(&body).await;
    }
    let out = child.wait_with_output().await.unwrap();
    let raw = out.stdout;
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| (i, 4))
        .or_else(|| raw.windows(2).position(|w| w == b"\n\n").map(|i| (i, 2)));
    let Some((at, sep)) = split else {
        return plain(500);
    };
    let head = String::from_utf8_lossy(&raw[..at]).to_string();
    let mut r = Response::new(Body::from(raw[at + sep..].to_vec()));
    for line in head.lines() {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let v = v.trim();
        if k.eq_ignore_ascii_case("status") {
            let code = v.split(' ').next().and_then(|c| c.parse().ok()).unwrap_or(500);
            *r.status_mut() = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        } else if let (Ok(n), Ok(val)) = (
            header::HeaderName::from_bytes(k.trim().as_bytes()),
            HeaderValue::from_str(v),
        ) {
            r.headers_mut().insert(n, val);
        }
    }
    r
}
