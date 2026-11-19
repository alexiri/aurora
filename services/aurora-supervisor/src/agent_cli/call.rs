//! (P5-GUIDE-6, `docs/24 §10a` *App targets*, SEC-118, SEC-120) `aurora-agent call <app> …`: one
//! request (or a paginated series) through the inherited proxy, the response saved and a bounded
//! preview printed.
//!
//! It adds no reach: the host is the operation's validated host or the account's `base_url`; the
//! only Aurora header is `x-aurora-connection-id`; redirects are never followed; a `next` link is
//! followed only to the origin of the first request. The gateway decides every request.
use std::path::{Path, PathBuf};
use std::time::Duration;

use aurora_core::limits::{AGENT_CLI_PAGES_DEFAULT, AGENT_CLI_PAGES_MAX, AGENT_CLI_SAVE_MAX};
use reqwest::{Method, Url};
use serde_json::Value;

use super::api::{Account, ApiDoc, Operation, Pagination};
use super::files;
use super::jsonpath;
use super::preview::{self, Summary};
use super::request::{self, Built};
use super::text::{bound, oneline};

pub const EXIT_OK: u8 = 0;
pub const EXIT_UPSTREAM: u8 = 1;
pub const EXIT_USAGE: u8 = 2;
pub const EXIT_GATEWAY: u8 = 3;

/// What a verb produced: the exit code, stdout bytes and a stderr text.
#[derive(Debug, Default)]
pub struct Outcome {
    pub code: u8,
    pub out: Vec<u8>,
    pub err: String,
}

impl Outcome {
    #[must_use]
    pub fn ok(out: String) -> Self {
        Self {
            code: EXIT_OK,
            out: out.into_bytes(),
            err: String::new(),
        }
    }

    #[must_use]
    pub fn fail(code: u8, err: impl Into<String>) -> Self {
        Self {
            code,
            out: Vec::new(),
            err: err.into(),
        }
    }

    #[must_use]
    pub fn usage(err: impl Into<String>) -> Self {
        Self::fail(EXIT_USAGE, err)
    }
}

#[derive(Debug, Default, Clone)]
pub struct CallOpts {
    pub account: Option<String>,
    pub all: bool,
    pub max_pages: Option<u32>,
    pub path: Option<String>,
    pub method: Option<String>,
    pub body: Option<String>,
    pub out: Option<PathBuf>,
    pub raw: bool,
}

/// Everything a call needs from its surroundings; tests replace the client and the roots.
pub struct Env {
    pub client: reqwest::Client,
    pub out_dir: PathBuf,
    pub protected: Vec<PathBuf>,
    pub cwd: PathBuf,
    /// Seconds before the one-line "waiting" notice (a request held for approval).
    pub wait_notice: Duration,
    /// (P5-GUIDE-7) The supervisor's tools socket, for the `aurora` target.
    pub tools_socket: PathBuf,
    /// (P5-GUIDE-7) The conversation this process works for (`AURORA_SESSION`, set only in the
    /// harness's environment); `None` outside a turn.
    pub session: Option<String>,
}

impl Env {
    /// The real environment: the proxy and trust store the shell already has, no redirects.
    pub fn production() -> Result<Self, String> {
        let mut b = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(15 * 60))
            .user_agent("aurora-agent");
        // The sandbox CA (`11 §Sandbox image`): the image points SSL_CERT_FILE at it for curl; the
        // platform verifier may not read that variable, so add it as a root explicitly.
        if let Ok(p) = std::env::var("SSL_CERT_FILE")
            && let Ok(pem) = std::fs::read(&p)
            && let Ok(certs) = reqwest::Certificate::from_pem_bundle(&pem)
        {
            for c in certs {
                b = b.add_root_certificate(c);
            }
        }
        Ok(Self {
            client: b.build().map_err(|e| format!("http client: {e}"))?,
            out_dir: PathBuf::from(files::DEFAULT_DIR),
            protected: files::PROTECTED.iter().map(PathBuf::from).collect(),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            wait_notice: Duration::from_secs(3),
            tools_socket: PathBuf::from(crate::tools::DEFAULT_SOCKET),
            session: std::env::var("AURORA_SESSION")
                .ok()
                .filter(|s| !s.is_empty()),
        })
    }
}

