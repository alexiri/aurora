//! Template summaries: `approvals.<tool>` (docs/09 §3.4) with the core decoders (§5).
//!
//! Every value is derived from the request only (never from credentials). A field whose
//! value cannot be produced (missing, decode failure, truncated body, forbidden header)
//! is omitted; partial values are never shown. The result is clamped/sanitized.

use crate::{ApprovalSummary, SummaryField, SummarySource};
use aurora_core::limits::SUMMARY_TEXT_MAX;
use aurora_registry::manifest::{Approval, Decoder, GraphqlSelect, Inject, Source};
use aurora_registry::template::{Ns, Part};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD};
use mail_parser::{HeaderValue, MessageParser};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};

/// Everything but RFC 3986 unreserved characters.
const LINK_VALUE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// Everything the template summarizer may look at.
pub struct Input<'a> {
    pub host: &'a str,
    /// Request path (already redacted by the caller), without query.
    pub path: &'a str,
    pub query: Option<&'a str>,
    /// Case-insensitive lookup of a request header value.
    pub header: &'a dyn Fn(&str) -> Option<String>,
    pub body_prefix: &'a [u8],
    /// The body is longer than `body_prefix`.
    pub truncated: bool,
    /// Hosts the app declares (`hosts`), for link validation.
    pub declared_hosts: &'a [&'a str],
    /// `about.web_hosts`.
    pub web_hosts: &'a [String],
    /// Header names set by the matched host's inject mechanisms (never shown).
    pub injected_headers: &'a [String],
}

/// Header names an `inject` list writes (lower-case), to be refused as sources.
#[must_use]
pub fn injected_header_names(inject: &[Inject]) -> Vec<String> {
    inject
        .iter()
        .filter_map(|i| match i {
            Inject::Header { name, .. } | Inject::RemoveHeader(name) => {
                Some(name.to_ascii_lowercase())
            }
            _ => None,
        })
        .collect()
}

/// True when the approval asks for the hook first (`summarize: hook`).
#[must_use]
pub fn wants_hook(a: &Approval) -> bool {
    a.summarize.is_some()
}

/// Template summary for `a`. Falls back to nothing: callers use [`crate::generic`] when
/// there is no template for the tool.
#[must_use]
pub fn summarize(a: &Approval, inp: &Input<'_>) -> ApprovalSummary {
    let mut fields: Vec<SummaryField> = Vec::new();
    for f in &a.fields {
        if let Some(value) = source_value(&f.source, inp).filter(|v| !v.is_empty()) {
            fields.push(SummaryField {
                label: f.label.clone(),
                value,
            });
        }
    }
    let link = a
        .link
        .as_ref()
        .and_then(|t| render_link(t.parts(), inp, &fields));
    ApprovalSummary {
        title: a.title.clone(),
        fields,
        text: None,
        link,
        group: None,
        source: SummarySource::Template,
    }
    .clamp()
}

fn path_segment(path: &str, n: usize) -> Option<&str> {
    // 1-based over the `/`-split path (index 0 is the empty part before the leading `/`).
    path.split('/').nth(n).filter(|s| !s.is_empty())
}

fn query_param(query: Option<&str>, name: &str) -> Option<String> {
    query?
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| form_decode(k) == name)
        .map(|(_, v)| form_decode(v))
}

fn form_decode(s: &str) -> String {
    let s = s.replace('+', " ");
    percent_decode_str(&s).decode_utf8_lossy().into_owned()
}

/// Headers that are never shown, whatever a template says.
#[must_use]
pub fn header_forbidden(name: &str, injected: &[String]) -> bool {
    let n = name.to_ascii_lowercase();
    n == "authorization"
        || n == "cookie"
        || n == "x-api-key"
        || n.starts_with("proxy-")
        || n.starts_with("x-aurora-")
        || injected.contains(&n)
}

fn source_value(s: &Source, inp: &Input<'_>) -> Option<String> {
    match s {
        Source::Path { segment } => path_segment(inp.path, *segment as usize).map(str::to_owned),
        Source::Query { name } => query_param(inp.query, name),
        Source::Header { name } => {
            if header_forbidden(name, inp.injected_headers) {
                return None;
            }
            (inp.header)(name)
        }
        Source::Body { steps } => {
            if inp.truncated {
                return None;
            }
            run_steps(steps, inp.body_prefix)
        }
    }
}

