//! (phase 5, P5-WEBHOOK-2, D-320, C-910, SEC-127) The send half of the secret-link lane: an
//! admitted, approved, rate-taken and rendered outgoing-webhook request is posted to exactly the
//! stored URL with the configured method, headers and body. The agent's own headers never go
//! upstream, a send is never retried and never cached, redirects follow C-910 (`307`/`308` resend
//! the method and body with a fresh signature, `301`/`302`/`303` continue as a body-less `GET`,
//! at most [`REDIRECTS_MAX`] `https` hops, destination guard and egress policy per hop), the
//! receiver's answer is capped and checked for an echo of the URL or any secret, and a gateway
//! refusal carries `X-Aurora-Error`. The URL, the secret values and the signing secret appear in no
//! log row, span or error.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aurora_core::ApiError;
use aurora_core::limits::{WEBHOOK_RESPONSE_MAX, WEBHOOK_TIMEOUT};
use bytes::{Bytes, BytesMut};
use http::{HeaderMap, HeaderName, HeaderValue, Method, Response, StatusCode, Uri, header};
use http_body_util::BodyExt;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::json;
use uuid::Uuid;

use crate::pipeline::{PreparedRequest, Scheme};
use crate::seclink::{Dest, LinkFetch, Next, REDIRECTS_MAX, follow, needles, parse_dest, refused};
use crate::upstream::Upstream;
use crate::{Body, empty_body, full_body, guidance};

/// The shortest secret value that is searched for in the answer (a one-letter value would match
/// everything).
const SECRET_NEEDLE_MIN: usize = 6;

/// A send-mode request, rendered and ready to post.
pub struct SendPlan {
    pub method: Method,
    pub content_type: String,
    pub body: Bytes,
    /// The configured headers (never the agent's), name and value.
    pub headers: Vec<(String, String)>,
    /// The signing secret when signing is on; sealed in the credential, never logged.
    pub signing_secret: Option<String>,
    /// Every secret value, for the echo check.
    pub secret_values: Vec<String>,
    pub delivery_id: Uuid,
}

impl std::fmt::Debug for SendPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SendPlan")
            .field("delivery_id", &self.delivery_id)
            .finish_non_exhaustive()
    }
}

/// What the receiver answered, bounded.
struct Reply {
    status: StatusCode,
    content_type: Option<HeaderValue>,
    retry_after: Option<HeaderValue>,
    body: Bytes,
    truncated: bool,
}

/// A refusal of the gateway's own: the code and whether the receiver may have the message.
struct Fail {
    err: ApiError,
}

fn fail(
    status: StatusCode,
    code: &'static str,
    msg: &'static str,
    delivered: Option<bool>,
) -> Fail {
    let e = ApiError::new(status, code, msg);
    Fail {
        err: match delivered {
            Some(d) => e.with_details(json!({ "may_have_been_delivered": d })),
            None => e,
        },
    }
}

fn unreachable(delivered: bool) -> Fail {
    fail(
        StatusCode::BAD_GATEWAY,
        "link_unreachable",
        if delivered {
            "The connection to the webhook broke after the message was sent: it may have been delivered."
        } else {
            "The webhook's server could not be reached. Nothing was sent."
        },
        Some(delivered),
    )
}

fn timed_out() -> Fail {
    fail(
        StatusCode::GATEWAY_TIMEOUT,
        "webhook_timeout",
        "The webhook did not answer in time. The message may have been delivered: do not send it again without asking the person.",
        Some(true),
    )
}

fn echoed() -> Fail {
    fail(
        StatusCode::BAD_GATEWAY,
        "upstream_echoed_credential",
        "The receiver answered with the webhook's address or a secret, so the response was withheld.",
        None,
    )
}