fn pick_account<'a>(
    doc: &'a ApiDoc,
    want: Option<&str>,
    bound_to: Option<&str>,
) -> Result<&'a Account, String> {
    // (C-911) An operation bound to an account (a webhook) uses that account; naming another is
    // refused, and the other accounts of the app are no ambiguity.
    if let Some(letter) = bound_to {
        let own = doc.account(letter).ok_or_else(|| {
            format!(
                "{}: this operation belongs to account {letter}, which is not granted",
                doc.app.id
            )
        })?;
        if let Some(k) = want
            && doc
                .account(k)
                .is_none_or(|a| a.connection_id != own.connection_id)
        {
            return Err(format!(
                "this operation belongs to account {} ({}); --account {k:?} names another",
                own.letter, own.label
            ));
        }
        return Ok(own);
    }
    let list = || {
        doc.accounts
            .iter()
            .map(|a| format!("  {} = {} ({})", a.letter, a.label, a.connection_id))
            .collect::<Vec<_>>()
            .join("\n")
    };
    match (want, doc.accounts.as_slice()) {
        (_, []) => Err(format!("{}: no granted account", doc.app.id)),
        (Some(k), _) => doc.account(k).ok_or_else(|| {
            format!(
                "no account {k:?} for {}; choose one with --account:\n{}",
                doc.app.id,
                list()
            )
        }),
        (None, [only]) => Ok(only),
        (None, _) => Err(format!(
            "{} has several accounts; choose one with --account <letter>:\n{}",
            doc.app.id,
            list()
        )),
    }
}

fn read_body(spec: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    const LIMIT: u64 = 16 * 1024 * 1024;
    let mut buf = Vec::new();
    let n = if spec == "-" {
        std::io::stdin().take(LIMIT + 1).read_to_end(&mut buf)
    } else if let Some(p) = spec.strip_prefix('@') {
        std::fs::File::open(p)
            .map_err(|e| format!("--body {p}: {e}"))?
            .take(LIMIT + 1)
            .read_to_end(&mut buf)
    } else {
        return Err("--body takes @file or - (stdin)".into());
    }
    .map_err(|e| format!("--body: {e}"))?;
    if n as u64 > LIMIT {
        return Err("--body is over 16 MiB".into());
    }
    Ok(buf)
}

fn set_query(q: &mut Vec<(String, String)>, name: &str, value: String) {
    if let Some(e) = q.iter_mut().find(|(k, _)| k == name) {
        e.1 = value;
    } else {
        q.push((name.to_owned(), value));
    }
}

struct Page {
    status: u16,
    content_type: String,
    link_next: Option<String>,
    body: Vec<u8>,
    /// The body hit `AGENT_CLI_SAVE_MAX`.
    cut: bool,
    /// (C-911) The gateway's `X-Aurora-Error` code: this answer is the gateway's, not the app's.
    aurora_error: Option<String>,
}