enum Val {
    Bytes(Vec<u8>),
    Text(String),
}

impl Val {
    fn bytes(&self) -> &[u8] {
        match self {
            Val::Bytes(b) => b,
            Val::Text(t) => t.as_bytes(),
        }
    }
    fn text(&self) -> Option<&str> {
        match self {
            Val::Bytes(b) => std::str::from_utf8(b).ok(),
            Val::Text(t) => Some(t),
        }
    }
}

/// Runs the decoder chain over the body; the last step must yield text.
pub fn run_steps(steps: &[Decoder], body: &[u8]) -> Option<String> {
    let mut v = Val::Bytes(body.to_vec());
    for s in steps {
        v = step(s, &v)?;
    }
    match v {
        Val::Text(t) => Some(t),
        Val::Bytes(b) => String::from_utf8(b).ok(),
    }
}

fn step(d: &Decoder, v: &Val) -> Option<Val> {
    match d {
        Decoder::Json(ptr) if ptr.split('/').any(|s| s == "*") => {
            // (FIX-118) `*` visits every element of an array (or member of an object); the scalar
            // leaves are listed, comma-separated, so every recipient or attendee is shown.
            let j: serde_json::Value = serde_json::from_slice(v.bytes()).ok()?;
            let mut nodes = vec![&j];
            for seg in ptr.split('/').skip(1) {
                let mut next = Vec::new();
                for n in nodes {
                    if seg == "*" {
                        match n {
                            serde_json::Value::Array(a) => next.extend(a.iter()),
                            serde_json::Value::Object(o) => next.extend(o.values()),
                            _ => {}
                        }
                    } else if let Some(c) = n.pointer(&format!("/{seg}")) {
                        next.push(c);
                    }
                }
                nodes = next;
            }
            let leaves: Vec<String> = nodes.into_iter().filter_map(scalar_text).collect();
            (!leaves.is_empty()).then(|| Val::Text(leaves.join(", ")))
        }
        Decoder::Json(ptr) => {
            let j: serde_json::Value = serde_json::from_slice(v.bytes()).ok()?;
            scalar_text(j.pointer(ptr)?).map(Val::Text)
        }
        Decoder::Form(name) => {
            let t = v.text()?;
            t.split('&')
                .filter_map(|kv| kv.split_once('='))
                .find(|(k, _)| form_decode(k) == *name)
                .map(|(_, val)| Val::Text(form_decode(val)))
        }
        Decoder::Graphql(sel) => graphql(v.bytes(), *sel).map(Val::Text),
        Decoder::Base64url(_) => {
            let t = v.text()?.trim().trim_end_matches('=');
            URL_SAFE_NO_PAD.decode(t).ok().map(Val::Bytes)
        }
        Decoder::Base64(_) => {
            let t = v.text()?.trim().trim_end_matches('=');
            STANDARD_NO_PAD.decode(t).ok().map(Val::Bytes)
        }
        Decoder::MimeHeader(name) => {
            let msg = MessageParser::default().parse(v.bytes())?;
            // (FIX-118) Every header of that name: a second `To:` or `Cc:` line is a recipient
            // too, and an approval must show who gets the mail.
            let mut parts = Vec::new();
            for h in msg.headers().all(name.as_str()) {
                let s = match h.value() {
                    HeaderValue::Text(t) => t.to_owned(),
                    HeaderValue::Address(list) => list
                        .mailboxes()
                        .map(|m| match (m.name(), m.address()) {
                            (Some(n), Some(a)) => format!("{n} <{a}>"),
                            (None, Some(a)) => a.to_owned(),
                            (Some(n), None) => n.to_owned(),
                            (None, None) => String::new(),
                        })
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(", "),
                    other => other.as_text()?.to_owned(),
                };
                if !s.is_empty() {
                    parts.push(s);
                }
            }
            if parts.is_empty() {
                return None;
            }
            Some(Val::Text(parts.join(", ")))
        }
        Decoder::MimeAttachments(_) => {
            let msg = MessageParser::default().parse(v.bytes())?;
            Some(Val::Text(msg.attachments().len().to_string()))
        }
        Decoder::JsonCount(ptr) => {
            let j: serde_json::Value = serde_json::from_slice(v.bytes()).ok()?;
            let n = match j.pointer(ptr) {
                Some(serde_json::Value::Array(a)) => a.len(),
                Some(_) => return None,
                None => 0,
            };
            Some(Val::Text(n.to_string()))
        }
        Decoder::JsonText(ptr) => {
            let j: serde_json::Value = serde_json::from_slice(v.bytes()).ok()?;
            let mut out = Vec::new();
            collect_text(j.pointer(ptr)?, &mut out, 0);
            (!out.is_empty()).then(|| Val::Text(out.join(" ")))
        }
        Decoder::MimeText(_) => {
            let msg = MessageParser::default().parse(v.bytes())?;
            let part = msg
                .parts()
                .find(|p| !p.is_multipart() && p.is_content_type("text", "plain"))?;
            let t = part.text_prefix(SUMMARY_TEXT_MAX)?;
            Some(Val::Text(t.into_owned()))
        }
    }
}