/// The needles of the echo check (SEC-127): the URL, its path-with-query (raw and percent-encoded)
/// and every secret value, the signing secret included.
fn all_needles(url: &str, first: &Dest, plan: &SendPlan) -> Vec<Vec<u8>> {
    let mut out = needles(url, first);
    let secrets = plan
        .secret_values
        .iter()
        .chain(plan.signing_secret.iter())
        .filter(|s| s.len() >= SECRET_NEEDLE_MIN);
    for s in secrets {
        for n in [
            s.as_bytes().to_vec(),
            utf8_percent_encode(s, NON_ALPHANUMERIC)
                .to_string()
                .into_bytes(),
        ] {
            if !out.contains(&n) {
                out.push(n);
            }
        }
    }
    out
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn same_origin(a: &Dest, b: &Dest) -> bool {
    a.host.eq_ignore_ascii_case(&b.host) && a.port == b.port
}

/// One request of the chain.
fn request_headers(plan: &SendPlan, with_body: bool, own_origin: bool) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(
        header::USER_AGENT,
        HeaderValue::from_static("Aurora-Webhook/1"),
    );
    if let Ok(v) = HeaderValue::from_str(&plan.delivery_id.to_string()) {
        h.insert("x-aurora-delivery", v);
    }
    if with_body {
        if let Ok(v) = HeaderValue::from_str(&plan.content_type) {
            h.insert(header::CONTENT_TYPE, v);
        }
        if let Some(secret) = &plan.signing_secret {
            let sig = aurora_webhook::sign(secret.as_bytes(), unix_now(), &plan.body);
            if let Ok(v) = HeaderValue::from_str(&sig) {
                h.insert("x-aurora-signature", v);
            }
        }
    }
    // The configured headers may carry a secret (a bearer token): they go to the stored origin only,
    // never to a host a redirect names.
    if own_origin {
        for (n, v) in &plan.headers {
            if let (Ok(n), Ok(v)) = (
                HeaderName::from_bytes(n.as_bytes()),
                HeaderValue::from_str(v),
            ) {
                h.insert(n, v);
            }
        }
    }
    h
}

async fn post(
    up: &Upstream,
    url: &str,
    plan: &SendPlan,
    hops: &dyn crate::seclink::Hops,
) -> Result<Reply, Fail> {
    let first = url
        .parse::<Uri>()
        .ok()
        .and_then(|u| parse_dest(&u))
        .ok_or_else(|| Fail { err: refused("") })?;
    let needles = all_needles(url, &first, plan);
    let mut cur = first.clone();
    let mut method = plan.method.clone();
    let mut with_body = true;
    let mut answered = false;
    for hop in 0..=REDIRECTS_MAX {
        hops.check(&cur.host, cur.port)
            .map_err(|e| Fail { err: e })?;
        let body = if with_body {
            full_body(plan.body.clone())
        } else {
            empty_body()
        };
        let resp = up
            .send_ex(PreparedRequest {
                scheme: Scheme::Https,
                host: cur.host.clone(),
                port: cur.port,
                method: method.clone(),
                path: cur.path.clone(),
                query: cur.query.clone(),
                headers: request_headers(plan, with_body, same_origin(&cur, &first)),
                upgrade: None,
                body,
                hook: None,
                response_headers: HeaderMap::new(),
                retry: None,
                ldap: None,
                link: None,
                app_host: None,
            })
            .await
            .map_err(|(e, nothing_sent)| match e.status() {
                StatusCode::FORBIDDEN => Fail {
                    err: refused(&cur.host),
                },
                StatusCode::GATEWAY_TIMEOUT => timed_out(),
                _ => unreachable(answered || !nothing_sent),
            })?;
        answered = true;
        let status = resp.status();
        if matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308) {
            // The redirect and its `Location` never reach the agent.
            let loc = resp
                .headers()
                .get(header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            drop(resp);
            if hop == REDIRECTS_MAX {
                return Err(fail(
                    StatusCode::BAD_GATEWAY,
                    "link_too_many_redirects",
                    "The webhook redirects more often than the gateway follows.",
                    Some(true),
                ));
            }
            let Some(loc) = loc else {
                return Err(unreachable(true));
            };
            match follow(&cur, &loc) {
                Next::Go(d) => cur = d,
                Next::Refused => {
                    return Err(Fail {
                        err: refused(&cur.host),
                    });
                }
            }
            if (301..=303).contains(&status.as_u16()) {
                method = Method::GET;
                with_body = false;
            }
            continue;
        }
        if status == StatusCode::NOT_FOUND || status == StatusCode::GONE {
            return Err(fail(
                StatusCode::NOT_FOUND,
                "link_not_found",
                "The webhook no longer exists: it was probably deleted or regenerated. Ask the person to reconnect with the new URL.",
                Some(true),
            ));
        }
        let content_type = resp.headers().get(header::CONTENT_TYPE).cloned();
        let retry_after = resp.headers().get(header::RETRY_AFTER).cloned();
        if resp
            .headers()
            .values()
            .any(|v| crate::inject::echoes(&needles, v.as_bytes()))
        {
            return Err(echoed());
        }
        let mut stream = resp.into_body();
        let mut buf = BytesMut::new();
        let mut truncated = false;
        while let Some(frame) = stream.frame().await {
            let Ok(frame) = frame else { break };
            if let Some(d) = frame.data_ref() {
                let room = WEBHOOK_RESPONSE_MAX.saturating_sub(buf.len());
                if d.len() > room {
                    buf.extend_from_slice(&d[..room]);
                    truncated = true;
                    break;
                }
                buf.extend_from_slice(d);
            }
        }
        if crate::inject::echoes(&needles, &buf) {
            return Err(echoed());
        }
        return Ok(Reply {
            status,
            content_type,
            retry_after,
            body: buf.freeze(),
            truncated,
        });
    }
    Err(unreachable(true))
}

