//! (phase 5, P5-CALDAV, D-275, C-845, SEC-109) Connect of an `own_server` app (CalDAV): the person
//! enters the server's URL, a username and an app password. Discovery runs through the guarded
//! client (the destination guard; private ranges only under `gateway.allowed_private`):
//! `/.well-known/caldav` (≤ 3 `https` redirects) → `current-user-principal` → `calendar-home-set`,
//! with `Authorization: Basic`. Every origin discovery touches (≤ 4) is checked **before** a
//! request with the credential goes there: a host a static package claim, a registered instance,
//! a governed LLM endpoint or an Aurora address holds is refused with `server_host_claimed`.
//! Stored: `url` = the calendar home and `hosts` = the exact `host:port` origins (metadata), the
//! username and password (credentials). Every refusal is one of the fixed `caldav_*` codes and none
//! carries the password, a URL or a response body.
use super::*;
use aurora_policy::host::HostPattern;
use aurora_registry::identify::origin_of;
use aurora_registry::snapshot::ClaimStatus;
use reqwest::{Method, Url};
use std::net::IpAddr;

const URL_MAX: usize = 2_048;
const FIELD_MAX: usize = 512;
const READ_MAX: usize = 256 * 1024;
/// Redirect hops followed per request (as the gateway does).
const HOPS_MAX: usize = aurora_core::limits::SECRET_LINK_REDIRECTS_MAX;
const PROPS: &str = r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:current-user-principal/><c:calendar-home-set/></d:prop></d:propfind>"#;

fn refuse(code: &'static str, msg: &'static str) -> ConnectErr {
    ConnectErr::Api(ApiError::unprocessable(code, msg))
}

fn not_https() -> ConnectErr {
    refuse(
        "caldav_not_https",
        "The server address must start with https://.",
    )
}

fn invalid() -> ConnectErr {
    refuse("caldav_not_caldav", "That is not a usable CalDAV address.")
}

fn destination_refused() -> ConnectErr {
    refuse(
        "caldav_destination_refused",
        "The server is at an address Aurora does not reach: a private or internal address, or one an organization policy blocks.",
    )
}

fn unreachable() -> ConnectErr {
    refuse(
        "caldav_unreachable",
        "The server could not be reached (no answer, a name that does not resolve, or a TLS error).",
    )
}

fn unauthorized() -> ConnectErr {
    refuse(
        "caldav_unauthorized",
        "The server refused the username or password. Many providers need an app-specific password.",
    )
}

fn not_caldav() -> ConnectErr {
    refuse(
        "caldav_not_caldav",
        "The server did not answer as a CalDAV server: no account principal or calendar home was found at this address.",
    )
}

fn claimed() -> ConnectErr {
    refuse(
        "server_host_claimed",
        "That host belongs to another app or to Aurora, so a personal server cannot be pointed at it.",
    )
}

fn needs_oauth() -> ConnectErr {
    refuse(
        "caldav_needs_oauth",
        "Google calendars need OAuth: connect the Google Calendar app instead.",
    )
}

fn unsupported() -> ConnectErr {
    refuse(
        "caldav_unsupported",
        "Microsoft 365 and Outlook have no CalDAV: use the calendar link (ICS) app to read and invitations to write.",
    )
}

/// A host the person should use another app for (checked before any request).
fn vendor(host: &str) -> Result<(), ConnectErr> {
    let h = host.to_ascii_lowercase();
    let is = |d: &str| h == d || h.ends_with(&format!(".{d}"));
    if is("google.com") || is("googleapis.com") || is("googleusercontent.com") {
        return Err(needs_oauth());
    }
    if is("office365.com") || is("office.com") || is("outlook.com") || is("live.com") {
        return Err(unsupported());
    }
    Ok(())
}

/// `https`, a host, no userinfo, no IP literal the destination guard would refuse.
fn check_url(url: &Url, allow: &aurora_core::ranges::PrivateAllow) -> Result<(), ConnectErr> {
    if url.scheme() != "https" {
        return Err(if matches!(url.scheme(), "http" | "ftp" | "file") {
            not_https()
        } else {
            invalid()
        });
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid());
    }
    let Some(host) = url.host_str() else {
        return Err(invalid());
    };
    if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>()
        && !allow.permits(ip, host)
    {
        return Err(destination_refused());
    }
    Ok(())
}