fn scalar_text(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        x @ (serde_json::Value::Number(_) | serde_json::Value::Bool(_)) => Some(x.to_string()),
        _ => None,
    }
}

/// Every string stored under a `text` or `content` key below `v` (nested document and rich-text
/// block formats), in document order; a string `v` is its own text.
fn collect_text(v: &serde_json::Value, out: &mut Vec<String>, depth: usize) {
    if depth > 32 {
        return;
    }
    match v {
        serde_json::Value::String(s) if depth == 0 => out.push(s.clone()),
        serde_json::Value::Array(a) => a.iter().for_each(|x| collect_text(x, out, depth + 1)),
        serde_json::Value::Object(o) => {
            for (k, x) in o {
                match x {
                    serde_json::Value::String(s) if k == "text" || k == "content" => {
                        out.push(s.clone());
                    }
                    _ => collect_text(x, out, depth + 1),
                }
            }
        }
        _ => {}
    }
}

/// `operation_name`: the request's `operationName`, else the first named operation.
/// `field_names`: top-level fields of the first operation, comma-separated.
fn graphql(body: &[u8], sel: GraphqlSelect) -> Option<String> {
    let (query, op_name) = match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(serde_json::Value::Object(o)) => (
            o.get("query")?.as_str()?.to_owned(),
            o.get("operationName")
                .and_then(|x| x.as_str())
                .map(str::to_owned),
        ),
        _ => (std::str::from_utf8(body).ok()?.to_owned(), None),
    };
    let doc = graphql_parser::parse_query::<String>(&query).ok()?;
    use graphql_parser::query::{Definition, OperationDefinition as Op, Selection};
    let ops: Vec<(Option<&str>, &graphql_parser::query::SelectionSet<String>)> = doc
        .definitions
        .iter()
        .filter_map(|d| match d {
            Definition::Operation(Op::Query(q)) => Some((q.name.as_deref(), &q.selection_set)),
            Definition::Operation(Op::Mutation(m)) => Some((m.name.as_deref(), &m.selection_set)),
            Definition::Operation(Op::Subscription(s)) => {
                Some((s.name.as_deref(), &s.selection_set))
            }
            Definition::Operation(Op::SelectionSet(s)) => Some((None, s)),
            Definition::Fragment(_) => None,
        })
        .collect();
    let chosen = match &op_name {
        Some(n) => ops.iter().find(|(name, _)| *name == Some(n.as_str()))?,
        None => ops.first()?,
    };
    match sel {
        GraphqlSelect::OperationName => chosen.0.map(str::to_owned),
        GraphqlSelect::FieldNames => {
            let names: Vec<&str> = chosen
                .1
                .items
                .iter()
                .filter_map(|s| match s {
                    Selection::Field(f) => Some(f.name.as_str()),
                    _ => None,
                })
                .collect();
            (!names.is_empty()).then(|| names.join(", "))
        }
    }
}