async fn send(env: &Env, url: &Url, built: &Built, connection_id: &str) -> Result<Page, String> {
    let method = Method::from_bytes(built.method.as_bytes())
        .map_err(|_| format!("method {:?} is not valid", built.method))?;
    let mut req = env
        .client
        .request(method, url.clone())
        .header("x-aurora-connection-id", connection_id)
        .header("accept", "application/json, */*;q=0.5");
    for (k, v) in &built.headers {
        req = req.header(k.as_str(), v.as_str());
    }
    if let Some(b) = &built.body {
        req = req
            .header("content-type", b.content_type.as_str())
            .body(b.bytes.clone());
    }
    let fut = req.send();
    tokio::pin!(fut);
    let mut resp = tokio::select! {
        r = &mut fut => r,
        () = tokio::time::sleep(env.wait_notice) => {
            eprintln!("waiting: the request is slow or held for approval (07 §Approvals)");
            fut.await
        }
    }
    .map_err(|e| format!("request failed: {}", oneline(&e.to_string(), 300)))?;
    let status = resp.status().as_u16();
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let aurora_error = resp
        .headers()
        .get("x-aurora-error")
        .and_then(|v| v.to_str().ok())
        .map(|v| oneline(v, 64));
    let link_next = resp
        .headers()
        .get_all("link")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(next_link);
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .map(ToOwned::to_owned);
    let mut body = Vec::new();
    let mut cut = false;
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("reading the response: {e}"))?
    {
        if body.len() + chunk.len() > AGENT_CLI_SAVE_MAX {
            body.extend_from_slice(&chunk[..AGENT_CLI_SAVE_MAX - body.len()]);
            cut = true;
            break;
        }
        body.extend_from_slice(&chunk);
    }
    if (300..400).contains(&status)
        && let Some(l) = location
    {
        return Err(format!(
            "HTTP {status}: redirect to {} not followed (aurora-agent never follows redirects)",
            oneline(&l, 200)
        ));
    }
    Ok(Page {
        status,
        content_type,
        link_next,
        body,
        cut,
        aurora_error,
    })
}

/// The `rel="next"` target of one `Link` header value.
fn next_link(h: &str) -> Option<String> {
    h.split(',').find_map(|part| {
        let part = part.trim();
        let url = part.strip_prefix('<')?.split_once('>')?;
        let rel_next =
            url.1.split(';').map(str::trim).any(|p| {
                p.eq_ignore_ascii_case("rel=\"next\"") || p.eq_ignore_ascii_case("rel=next")
            });
        rel_next.then(|| url.0.to_owned())
    })
}

/// The gateway's error envelope (`06 §Errors`): exactly `{"error":{"code","message"[,"details"]}}`.
fn gateway_refusal(status: u16, body: &[u8]) -> Option<(String, String, Option<Value>)> {
    if status < 400 {
        return None;
    }
    let v: Value = serde_json::from_slice(body).ok()?;
    let top = v.as_object()?;
    let e = top.get("error")?.as_object()?;
    if top.len() != 1
        || e.keys()
            .any(|k| !matches!(k.as_str(), "code" | "message" | "details"))
    {
        return None;
    }
    Some((
        e.get("code")?.as_str()?.to_owned(),
        e.get("message")?.as_str()?.to_owned(),
        e.get("details").cloned(),
    ))
}

fn failure(page: &Page) -> Outcome {
    // A refusal is the gateway's when it has the `06 §Errors` body, or says so in `X-Aurora-Error`.
    let refusal = gateway_refusal(page.status, &page.body).or_else(|| {
        page.aurora_error.as_ref().map(|c| {
            let message = serde_json::from_slice::<Value>(&page.body)
                .ok()
                .and_then(|v| v["error"]["message"].as_str().map(str::to_owned))
                .unwrap_or_default();
            (c.clone(), message, None)
        })
    });
    if let Some((code, message, details)) = refusal {
        let mut s = format!(
            "refused by the Aurora gateway (HTTP {}, {}): {}\n",
            page.status,
            oneline(&code, 64),
            oneline(&message, 600)
        );
        if let Some(d) = &details {
            s.push_str(&format!("details: {}\n", oneline(&d.to_string(), 800)));
            if let Some(m) = d.get("may_have_been_delivered").and_then(Value::as_bool) {
                s.push_str(&format!("may_have_been_delivered: {m}\n"));
            }
        }
        return Outcome::fail(EXIT_GATEWAY, bound(&s));
    }
    let text = String::from_utf8_lossy(&page.body);
    let lines: Vec<String> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(10)
        .map(|l| oneline(l, 200))
        .collect();
    Outcome::fail(
        EXIT_UPSTREAM,
        bound(&format!(
            "HTTP {} from the app\n{}\n",
            page.status,
            lines.join("\n")
        )),
    )
}

