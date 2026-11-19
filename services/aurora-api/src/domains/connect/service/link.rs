//! (phase 5, P5-ICS, D-273, C-843, SEC-107) Connect of a secret-link app: the person enters a name
//! and the full URL. The URL is validated (`https`, a DNS host, ≤ 2,048 characters; `webcal://`
//! is normalized), fetched once through the guarded client (the destination guard, private ranges
//! only under `gateway.allowed_private`; 10 s, the first 256 KiB, ≤ 3 `https` redirects) and its
//! beginning checked against the manifest's `expect_prefix`. The URL is then stored as the
//! credential and its **host** as metadata `host`; every refusal is one of the fixed codes of
//! C-843 and none of them carries the URL, a path or a query.
use super::*;
use reqwest::Url;
use std::net::IpAddr;

/// Longest URL accepted.
const URL_MAX: usize = 2_048;
/// Bytes of the answer read for the prefix check.
const READ_MAX: usize = 256 * 1024;
/// Redirect hops followed (as the gateway does).
const HOPS_MAX: usize = aurora_core::limits::SECRET_LINK_REDIRECTS_MAX;

fn refuse(code: &'static str, msg: &'static str) -> ConnectErr {
    ConnectErr::Api(ApiError::unprocessable(code, msg))
}

fn not_https() -> ConnectErr {
    refuse(
        "link_not_https",
        "The link must start with https:// (or webcal://).",
    )
}

fn invalid_link() -> ConnectErr {
    refuse("link_invalid", "That is not a usable calendar link.")
}

fn destination_refused() -> ConnectErr {
    refuse(
        "link_destination_refused",
        "The link points at an address Aurora does not reach: a private or internal address, or one an organization policy blocks.",
    )
}

fn not_found() -> ConnectErr {
    refuse(
        "link_not_found",
        "The link's server says there is nothing there: it may be unpublished or reset. Copy the link again.",
    )
}

fn unreachable() -> ConnectErr {
    refuse(
        "link_unreachable",
        "The link's server could not be reached (no answer, a name that does not resolve, or a TLS error).",
    )
}

fn unexpected_content() -> ConnectErr {
    refuse(
        "link_unexpected_content",
        "The link does not serve what this app reads. Copy the calendar's ICS link.",
    )
}

/// The URL as the person typed it: trimmed, `webcal://` read as `https://`.
fn normalize(raw: &str, allow: &aurora_core::ranges::PrivateAllow) -> Result<Url, ConnectErr> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > URL_MAX || raw.chars().any(char::is_control) {
        return Err(invalid_link());
    }
    let lower = raw.to_ascii_lowercase();
    let text = if lower.starts_with("webcal://") {
        format!("https://{}", &raw["webcal://".len()..])
    } else {
        raw.to_owned()
    };
    let url = Url::parse(&text).map_err(|_| invalid_link())?;
    check_url(&url, allow)?;
    Ok(url)
}

/// `https`, a host, no userinfo, no IP literal the destination guard would refuse.
fn check_url(url: &Url, allow: &aurora_core::ranges::PrivateAllow) -> Result<(), ConnectErr> {
    if url.scheme() != "https" {
        return Err(if matches!(url.scheme(), "http" | "ftp" | "file") {
            not_https()
        } else {
            invalid_link()
        });
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid_link());
    }
    let Some(host) = url.host_str() else {
        return Err(invalid_link());
    };
    if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>()
        && !allow.permits(ip, host)
    {
        return Err(destination_refused());
    }
    Ok(())
}

/// What the answer's beginning must be (`expect_prefix`), leading whitespace and a BOM ignored.
fn has_prefix(body: &[u8], prefix: &str) -> bool {
    let text = String::from_utf8_lossy(body);
    text.trim_start_matches(['\u{feff}', ' ', '\t', '\r', '\n'])
        .starts_with(prefix)
}

