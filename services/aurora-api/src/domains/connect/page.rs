//! Tiny HTML pages for the OAuth callback (C-14).
use super::flow;
use axum::response::{IntoResponse, Response};
use http::{HeaderValue, StatusCode, header};

fn respond(body: String, nonce: Option<&str>) -> Response {
    let csp = match nonce {
        Some(n) => format!("default-src 'none'; script-src 'nonce-{n}'"),
        None => "default-src 'none'".to_owned(),
    };
    let mut r = (StatusCode::OK, body).into_response();
    let h = r.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    if let Ok(v) = HeaderValue::from_str(&csp) {
        h.insert(header::CONTENT_SECURITY_POLICY, v);
    }
    // The flow is finished: drop the binding cookie.
    if let Ok(v) = HeaderValue::from_str(&flow::cookie("", 0)) {
        h.append(header::SET_COOKIE, v);
    }
    r
}

/// Success: post `aurora:connected` to the opener (target = external origin) and close.
pub fn connected(connection_id: uuid::Uuid, missing_scopes: &[String], origin: &str) -> Response {
    let nonce = flow::random(16);
    let missing_js = serde_json::to_string(missing_scopes)
        .unwrap_or_else(|_| "[]".into())
        .replace('<', "\\u003c");
    let note = if missing_scopes.is_empty() {
        String::new()
    } else {
        let list = missing_scopes
            .iter()
            .map(|s| html_escape(s))
            .collect::<Vec<_>>()
            .join(", ");
        format!("<p>Warning: these permissions were not granted: {list}.</p>")
    };
    let origin_js = serde_json::to_string(origin).unwrap_or_else(|_| "\"\"".into());
    respond(
        format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><title>Connected</title></head><body>\
             <p>Connected. You can close this window.</p>{note}\
             <script nonce=\"{nonce}\">try{{window.opener&&window.opener.postMessage(\
             {{type:\"aurora:connected\",connection_id:\"{connection_id}\",missing_scopes:{missing_js}}},{origin_js})}}catch(e){{}}if({missing_js}.length===0)window.close();</script>\
             </body></html>"
        ),
        Some(&nonce),
    )
}

/// Failure: the page carries `error=<code>` (codes are fixed identifiers).
pub fn error(code: &str) -> Response {
    respond(
        format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><title>Connection failed</title>\
             <meta name=\"aurora-error\" content=\"{code}\"></head><body>\
             <p>The connection could not be completed.</p><p>error={code}</p></body></html>"
        ),
        None,
    )
}

/// Fragment-mode bridge: re-submits the URL fragment as a query string.
pub fn bridge() -> Response {
    let nonce = flow::random(16);
    let mut r = respond(
        format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><title>Connecting</title></head><body>\
             <script nonce=\"{nonce}\">var h=location.hash.slice(1);\
             if(h){{location.replace(location.pathname+\"?\"+h)}}\
             else{{location.replace(location.pathname+\"?error=oauth_state_invalid\")}}</script></body></html>"
        ),
        Some(&nonce),
    );
    // The bridge must keep the cookie for the follow-up request.
    r.headers_mut().remove(header::SET_COOKIE);
    r
}

fn html_escape(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '<' => "&lt;".to_owned(),
            '>' => "&gt;".to_owned(),
            '&' => "&amp;".to_owned(),
            '"' => "&quot;".to_owned(),
            '\'' => "&#39;".to_owned(),
            c => c.to_string(),
        })
        .collect()
}