fn parse(raw: &str, allow: &aurora_core::ranges::PrivateAllow) -> Result<Url, ConnectErr> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > URL_MAX || raw.chars().any(char::is_control) {
        return Err(invalid());
    }
    // `caldav://`-style addresses are not a thing; a bare host is read as `https://host`.
    let text = if raw.contains("://") {
        raw.to_owned()
    } else {
        format!("https://{raw}")
    };
    let url = Url::parse(&text).map_err(|_| invalid())?;
    check_url(&url, allow)?;
    Ok(url)
}

fn origin(url: &Url) -> String {
    origin_of(
        url.host_str().unwrap_or_default(),
        url.port_or_known_default().unwrap_or(443),
    )
}

/// The text between the first `<…local>` element and its end, prefix-agnostic.
fn element<'a>(xml: &'a str, local: &str) -> Option<&'a str> {
    let mut from = 0;
    while let Some(i) = xml[from..].find('<') {
        let start = from + i + 1;
        let rest = &xml[start..];
        let end = rest.find(['>', ' ', '\t', '\r', '\n', '/'])?;
        let name = &rest[..end];
        let open = !name.starts_with(['/', '?', '!']);
        if open && name.rsplit(':').next().is_some_and(|n| n == local) {
            let tag_end = rest.find('>')?;
            if rest[..tag_end].ends_with('/') {
                return Some("");
            }
            let body = &rest[tag_end + 1..];
            let close = body.find(&format!("</{name}"))?;
            return Some(&body[..close]);
        }
        from = start;
    }
    None
}

fn unescape(s: &str) -> String {
    s.trim()
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The first `href` inside element `local` of a multistatus.
fn href_in(xml: &str, local: &str) -> Option<String> {
    let inner = element(xml, local)?;
    let h = unescape(element(inner, "href")?);
    (!h.is_empty()).then_some(h)
}

/// A response: the status, and its XML body (capped) for a 207.
struct Reply {
    status: u16,
    body: String,
}

struct Walk<'a> {
    ctx: &'a Ctx<'a>,
    allow: std::sync::Arc<aurora_core::ranges::PrivateAllow>,
    user: &'a str,
    password: &'a str,
    /// The exact origins discovery has used so far, in order.
    origins: Vec<String>,
}

impl Walk<'_> {
    /// Refuse an origin held by a package claim, a registered instance, a governed LLM endpoint or
    /// an Aurora address; record it. Runs **before** any request with the credential goes there.
    async fn admit(&mut self, url: &Url) -> Result<(), ConnectErr> {
        check_url(url, &self.allow)?;
        let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
        vendor(&host)?;
        let port = url.port_or_known_default().unwrap_or(443);
        let o = origin(url);
        if self.origins.contains(&o) {
            return Ok(());
        }
        if self.held(&host, port).await? {
            return Err(claimed());
        }
        if self.origins.len() >= aurora_registry::manifest::OWN_SERVER_HOSTS_MAX {
            return Err(not_caldav());
        }
        self.origins.push(o);
        Ok(())
    }

    async fn held(&self, host: &str, port: u16) -> Result<bool, ConnectErr> {
        let snap = self.ctx.snap;
        if snap
            .hosts
            .iter()
            .filter(|h| h.claim_status == ClaimStatus::Active)
            .any(|h| HostPattern::lenient(&h.host).matches(host, port))
            || snap
                .reserved_hosts
                .iter()
                .any(|p| HostPattern::lenient(p).matches(host, port))
            || crate::domains::plugins::ingest::reserved_names(self.ctx.state)
                .iter()
                .any(|p| HostPattern::lenient(p).matches(host, port))
        {
            return Ok(true);
        }
        let regs = app_instances::visible_on_host(&self.ctx.state.pool, None, host).await?;
        Ok(!regs.is_empty())
    }

    /// One `PROPFIND` (Depth 0), following ≤ 3 redirects; every hop is admitted first. Returns the
    /// reply and the URL that answered.
    async fn propfind(&mut self, start: &Url) -> Result<(Reply, Url), ConnectErr> {
        let env = &self.ctx.state.connect;
        let method = Method::from_bytes(b"PROPFIND").map_err(|_| invalid())?;
        let mut cur = start.clone();
        for hop in 0..=HOPS_MAX {
            self.admit(&cur).await?;
            let target = match &env.override_origin {
                Some(o) => {
                    let mut t = o.clone();
                    t.set_path(cur.path());
                    t.set_query(cur.query());
                    t
                }
                None => cur.clone(),
            };
            let mut resp = env
                .http_instance
                .request(method.clone(), target)
                .basic_auth(self.user, Some(self.password))
                .header("Depth", "0")
                .header(
                    reqwest::header::CONTENT_TYPE,
                    "application/xml; charset=utf-8",
                )
                .header(reqwest::header::ACCEPT, "application/xml, text/xml")
                .body(PROPS)
                .send()
                .await
                .map_err(|e| match super::super::env::classify_send_error(&e) {
                    "destination_refused" => destination_refused(),
                    _ => unreachable(),
                })?;
            let status = resp.status();
            if status.is_redirection() {
                let next = resp
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|l| cur.join(l).ok());
                let Some(next) = next else {
                    return Err(unreachable());
                };
                if hop == HOPS_MAX {
                    return Err(unreachable());
                }
                if next.scheme() != "https" {
                    return Err(not_https());
                }
                cur = next;
                continue;
            }
            let mut buf = Vec::new();
            while buf.len() < READ_MAX {
                match resp.chunk().await {
                    Ok(Some(c)) => buf.extend_from_slice(&c),
                    Ok(None) => break,
                    Err(_) => return Err(unreachable()),
                }
            }
            buf.truncate(READ_MAX);
            return Ok((
                Reply {
                    status: status.as_u16(),
                    body: String::from_utf8_lossy(&buf).into_owned(),
                },
                cur,
            ));
        }
        Err(unreachable())
    }
}