fn refusal_response(f: &Fail, delivery: &Uuid) -> Response<Body> {
    let mut r = guidance::response(&f.err);
    if let Ok(v) = HeaderValue::from_str(f.err.code()) {
        r.headers_mut().insert("x-aurora-error", v);
    }
    if let Ok(v) = HeaderValue::from_str(&delivery.to_string()) {
        r.headers_mut().insert("x-aurora-delivery", v);
    }
    r
}

/// Posts an admitted send and answers the agent; the request-log row is written here, once.
pub async fn answer(_up: &Upstream, f: LinkFetch, plan: SendPlan) -> Response<Body> {
    answer_with(_up, f, plan, WEBHOOK_TIMEOUT).await
}

/// [`answer`] with an explicit time-out (tests use a short one).
pub async fn answer_with(
    up: &Upstream,
    f: LinkFetch,
    plan: SendPlan,
    timeout: Duration,
) -> Response<Body> {
    let LinkFetch {
        url, hops, report, ..
    } = f;
    let delivery = plan.delivery_id;
    let result = tokio::time::timeout(timeout, post(up, &url, &plan, &*hops))
        .await
        .unwrap_or_else(|_| Err(timed_out()));
    match result {
        Err(fl) => {
            report.finish(fl.err.status().as_u16(), None, Some(fl.err.code()));
            refusal_response(&fl, &delivery)
        }
        Ok(reply) => {
            let mut r = Response::new(full_body(reply.body));
            *r.status_mut() = reply.status;
            let h = r.headers_mut();
            if let Some(ct) = reply.content_type {
                h.insert(header::CONTENT_TYPE, ct);
            }
            if let Some(ra) = reply.retry_after {
                h.insert(header::RETRY_AFTER, ra);
            }
            if let Ok(v) = HeaderValue::from_str(&delivery.to_string()) {
                h.insert("x-aurora-delivery", v);
            }
            if reply.truncated {
                h.insert("x-aurora-truncated", HeaderValue::from_static("1"));
            }
            report.finish(reply.status.as_u16(), Some(reply.status.as_u16()), None);
            r
        }
    }
}