fn render_link(parts: &[Part], inp: &Input<'_>, fields: &[SummaryField]) -> Option<String> {
    let mut out = String::new();
    for p in parts {
        match p {
            Part::Lit(l) => out.push_str(l),
            Part::Ref(r) => {
                let v = match r.ns {
                    Ns::Request => inp.host.to_owned(),
                    Ns::Path => path_segment(inp.path, r.name.parse().ok()?)?.to_owned(),
                    Ns::Query => query_param(inp.query, &r.name)?,
                    Ns::Field => fields.iter().find(|f| f.label == r.name)?.value.clone(),
                    _ => return None,
                };
                out.push_str(&utf8_percent_encode(&v, LINK_VALUE).to_string());
            }
        }
    }
    link_host_ok(&out, inp).then_some(out)
}

fn link_host_ok(link: &str, inp: &Input<'_>) -> bool {
    let Some(rest) = link.strip_prefix("https://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains(['@', '\\', ':', ' ']) {
        return false;
    }
    let host = authority.to_ascii_lowercase();
    inp.declared_hosts
        .iter()
        .any(|h| h.eq_ignore_ascii_case(&host))
        || inp.web_hosts.iter().any(|h| h.eq_ignore_ascii_case(&host))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use serde_json::json;

    fn run(
        a: serde_json::Value,
        path: &str,
        query: Option<&str>,
        body: &[u8],
        trunc: bool,
    ) -> ApprovalSummary {
        let a: Approval = serde_json::from_value(a).unwrap();
        let hdr = |n: &str| (n.eq_ignore_ascii_case("x-org")).then(|| "acme".to_owned());
        let web = vec!["app.example.com".to_owned()];
        let inj = vec!["x-injected".to_owned()];
        summarize(
            &a,
            &Input {
                host: "api.example.com",
                path,
                query,
                header: &hdr,
                body_prefix: body,
                truncated: trunc,
                declared_hosts: &["api.example.com"],
                web_hosts: &web,
                injected_headers: &inj,
            },
        )
    }

    fn val(s: &ApprovalSummary, l: &str) -> Option<String> {
        s.fields
            .iter()
            .find(|f| f.label == l)
            .map(|f| f.value.clone())
    }

    #[test]
    fn path_query_header() {
        let s = run(
            json!({"title":"T","fields":[
                {"label":"Task","source":"path","segment":4},
                {"label":"Q","source":"query","name":"a b"},
                {"label":"Org","source":"header","name":"X-Org"},
                {"label":"Auth","source":"header","name":"Authorization"},
                {"label":"Cookie","source":"header","name":"cookie"},
                {"label":"Px","source":"header","name":"Proxy-Foo"},
                {"label":"Key","source":"header","name":"x-api-key"},
                {"label":"Inj","source":"header","name":"X-Injected"},
                {"label":"Missing","source":"path","segment":9}]}),
            "/rest/v2/tasks/123/close",
            Some("a+b=c%20d"),
            b"",
            false,
        );
        assert_eq!(s.title, "T");
        assert_eq!(val(&s, "Task").as_deref(), Some("123"));
        assert_eq!(val(&s, "Q").as_deref(), Some("c d"));
        assert_eq!(val(&s, "Org").as_deref(), Some("acme"));
        for l in ["Auth", "Cookie", "Px", "Key", "Inj", "Missing"] {
            assert!(val(&s, l).is_none(), "{l}");
        }
        assert_eq!(s.source, SummarySource::Template);
    }

    #[test]
    fn raw_mime_message() {
        let mime = "To: Alice <alice@example.com>, bob@example.com\r\nSubject: =?UTF-8?B?SMOpbGxv?=\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHello body\r\n";
        let raw = URL_SAFE_NO_PAD.encode(mime);
        let body = serde_json::to_vec(&json!({ "raw": raw })).unwrap();
        let s = run(
            json!({"title":"Send","fields":[
              {"label":"To","source":"body","steps":[{"json":"/raw"},{"base64url":{}},{"mime_header":"To"}]},
              {"label":"Subject","source":"body","steps":[{"json":"/raw"},{"base64url":{}},{"mime_header":"Subject"}]},
              {"label":"Text","source":"body","steps":[{"json":"/raw"},{"base64url":{}},{"mime_text":{}}]}]}),
            "/mail/v1/users/me/messages/send",
            None,
            &body,
            false,
        );
        assert_eq!(
            val(&s, "To").as_deref(),
            Some("Alice <alice@example.com>, bob@example.com")
        );
        assert_eq!(val(&s, "Subject").as_deref(), Some("H\u{e9}llo"));
        assert_eq!(val(&s, "Text").as_deref(), Some("Hello body"));
        // Truncated body: every body field omitted, never partial.
        let s = run(
            json!({"title":"Send","fields":[
              {"label":"To","source":"body","steps":[{"json":"/raw"},{"base64url":{}},{"mime_header":"To"}]}]}),
            "/",
            None,
            &body[..20],
            true,
        );
        assert!(s.fields.is_empty());
    }

    /// (FIX-118, PKG-1) Every recipient — repeated `To:` lines, Cc and Bcc — and the attachment
    /// count reach the approval card.
    #[test]
    fn mail_shows_every_recipient_and_the_attachment_count() {
        let mime = "To: a@example.com\r\nTo: b@example.com, \"C\" <c@example.com>\r\nCc: cc@example.com\r\nBcc: spy@evil.example\r\nSubject: Status\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nSee attached\r\n--b\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=\"x.pdf\"\r\n\r\nPDF\r\n--b--\r\n";
        let body = serde_json::to_vec(&json!({ "raw": URL_SAFE_NO_PAD.encode(mime) })).unwrap();
        let step = |h: &str| json!([{"json":"/raw"},{"base64url":{}},{"mime_header":h}]);
        let s = run(
            json!({"title":"Send","fields":[
              {"label":"To","source":"body","steps":step("To")},
              {"label":"Cc","source":"body","steps":step("Cc")},
              {"label":"Bcc","source":"body","steps":step("Bcc")},
              {"label":"Attachments","source":"body","steps":[{"json":"/raw"},{"base64url":{}},{"mime_attachments":{}}]}]}),
            "/",
            None,
            &body,
            false,
        );
        assert_eq!(
            val(&s, "To").as_deref(),
            Some("a@example.com, b@example.com, C <c@example.com>")
        );
        assert_eq!(val(&s, "Cc").as_deref(), Some("cc@example.com"));
        assert_eq!(val(&s, "Bcc").as_deref(), Some("spy@evil.example"));
        assert_eq!(val(&s, "Attachments").as_deref(), Some("1"));
    }

    #[test]
    fn json_wildcards_counts_and_nested_text() {
        let st = |v: serde_json::Value| -> Vec<Decoder> { serde_json::from_value(v).unwrap() };
        let body = br#"{"message":{"toRecipients":[{"emailAddress":{"address":"a@x.io"}},{"emailAddress":{"address":"b@x.io"}}],"attachments":[{},{},{}]},
            "body":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"first"}]},{"type":"paragraph","content":[{"type":"text","text":"second"},{"type":"text","text":"third"}]}]},
            "children":[{"paragraph":{"rich_text":[{"type":"text","text":{"content":"block words"}}]}}]}"#;
        assert_eq!(
            run_steps(
                &st(json!([{"json":"/message/toRecipients/*/emailAddress/address"}])),
                body
            )
            .as_deref(),
            Some("a@x.io, b@x.io")
        );
        assert!(run_steps(&st(json!([{"json":"/message/nobody/*/x"}])), body).is_none());
        assert_eq!(
            run_steps(&st(json!([{"json_count":"/message/attachments"}])), body).as_deref(),
            Some("3")
        );
        assert_eq!(
            run_steps(&st(json!([{"json_count":"/message/ccRecipients"}])), body).as_deref(),
            Some("0")
        );
        assert_eq!(
            run_steps(&st(json!([{"json_text":"/body"}])), body).as_deref(),
            Some("first second third")
        );
        assert_eq!(
            run_steps(&st(json!([{"json_text":"/children"}])), body).as_deref(),
            Some("block words")
        );
    }

    #[test]
    fn mime_text_multipart_first_plain_and_clamp() {
        let long = "x".repeat(3000);
        let mime = format!(
            "Content-Type: multipart/alternative; boundary=b\r\n\r\n--b\r\nContent-Type: text/html\r\n\r\n<p>no</p>\r\n--b\r\nContent-Type: text/plain\r\n\r\n{long}\r\n--b--\r\n"
        );
        let t = run_steps(
            &[
                Decoder::Base64(Default::default()),
                Decoder::MimeText(Default::default()),
            ],
            STANDARD_NO_PAD.encode(mime).as_bytes(),
        )
        .unwrap();
        assert!(t.starts_with("xxx") && t.chars().count() <= SUMMARY_TEXT_MAX);
    }

    #[test]
    fn form_json_base64_and_failures() {
        let st = |v: serde_json::Value| -> Vec<Decoder> { serde_json::from_value(v).unwrap() };
        assert_eq!(
            run_steps(&st(json!([{"form":"to"}])), b"a=1&to=x%40y.z+w&b=2").as_deref(),
            Some("x@y.z w")
        );
        assert!(run_steps(&st(json!([{"form":"nope"}])), b"a=1").is_none());
        assert_eq!(
            run_steps(&st(json!([{"json":"/a/0/b"}])), br#"{"a":[{"b":42}]}"#).as_deref(),
            Some("42")
        );
        assert!(run_steps(&st(json!([{"json":"/a"}])), br#"{"a":{"x":1}}"#).is_none());
        assert!(run_steps(&st(json!([{"json":"/a"}])), b"{\"a\":").is_none());
        assert_eq!(
            run_steps(
                &st(json!([{"json":"/v"},{"base64":{}}])),
                br#"{"v":"aGk="}"#
            )
            .as_deref(),
            Some("hi")
        );
        assert!(run_steps(&st(json!([{"base64":{}}])), b"!!!").is_none());
        assert!(run_steps(&st(json!([{"mime_header":"To"}])), b"").is_none());
    }

    #[test]
    fn graphql_decoders() {
        let st = |v: serde_json::Value| -> Vec<Decoder> { serde_json::from_value(v).unwrap() };
        let body = serde_json::to_vec(&json!({
            "query": "mutation CreateIssue($i: I!) { createIssue(input: $i) { id } addLabel { id } }",
            "operationName": "CreateIssue"
        }))
        .unwrap();
        assert_eq!(
            run_steps(&st(json!([{"graphql":"operation_name"}])), &body).as_deref(),
            Some("CreateIssue")
        );
        assert_eq!(
            run_steps(&st(json!([{"graphql":"field_names"}])), &body).as_deref(),
            Some("createIssue, addLabel")
        );
        assert_eq!(
            run_steps(&st(json!([{"graphql":"operation_name"}])), b"query Q { a }").as_deref(),
            Some("Q")
        );
        assert!(run_steps(&st(json!([{"graphql":"operation_name"}])), b"{ a }").is_none());
        assert!(run_steps(&st(json!([{"graphql":"operation_name"}])), b"nonsense {").is_none());
    }

    #[test]
    fn links() {
        let f = |link: &str| {
            run(
                json!({"title":"T","fields":[{"label":"Id","source":"path","segment":2}],"link":link}),
                "/x/42/y",
                Some("q=v"),
                b"",
                false,
            )
            .link
        };
        assert_eq!(
            f("https://app.example.com/t/{path.2}?q={query.q}").as_deref(),
            Some("https://app.example.com/t/42?q=v")
        );
        assert_eq!(
            f("https://{request.host}/i/{field.Id}").as_deref(),
            Some("https://api.example.com/i/42")
        );
        assert!(f("https://evil.example.org/x").is_none());
        assert!(f("http://app.example.com/x").is_none());
        assert!(f("https://app.example.com@evil.com/x").is_none());
        assert!(f("https://{path.1}/x").is_none());
        assert!(f("https://app.example.com/{path.9}").is_none());
        assert!(f("https://app.example.com/{field.Nope}").is_none());
    }

    #[test]
    fn sanitized_and_clamped() {
        let s = run(
            json!({"title":"T\nx","fields":[{"label":"A\u{202e}","source":"query","name":"v"}]}),
            "/",
            Some(&format!("v={}", "z".repeat(500))),
            b"",
            false,
        );
        assert!(!s.title.contains('\n'));
        assert!(s.fields[0].value.chars().count() <= 200);
        assert!(!s.fields[0].label.contains('\u{202e}'));
    }
}