/// What a discovery answer means: `Ok(Some(reply))` a multistatus to read, `Ok(None)` "nothing
/// here" (try the next place), `Err` a refusal.
fn judge(r: Reply) -> Result<Option<Reply>, ConnectErr> {
    match r.status {
        401 | 403 => Err(unauthorized()),
        200 | 207 => Ok(Some(r)),
        404 | 405 | 501 => Ok(None),
        _ => Err(unreachable()),
    }
}

/// Discover the calendar home and store it, with the origins used, as metadata.
pub(super) async fn check(ctx: &Ctx<'_>, g: &mut Gathered) -> Result<(), ConnectErr> {
    let Some(os) = ctx.app.own_server.as_ref() else {
        return Ok(());
    };
    let env = &ctx.state.connect;
    let allow = env.allow.load_full();
    let text = |m: &Map<String, Value>, k: &str| -> Option<String> {
        m.get(k)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .filter(|s| !s.is_empty() && s.len() <= FIELD_MAX)
    };
    let raw = g
        .metadata
        .get(&os.url_field)
        .and_then(Value::as_str)
        .ok_or_else(invalid)?
        .to_owned();
    let user = text(&g.credentials, "username").ok_or_else(unauthorized)?;
    let password = text(&g.credentials, "password").ok_or_else(unauthorized)?;
    let start = parse(&raw, &allow)?;
    let mut w = Walk {
        ctx,
        allow: allow.clone(),
        user: &user,
        password: &password,
        origins: Vec::new(),
    };
    // The entered address is refused first (a claimed, Google or Exchange host sends nothing).
    w.admit(&start).await?;
    let mut well_known = start.clone();
    well_known.set_path("/.well-known/caldav");
    well_known.set_query(None);
    let (r, at) = w.propfind(&well_known).await?;
    let mut hit = judge(r)?.map(|r| (r, at));
    if hit.is_none() {
        // The server does not serve the well-known path: the entered address is the context.
        let (r, at) = w.propfind(&start).await?;
        hit = judge(r)?.map(|r| (r, at));
    }
    let Some((r, at)) = hit else {
        return Err(not_caldav());
    };
    let home = if let Some(h) = href_in(&r.body, "calendar-home-set") {
        at.join(&h)
    } else {
        let p = href_in(&r.body, "current-user-principal").ok_or_else(not_caldav)?;
        let principal = at.join(&p).map_err(|_| not_caldav())?;
        let (r2, at2) = w.propfind(&principal).await?;
        let r2 = judge(r2)?.ok_or_else(not_caldav)?;
        let h = href_in(&r2.body, "calendar-home-set").ok_or_else(not_caldav)?;
        at2.join(&h)
    }
    .map_err(|_| not_caldav())?;
    w.admit(&home).await?;
    let hosts = std::mem::take(&mut w.origins);
    g.metadata.insert(
        os.url_field.clone(),
        Value::String(home.as_str().to_owned()),
    );
    g.metadata.insert(
        os.hosts_field.clone(),
        Value::Array(hosts.into_iter().map(Value::String).collect()),
    );
    // The label: the person's name for the account, else `<username> at <host>`.
    let name = g
        .fields
        .get("name")
        .and_then(Value::as_str)
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty());
    g.label =
        Some(name.unwrap_or_else(|| format!("{user} at {}", home.host_str().unwrap_or_default())));
    Ok(())
}