fn origin_ok(first: &Url, next: &Url) -> bool {
    first.origin() == next.origin() && next.fragment().is_none()
}

/// Runs `call` for an app target.
pub async fn call_app(
    env: &Env,
    apps_dir: &Path,
    target: &str,
    name: Option<&str>,
    args: &[String],
    opts: &CallOpts,
) -> Outcome {
    let loaded = super::api::load_all(apps_dir);
    let Some(folder) = loaded.iter().find(|f| f.id == target) else {
        return Outcome::usage(format!(
            "unknown target {target:?}; `aurora-agent list` shows the connected apps"
        ));
    };
    let doc = match &folder.api {
        Ok(d) => d,
        Err(e) => return Outcome::usage(format!("{target}: {e}")),
    };
    match call_doc(env, doc, name, args, opts).await {
        Ok(o) | Err(o) => o,
    }
}

#[allow(clippy::too_many_lines)]
async fn call_doc(
    env: &Env,
    doc: &ApiDoc,
    name: Option<&str>,
    args: &[String],
    opts: &CallOpts,
) -> Result<Outcome, Outcome> {
    let usage = |e: String| Outcome::usage(e);
    let max_pages = opts.max_pages.unwrap_or(AGENT_CLI_PAGES_DEFAULT);
    if max_pages == 0 || max_pages > AGENT_CLI_PAGES_MAX {
        return Err(usage(format!(
            "--max-pages must be 1 to {AGENT_CLI_PAGES_MAX}"
        )));
    }
    if opts.max_pages.is_some() && !opts.all {
        return Err(usage("--max-pages needs --all".into()));
    }
    let parsed = request::parse_args(args).map_err(usage)?;
    let op: Option<&Operation> = match name {
        Some(n) => Some(doc.operation(n).ok_or_else(|| {
            usage(format!(
                "{}: no operation {:?}; `aurora-agent search <words> --in {}` finds it",
                doc.app.id,
                oneline(n, 64),
                doc.app.id
            ))
        })?),
        None => None,
    };
    if op.is_none() && opts.path.is_none() {
        return Err(usage(
            "call <app> <operation>, or call <app> --path <path>".into(),
        ));
    }
    if op.is_some_and(Operation::is_catalogue) && opts.path.is_none() {
        return Err(usage(format!(
            "{} is a catalogue entry (pattern {}): call it with an explicit --path",
            name.unwrap_or_default(),
            op.map_or("", |o| o.path.as_str())
        )));
    }
    let bound_to = op
        .filter(|_| opts.path.is_none())
        .and_then(|o| o.account.as_deref());
    let account = pick_account(doc, opts.account.as_deref(), bound_to).map_err(usage)?;
    let whole_body = opts
        .body
        .as_deref()
        .map(read_body)
        .transpose()
        .map_err(usage)?;
    let built = match (&opts.path, op) {
        (Some(p), o) => {
            let method = opts
                .method
                .clone()
                .or_else(|| o.map(|o| o.method.clone()))
                .unwrap_or_else(|| "GET".into());
            request::build_adhoc(p, &method, &parsed, whole_body).map_err(usage)?
        }
        (None, Some(o)) => {
            if opts.method.is_some() {
                return Err(usage("--method applies with --path only".into()));
            }
            request::build_operation(o, &parsed, whole_body).map_err(usage)?
        }
        (None, None) => unreachable!("checked above"),
    };
    let host_base = match op
        .and_then(|o| o.host.as_deref())
        .filter(|_| opts.path.is_none())
    {
        Some(h) => format!("https://{h}"),
        None => account.base_url.clone().ok_or_else(|| {
            usage(format!(
                "account {} has no known host; read .aurora/apps/{}/guide.md",
                account.letter, doc.app.id
            ))
        })?,
    };
    let base = Url::parse(&host_base).map_err(|e| usage(format!("host {host_base:?}: {e}")))?;
    if base.path() != "/" || base.query().is_some() || !matches!(base.scheme(), "https" | "http") {
        return Err(usage(format!("host {host_base:?} is not a plain origin")));
    }
    let out_path = match &opts.out {
        Some(p) => Some(files::check_out(p, &env.cwd, &env.protected).map_err(usage)?),
        None => None,
    };
    let pagination = op.and_then(|o| o.pagination.clone());
    let items_path = op.and_then(|o| o.items.clone());
    let mut query = built.query.clone();
    if opts.all {
        match &pagination {
            Some(Pagination::PageParam { param, start, .. })
                if !query.iter().any(|(k, _)| k == param) =>
            {
                set_query(&mut query, param, start.unwrap_or(1).to_string());
            }
            _ => {}
        }
    }
    let build_url = |q: &[(String, String)]| -> Result<Url, Outcome> {
        let u = Url::parse(&format!(
            "{}{}{}",
            host_base.trim_end_matches('/'),
            built.path,
            request::query_string(q)
        ))
        .map_err(|e| usage(format!("url: {e}")))?;
        if !origin_ok(&base, &u) {
            return Err(usage(
                "the request would leave the account's host; refused".into(),
            ));
        }
        Ok(u)
    };
    let mut url = build_url(&query)?;
    let mut work = built.clone();
    work.query = query;

    let mut pages = 0usize;
    let mut joined: Vec<Value> = Vec::new();
    let mut joinable = opts.all;
    let mut notes: Vec<String> = Vec::new();
    let mut first: Option<Page> = None;
    let mut more: Option<bool> = None;
    let mut saved_bytes = 0usize;
    loop {
        let page = send(env, &url, &work, &account.connection_id)
            .await
            .map_err(|e| Outcome::fail(EXIT_UPSTREAM, e))?;
        if page.status >= 400 || (300..400).contains(&page.status) {
            return Err(failure(&page));
        }
        pages += 1;
        saved_bytes += page.body.len();
        let parsed_json: Option<Value> = if page.cut {
            None
        } else {
            serde_json::from_slice(&page.body).ok()
        };
        let mut next: Option<(Url, Vec<(String, String)>)> = None;
        match (&pagination, &parsed_json) {
            (Some(p), Some(v)) => {
                let items = jsonpath::get(v, items_path.as_deref().unwrap_or("$"));
                let nonempty = items
                    .and_then(Value::as_array)
                    .is_some_and(|a| !a.is_empty());
                match p {
                    Pagination::LinkHeader => {
                        if let Some(l) = &page.link_next {
                            match url.join(l) {
                                Ok(n) if origin_ok(&base, &n) => {
                                    next = Some((n, work.query.clone()))
                                }
                                Ok(_) => notes
                                    .push("a next link to another origin was not followed".into()),
                                Err(_) => notes.push("a next link was not a URL".into()),
                            }
                        }
                    }
                    Pagination::PageParam { param, start, .. } => {
                        if nonempty {
                            let cur = work
                                .query
                                .iter()
                                .find(|(k, _)| k == param)
                                .and_then(|(_, v)| v.parse::<u64>().ok())
                                .unwrap_or_else(|| u64::from(start.unwrap_or(1)));
                            let mut q = work.query.clone();
                            set_query(&mut q, param, (cur + 1).to_string());
                            if let Ok(n) = build_url(&q) {
                                next = Some((n, q));
                            }
                        }
                    }
                    Pagination::Cursor { param, next: path } => {
                        if let Some(c) = jsonpath::get(v, path)
                            .and_then(Value::as_str)
                            .filter(|c| !c.is_empty())
                        {
                            let mut q = work.query.clone();
                            set_query(&mut q, param, c.to_owned());
                            if let Ok(n) = build_url(&q) {
                                next = Some((n, q));
                            }
                        }
                    }
                }
                if opts.all {
                    match items.and_then(Value::as_array) {
                        Some(a) => joined.extend(a.iter().cloned()),
                        None => {
                            joinable = false;
                            notes.push("pages were not joined: no list at the items path".into());
                            next = None;
                        }
                    }
                }
            }
            (None, Some(_)) if opts.all => {
                joinable = false;
                notes.push("the operation declares no pagination: one page fetched".into());
            }
            (_, None) if opts.all => {
                joinable = false;
                notes.push("pages were not joined: the response is not JSON (or was cut)".into());
            }
            _ => {}
        }
        let cut = page.cut;
        if pages == 1 {
            first = Some(page);
        }
        if !opts.all {
            more = Some(next.is_some());
            break;
        }
        if cut || saved_bytes >= AGENT_CLI_SAVE_MAX {
            notes.push("stopped at the save size limit".into());
            break;
        }
        match next {
            Some((n, q)) if pages < max_pages as usize => {
                url = n;
                work.query = q;
            }
            Some(_) => {
                notes.push(format!(
                    "stopped at --max-pages {max_pages}; more pages exist"
                ));
                more = Some(true);
                break;
            }
            None => {
                more = Some(false);
                break;
            }
        }
    }
    let first = first.ok_or_else(|| Outcome::fail(EXIT_UPSTREAM, "no response"))?;
    let (bytes, json_ok): (Vec<u8>, bool) = if opts.all && joinable {
        let b = serde_json::to_vec(&Value::Array(joined)).unwrap_or_default();
        (b, true)
    } else {
        (
            first.body.clone(),
            !first.cut && serde_json::from_slice::<Value>(&first.body).is_ok(),
        )
    };
    if first.cut && !opts.all {
        notes.push(format!(
            "the response was cut at {AGENT_CLI_SAVE_MAX} bytes"
        ));
    }
    if opts.raw {
        if let Some(p) = &out_path {
            files::save(
                &bytes,
                Some(p),
                &env.out_dir,
                &doc.app.id,
                name.unwrap_or("path"),
                "json",
            )
            .map_err(|e| Outcome::fail(EXIT_UPSTREAM, e))?;
        }
        return Ok(Outcome {
            code: EXIT_OK,
            out: bytes,
            err: String::new(),
        });
    }
    let ext = files::extension(&first.content_type, json_ok);
    let saved = files::save(
        &bytes,
        out_path.as_deref(),
        &env.out_dir,
        &doc.app.id,
        name.unwrap_or("path"),
        ext,
    )
    .map_err(|e| Outcome::fail(EXIT_UPSTREAM, e))?;
    let saved_s = saved.display().to_string();
    let ct = if json_ok && opts.all {
        "application/json"
    } else {
        first.content_type.as_str()
    };
    let summary = Summary {
        status: first.status,
        content_type: ct,
        bytes: bytes.len(),
        pages: Some(pages),
        saved: Some(&saved_s),
        more,
        notes: &notes,
    };
    Ok(Outcome::ok(preview::render(&summary, &bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_link_is_read_from_a_link_header() {
        let h =
            r#"<https://api.test/x?page=1>; rel="prev", <https://api.test/x?page=3>; rel="next""#;
        assert_eq!(next_link(h).as_deref(), Some("https://api.test/x?page=3"));
        assert_eq!(next_link(r#"<https://a>; rel="last""#), None);
    }

    #[test]
    fn only_the_gateways_envelope_is_a_refusal() {
        let g = br#"{"error":{"code":"blocked_by_policy","message":"no","details":{"a":1}}}"#;
        assert!(gateway_refusal(403, g).is_some());
        assert!(gateway_refusal(200, g).is_none());
        assert!(
            gateway_refusal(403, br#"{"error":{"code":"x","message":"m","extra":1}}"#).is_none()
        );
        assert!(gateway_refusal(403, br#"{"error":"nope"}"#).is_none());
        assert!(gateway_refusal(500, br#"{"message":"upstream"}"#).is_none());
    }
}
