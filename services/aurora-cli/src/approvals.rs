//! `aurora approvals watch`: long-poll pending approvals, decide interactively
//! or stream JSON lines.
use std::collections::HashSet;
use std::io::{BufRead, Write};
use std::time::Duration;

use anyhow::Result;
use aurora_client::types::{Approval, Decision};
use aurora_client::{Client, Error};

pub struct WatchOpts {
    pub json: bool,
    /// Only show approvals of this workspace id.
    pub workspace_id: Option<String>,
    /// Stop after this many poll rounds (tests); `None` = forever.
    pub max_rounds: Option<usize>,
    pub wait_secs: u64,
    pub retry_delay: Duration,
}

/// Every server-supplied string is printed without control characters (FIX-118, PKG-7): an
/// escape sequence in a summary field must not be able to repaint the approval screen.
pub fn format_approval(a: &Approval) -> String {
    use aurora_client::clean as c;
    let mut s = String::new();
    let agent = a
        .agent
        .as_ref()
        .map_or_else(|| "-".to_owned(), |g| c(&g.handle));
    s.push_str(&format!("\n[{}] {}\n", c(&a.id), c(&a.summary.title)));
    s.push_str(&format!(
        "  agent: {agent}   {} {}{}\n",
        c(&a.method),
        c(&a.host),
        c(&a.path)
    ));
    for f in &a.summary.fields {
        s.push_str(&format!("  {}: {}\n", c(&f.label), c(&f.value)));
    }
    if let Some(t) = &a.summary.text {
        s.push_str(&format!("  {}\n", c(t)));
    }
    if let Some(r) = &a.rule_name {
        s.push_str(&format!("  rule: {}\n", c(r)));
    }
    if let Some(d) = &a.deadline {
        s.push_str(&format!("  deadline: {}\n", c(d)));
    }
    s
}

pub async fn watch(
    client: &Client,
    opts: &WatchOpts,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<()> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut round = 0usize;
    loop {
        if opts.max_rounds.is_some_and(|m| round >= m) {
            return Ok(());
        }
        round += 1;
        let items = match client.pending_approvals(opts.wait_secs).await {
            Ok(i) => i,
            Err(Error::Api { status: 401, .. }) => {
                anyhow::bail!("unauthorized: run `aurora login` again")
            }
            Err(e) => {
                writeln!(out, "poll failed: {e}; retrying")?;
                tokio::time::sleep(opts.retry_delay).await;
                continue;
            }
        };
        for a in items {
            if opts
                .workspace_id
                .as_deref()
                .is_some_and(|w| w != a.workspace_id)
                || !seen.insert(a.id.clone())
            {
                continue;
            }
            if opts.json {
                writeln!(out, "{}", serde_json::to_string(&a)?)?;
                continue;
            }
            write!(out, "{}", format_approval(&a))?;
            loop {
                write!(out, "  [a]pprove / [d]eny / [s]kip > ")?;
                out.flush()?;
                let mut line = String::new();
                if input.read_line(&mut line)? == 0 {
                    return Ok(()); // EOF
                }
                let decision = match line.trim().to_ascii_lowercase().as_str() {
                    "a" | "approve" => Decision::Approve,
                    "d" | "deny" => Decision::Deny,
                    "s" | "skip" | "" => {
                        writeln!(out, "  skipped")?;
                        break;
                    }
                    _ => continue,
                };
                match client.decide(&a.id, decision).await {
                    Ok(r) => writeln!(out, "  {}", r.status)?,
                    Err(e) if e.code() == Some("already_decided") => {
                        writeln!(out, "  already decided")?
                    }
                    Err(e) => writeln!(out, "  error: {e}")?,
                }
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ap(id: &str, ws: &str) -> serde_json::Value {
        json!({"id":id,"org_id":"o","workspace_id":ws,"agent":{"id":"a","name":"Bot","handle":"bot"},
          "method":"POST","host":"api.x.com","path":"/v1","summary":{"title":"Send mail","fields":[{"label":"To","value":"a@b"}],
          "text":null,"link":null,"group":null,"source":"generic"},"status":"pending","deadline":"2026-11-05T21:58:00Z"})
    }

    fn opts(json: bool, ws: Option<&str>) -> WatchOpts {
        WatchOpts {
            json,
            workspace_id: ws.map(String::from),
            max_rounds: Some(1),
            wait_secs: 0,
            retry_delay: Duration::from_millis(1),
        }
    }

    /// (FIX-118, PKG-7) Escape sequences and carriage returns in server-supplied strings never
    /// reach the terminal.
    #[test]
    fn control_characters_are_stripped_from_the_approval_card() {
        let mut v = ap("p\u{1b}[2J1", "w1");
        v["summary"]["title"] = json!("Send\u{1b}[31m mail\r");
        v["summary"]["fields"][0]["value"] = json!("a@b\u{7}\u{1b}]0;pwned\u{7}\nBcc: x");
        let a: Approval = serde_json::from_value(v).unwrap();
        let s = format_approval(&a);
        assert!(!s.chars().any(|c| c.is_control() && c != '\n'), "{s:?}");
        assert!(s.contains("Send[31m mail"), "{s}");
        // A newline inside a value cannot start a fake line either.
        assert_eq!(s.lines().filter(|l| l.contains("Bcc: x")).count(), 1);
        assert!(s.contains("To: a@b]0;pwnedBcc: x"), "{s}");
    }

    async fn server() -> MockServer {
        let s = MockServer::start().await;
        Mock::given(path("/v1/approvals/pending"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"items":[ap("p1","w1"),ap("p2","w1"),ap("p3","w2")]})),
            )
            .mount(&s)
            .await;
        s
    }

    #[tokio::test]
    async fn interactive_approve_deny_skip() {
        let s = server().await;
        for (id, d) in [("p1", "approve"), ("p2", "deny")] {
            Mock::given(method("POST"))
                .and(path(format!("/v1/approvals/{id}/decision")))
                .and(body_json(json!({"decision": d})))
                .respond_with(ResponseTemplate::new(200).set_body_json({
                    let mut v = ap(id, "w1");
                    v["status"] = json!(if d == "approve" { "approved" } else { "denied" });
                    v
                }))
                .expect(1)
                .mount(&s)
                .await;
        }
        let c = Client::new(&s.uri(), Some("auk_k".into())).unwrap();
        let mut out = Vec::new();
        // p3 is skipped
        let mut input: &[u8] = b"a\nd\ns\n";
        watch(&c, &opts(false, None), &mut input, &mut out)
            .await
            .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("Send mail") && text.contains("approved") && text.contains("denied"));
        assert!(text.contains("skipped"));
    }

    #[tokio::test]
    async fn json_mode_streams_lines_with_workspace_filter() {
        let s = server().await;
        let c = Client::new(&s.uri(), Some("auk_k".into())).unwrap();
        let mut out = Vec::new();
        let mut input: &[u8] = b"";
        watch(&c, &opts(true, Some("w1")), &mut input, &mut out)
            .await
            .unwrap();
        let lines: Vec<serde_json::Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["id"], "p1");
    }
}
