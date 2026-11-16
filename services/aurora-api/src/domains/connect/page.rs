//! Tiny HTML pages for the OAuth callback (C-14).
use super::flow;
use super::service::{Applied, Landing};
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

/// A JSON string literal that is safe inside an inline `<script>`.
fn js(s: &str) -> String {
    serde_json::to_string(s)
        .unwrap_or_else(|_| "\"\"".into())
        .replace('<', "\\u003c")
}

/// The line that sends the person on when no dashboard dialog took the result (C-842): a link to
/// the Connections tab of the flow's scope (a same-origin path), else to the dashboard root.
fn continue_link(landing: Option<&Landing>) -> String {
    let href = landing
        .and_then(|l| l.connections_path.as_deref())
        .unwrap_or("/");
    format!(
        "<p>You can close this window, or <a href=\"{}\">open the Connections tab</a>.</p>",
        html_escape(href)
    )
}

/// The channel script (C-842). It posts `msg` on `BroadcastChannel("aurora-connect")` (same origin
/// as the dashboard, so nothing needs a target origin and no `window.opener` is involved) and
/// closes the window only when a dashboard dialog answers `aurora:connect-ack` for this result;
/// without an answer the page stays and shows the link.
fn channel_script(nonce: &str, msg: &str, ack_key: &str, ack_val: &str) -> String {
    format!(
        "<script nonce=\"{nonce}\">try{{var m={msg};var ch=new BroadcastChannel(\"aurora-connect\");\
         ch.onmessage=function(e){{var d=e.data;if(d&&d.type===\"aurora:connect-ack\"&&d.{ack_key}===m.{ack_val}){{window.close()}}}};\
         ch.postMessage(m)}}catch(e){{}}</script>"
    )
}

/// Success: post `aurora:connected` on the channel; the dashboard dialog's ack closes this window.
pub fn connected(
    connection_id: uuid::Uuid,
    missing_scopes: &[String],
    landing: &Landing,
    applied: &Applied,
) -> Response {
    let Ok(nonce) = flow::random(16) else {
        return error("server_error", Some(landing));
    };
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
    // (P5-UX-2, C-957) The access that was written with the account, and why it was not (the
    // account is kept either way). Agent ids are UUIDs the dashboard already holds.
    let granted_js = serde_json::to_string(&applied.granted_agent_ids)
        .unwrap_or_else(|_| "[]".into())
        .replace('<', "\\u003c");
    let failed_js = match &applied.failed {
        Some(code) => format!(",grant_failed:{{code:{}}}", js(code)),
        None => String::new(),
    };
    let msg = format!(
        "{{type:\"aurora:connected\",app_id:{},connection_id:\"{connection_id}\",missing_scopes:{missing_js},granted_agent_ids:{granted_js}{failed_js}}}",
        js(&landing.app_id)
    );
    let link = continue_link(Some(landing));
    respond(
        format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><title>Connected</title></head><body>\
             <p>Connected.</p>{note}{link}{script}\
             </body></html>",
            script = channel_script(&nonce, &msg, "connection_id", "connection_id"),
        ),
        Some(&nonce),
    )
}

/// Failure: the page carries `error=<code>` (codes are fixed identifiers) and posts
/// `aurora:connect-failed` on the channel (`app_id` only when the flow is known).
pub fn error(code: &str, landing: Option<&Landing>) -> Response {
    let Ok(nonce) = flow::random(16) else {
        return respond(error_body(code, landing, ""), None);
    };
    let app = landing
        .map(|l| format!("app_id:{},", js(&l.app_id)))
        .unwrap_or_default();
    let msg = format!("{{type:\"aurora:connect-failed\",{app}error:{}}}", js(code));
    let script = channel_script(&nonce, &msg, "error", "error");
    respond(error_body(code, landing, &script), Some(&nonce))
}

fn error_body(code: &str, landing: Option<&Landing>, script: &str) -> String {
    let link = continue_link(landing);
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Connection failed</title>\
         <meta name=\"aurora-error\" content=\"{code}\"></head><body>\
         <p>The connection could not be completed.</p><p>error={code}</p>{link}{script}</body></html>"
    )
}

/// Fragment-mode bridge: re-submits the URL fragment as a query string.
pub fn bridge() -> Response {
    let Ok(nonce) = flow::random(16) else {
        return error("server_error", None);
    };
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

#[cfg(test)]
mod tests {
    use super::*;

    async fn html(r: Response) -> String {
        let body = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        String::from_utf8_lossy(&body).into_owned()
    }

    fn landing() -> Landing {
        Landing {
            app_id: "acme_app".into(),
            connections_path: Some("/o/acme/w/main/credentials?tab=connections".into()),
        }
    }

    /// (FIX-149, C-842) The pages talk over the same-origin channel, never to `window.opener`, and
    /// close only on the dashboard's ack.
    #[tokio::test]
    async fn pages_use_the_channel_and_close_only_on_the_ack() {
        let applied = Applied {
            failed: Some("not_found".into()),
            ..Applied::default()
        };
        let ok = html(connected(
            uuid::Uuid::nil(),
            &["a:b".into()],
            &landing(),
            &applied,
        ))
        .await;
        assert!(ok.contains("granted_agent_ids:[]"), "{ok}");
        assert!(ok.contains("grant_failed:{code:\"not_found\"}"), "{ok}");
        assert!(ok.contains("BroadcastChannel(\"aurora-connect\")"), "{ok}");
        assert!(ok.contains("type:\"aurora:connected\""), "{ok}");
        assert!(ok.contains("app_id:\"acme_app\""), "{ok}");
        assert!(!ok.contains("opener"), "{ok}");
        let (before_ack, after_ack) = ok.split_once("aurora:connect-ack").expect("ack handler");
        assert!(!before_ack.contains("window.close"), "{ok}");
        assert!(after_ack.contains("window.close()"), "{ok}");
        assert!(ok.contains("tab=connections"), "{ok}");
        assert!(ok.contains("a:b"), "the missing-scopes warning stays: {ok}");

        let bad = html(error("oauth_exchange_failed", Some(&landing()))).await;
        assert!(bad.contains("error=oauth_exchange_failed"), "{bad}");
        assert!(bad.contains("type:\"aurora:connect-failed\""), "{bad}");
        assert!(bad.contains("app_id:\"acme_app\""), "{bad}");
        assert!(!bad.contains("opener"), "{bad}");
        let anon = html(error("oauth_state_invalid", None)).await;
        assert!(anon.contains("type:\"aurora:connect-failed\""), "{anon}");
        assert!(!anon.contains("app_id"), "{anon}");
    }
}
