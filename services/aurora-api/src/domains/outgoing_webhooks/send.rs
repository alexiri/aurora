//! (P5-WEBHOOK-2, C-909, C-910) The API's half of the upstream request of a send test: the same
//! rules as the gateway's (`webhook_send`) over the API's guarded client: the destination guard
//! (private ranges only under `gateway.allowed_private`), `https` only, at most
//! [`REDIRECTS_MAX`] hops (`307`/`308` resend the method and body with a fresh signature,
//! `301`/`302`/`303` continue as a body-less `GET`), no retry, `WEBHOOK_TIMEOUT`, the configured
//! headers only to the stored origin. The body, headers and signature come from `aurora-webhook`.
use std::time::{SystemTime, UNIX_EPOCH};

use aurora_core::ApiError;
use aurora_core::limits::{
    SECRET_LINK_REDIRECTS_MAX as REDIRECTS_MAX, WEBHOOK_RESPONSE_MAX, WEBHOOK_TIMEOUT,
};
use aurora_webhook::{Rendered, WebhookConfig};
use reqwest::{StatusCode, Url};
use serde_json::json;
use uuid::Uuid;

use crate::app::AppState;

pub(crate) struct Reply {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

fn err(
    status: StatusCode,
    code: &'static str,
    msg: &'static str,
    delivered: Option<bool>,
) -> ApiError {
    let e = ApiError::new(
        http::StatusCode::from_u16(status.as_u16()).unwrap_or(http::StatusCode::BAD_GATEWAY),
        code,
        msg,
    );
    match delivered {
        Some(d) => e.with_details(json!({ "may_have_been_delivered": d })),
        None => e,
    }
}

fn refused() -> ApiError {
    err(
        StatusCode::FORBIDDEN,
        "link_destination_refused",
        "The webhook's address is not permitted: it is a private or internal address.",
        None,
    )
}

/// The URL and its path-with-query, raw and percent-encoded: what an answer must not contain.
pub(crate) fn url_needles(url: &str) -> Vec<String> {
    let mut out = vec![url.to_owned()];
    out.push(
        percent_encoding::utf8_percent_encode(url, percent_encoding::NON_ALPHANUMERIC).to_string(),
    );
    if let Ok(u) = Url::parse(url) {
        let pq = match u.query() {
            Some(q) => format!("{}?{q}", u.path()),
            None => u.path().to_owned(),
        };
        if pq.len() >= 12 {
            out.push(
                percent_encoding::utf8_percent_encode(&pq, percent_encoding::NON_ALPHANUMERIC)
                    .to_string(),
            );
            out.push(pq);
        }
    }
    out
}

fn permitted(url: &Url, state: &AppState) -> Result<(), ApiError> {
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return Err(refused());
    }
    let host = url.host_str().ok_or_else(refused)?;
    if let Ok(ip) = host.trim_matches(['[', ']']).parse::<std::net::IpAddr>()
        && !state.connect.allow.load_full().permits(ip, host)
    {
        return Err(refused());
    }
    Ok(())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

pub(crate) async fn post(
    state: &AppState,
    url: &str,
    config: &WebhookConfig,
    rendered: &Rendered,
    signing: Option<&str>,
    delivery: Uuid,
) -> Result<Reply, ApiError> {
    let first = Url::parse(url).map_err(|_| refused())?;
    permitted(&first, state)?;
    let mut cur = first.clone();
    let mut method = match config.method {
        aurora_webhook::Method::Post => reqwest::Method::POST,
        aurora_webhook::Method::Put => reqwest::Method::PUT,
    };
    let mut with_body = true;
    let mut answered = false;
    for hop in 0..=REDIRECTS_MAX {
        let target = match &state.connect.override_origin {
            Some(o) => {
                let mut t = o.clone();
                t.set_path(cur.path());
                t.set_query(cur.query());
                t
            }
            None => cur.clone(),
        };
        let own = cur.host_str() == first.host_str()
            && cur.port_or_known_default() == first.port_or_known_default();
        let mut rb = state
            .connect
            .http_instance
            .request(method.clone(), target)
            .timeout(WEBHOOK_TIMEOUT)
            .header(reqwest::header::USER_AGENT, "Aurora-Webhook/1")
            .header("X-Aurora-Delivery", delivery.to_string());
        if with_body {
            rb = rb
                .header(
                    reqwest::header::CONTENT_TYPE,
                    rendered.content_type.as_str(),
                )
                .body(rendered.body.clone());
            if let Some(s) = signing {
                rb = rb.header(
                    "X-Aurora-Signature",
                    aurora_webhook::sign(s.as_bytes(), unix_now(), &rendered.body),
                );
            }
        }
        if own {
            for (n, v) in &rendered.headers {
                rb = rb.header(n.as_str(), v.as_str());
            }
        }
        let mut resp =
            rb.send().await.map_err(
                |e| match crate::domains::connect::env::classify_send_error(&e) {
                    "destination_refused" => refused(),
                    _ if e.is_timeout() => err(
                        StatusCode::GATEWAY_TIMEOUT,
                        "webhook_timeout",
                        "The webhook did not answer in time. The message may have been delivered.",
                        Some(true),
                    ),
                    _ => err(
                        StatusCode::BAD_GATEWAY,
                        "link_unreachable",
                        "The webhook's server could not be reached.",
                        Some(answered || !e.is_connect()),
                    ),
                },
            )?;
        answered = true;
        let status = resp.status();
        if status.is_redirection() {
            let next = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|l| cur.join(l).ok());
            let Some(next) = next else {
                return Err(err(
                    StatusCode::BAD_GATEWAY,
                    "link_unreachable",
                    "The webhook redirected without a place to go.",
                    Some(true),
                ));
            };
            if hop == REDIRECTS_MAX {
                return Err(err(
                    StatusCode::BAD_GATEWAY,
                    "link_too_many_redirects",
                    "The webhook redirects more often than Aurora follows.",
                    Some(true),
                ));
            }
            permitted(&next, state)?;
            if (301..=303).contains(&status.as_u16()) {
                method = reqwest::Method::GET;
                with_body = false;
            }
            cur = next;
            continue;
        }
        if matches!(status.as_u16(), 404 | 410) {
            return Err(err(
                StatusCode::NOT_FOUND,
                "link_not_found",
                "The webhook no longer exists: it was probably deleted or regenerated.",
                Some(true),
            ));
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let mut body = Vec::new();
        while body.len() < WEBHOOK_RESPONSE_MAX {
            match resp.chunk().await {
                Ok(Some(c)) => body.extend_from_slice(&c),
                _ => break,
            }
        }
        body.truncate(WEBHOOK_RESPONSE_MAX);
        return Ok(Reply {
            status: status.as_u16(),
            content_type,
            body,
        });
    }
    Err(err(
        StatusCode::BAD_GATEWAY,
        "link_unreachable",
        "The webhook's server could not be reached.",
        Some(true),
    ))
}