/// Fetch the link once and check it; the connect form's `name` becomes the label.
pub(super) async fn check(ctx: &Ctx<'_>, g: &mut Gathered) -> Result<(), ConnectErr> {
    let Some(sl) = ctx.app.secret_link.as_ref() else {
        return Ok(());
    };
    let env = &ctx.state.connect;
    let allow = env.allow.load_full();
    let raw = g
        .credentials
        .get(&sl.credential)
        .and_then(Value::as_str)
        .ok_or_else(invalid_link)?
        .to_owned();
    let url = normalize(&raw, &allow)?;
    // (P5-WEBHOOK-2, C-909) An outgoing webhook acts when it is called, so connect sends nothing:
    // the URL's form and its destination are checked (the name must resolve, and only to addresses
    // the destination guard permits), then it is stored.
    if sl.mode() == aurora_registry::manifest::SecretLinkMode::Send {
        if env.override_origin.is_none()
            && let Some(host) = url.host_str()
            && host.trim_matches(['[', ']']).parse::<IpAddr>().is_err()
        {
            match env.check_destination(host).await {
                Ok(()) => {}
                Err(super::super::env::ResolveError::Refused) => return Err(destination_refused()),
                Err(super::super::env::ResolveError::NotFound) => return Err(invalid_link()),
            }
        }
        g.metadata.insert(
            "host".into(),
            Value::String(url.host_str().unwrap_or_default().to_ascii_lowercase()),
        );
        g.credentials.insert(
            sl.credential.clone(),
            Value::String(url.as_str().to_owned()),
        );
        if let Some(name) = g.fields.get("name").and_then(Value::as_str) {
            g.label = Some(name.trim().to_owned());
        }
        return Ok(());
    }
    let mut cur = url.clone();
    let mut body: Option<Vec<u8>> = None;
    for hop in 0..=HOPS_MAX {
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
            .get(target)
            .header(
                reqwest::header::ACCEPT,
                "text/calendar, text/plain;q=0.8, */*;q=0.5",
            )
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
            check_url(&next, &allow)?;
            cur = next;
            continue;
        }
        if matches!(status.as_u16(), 401 | 403 | 404 | 410) {
            return Err(not_found());
        }
        if !status.is_success() {
            return Err(unreachable());
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
        body = Some(buf);
        break;
    }
    let body = body.ok_or_else(unreachable)?;
    if let Some(prefix) = &sl.expect_prefix
        && !has_prefix(&body, prefix)
    {
        return Err(unexpected_content());
    }
    // Stored: the normalized URL as the credential, its host as (non-secret) metadata.
    g.metadata.insert(
        "host".into(),
        Value::String(url.host_str().unwrap_or_default().to_ascii_lowercase()),
    );
    g.credentials.insert(
        sl.credential.clone(),
        Value::String(url.as_str().to_owned()),
    );
    // The connect form's `name` is the connection's label.
    if let Some(name) = g.fields.get("name").and_then(Value::as_str) {
        g.label = Some(name.trim().to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aurora_core::ranges::PrivateAllow;

    fn code(r: Result<Url, ConnectErr>) -> String {
        match r {
            Ok(_) => "ok".into(),
            Err(e) => ApiError::from(e).code().to_owned(),
        }
    }

    #[test]
    fn urls_are_validated_and_webcal_is_normalized() {
        let none = PrivateAllow::new(&[]);
        let ok = normalize("  webcal://cal.example/a/b.ics?x=1 ", &none)
            .map_err(|_| ())
            .unwrap();
        assert_eq!(ok.as_str(), "https://cal.example/a/b.ics?x=1");
        assert_eq!(
            code(normalize("http://cal.example/x.ics", &none)),
            "link_not_https"
        );
        assert_eq!(
            code(normalize("ftp://cal.example/x.ics", &none)),
            "link_not_https"
        );
        assert_eq!(
            code(normalize("javascript:alert(1)", &none)),
            "link_invalid"
        );
        assert_eq!(code(normalize("", &none)), "link_invalid");
        assert_eq!(code(normalize("not a url", &none)), "link_invalid");
        assert_eq!(
            code(normalize("https://u:p@cal.example/x", &none)),
            "link_invalid"
        );
        assert_eq!(
            code(normalize(
                &format!("https://cal.example/{}", "a".repeat(2_048)),
                &none
            )),
            "link_invalid"
        );
        // An IP literal only where `allowed_private` covers it.
        assert_eq!(
            code(normalize("https://10.1.2.3/x.ics", &none)),
            "link_destination_refused"
        );
        assert_eq!(
            code(normalize(
                "https://169.254.169.254/x",
                &PrivateAllow::new(&["10.0.0.0/8".into()])
            )),
            "link_destination_refused"
        );
        assert_eq!(
            code(normalize(
                "https://10.1.2.3/x.ics",
                &PrivateAllow::new(&["10.0.0.0/8".into()])
            )),
            "ok"
        );
    }

    #[test]
    fn the_prefix_check_ignores_leading_whitespace_and_a_bom() {
        assert!(has_prefix(
            b"\xef\xbb\xbf\r\nBEGIN:VCALENDAR\r\n",
            "BEGIN:VCALENDAR"
        ));
        assert!(!has_prefix(b"<html>BEGIN:VCALENDAR", "BEGIN:VCALENDAR"));
    }
}
