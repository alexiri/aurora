//! Adapter `fake`: in-process, directive-driven, no LLM (`docs/12-supervisor-harness.md`).
//!
//! Directives in the turn text: `[[say: …]]`, `[[tool: name {json}]]`, `[[http: GET url]]`
//! (through the session proxy), `[[sleep: 2s]]`, `[[fail: code]]`, `[[steer-accept]]`. Without any
//! directive the turn echoes the text word by word.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use aurora_protocol::runner::SendNowOutcome;
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc, watch};

use crate::harness::{
    HarnessAdapter, HarnessCapabilities, HarnessError, HarnessEvent, HarnessEventStream,
    HarnessSession, SessionOptions, SteerAck, SteerInput, TurnInput,
};

const STEER_WAIT: Duration = Duration::from_secs(10);
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, Default)]
pub struct FakeHarness;

#[async_trait]
impl HarnessAdapter for FakeHarness {
    fn id(&self) -> &'static str {
        "fake"
    }

    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities {
            resume: true,
            steer: true,
            background_tool: true,
            thinking_deltas: true,
            tool_events: true,
            platform_tools: false,
            dialects: vec!["anthropic_messages".into()],
            skills_dir: Some(".claude/skills".into()),
            instruction_files: vec!["AGENTS.md".into()],
        }
    }

    async fn start_session(
        &self,
        opts: SessionOptions,
    ) -> Result<Box<dyn HarnessSession>, HarnessError> {
        let session_ref = opts
            .resume_ref
            .clone()
            .unwrap_or_else(|| format!("fake-{}", opts.conversation_id));
        // A marker file stands in for the stored session so purges are observable in tests.
        if let Some(dir) = fake_session_dir(&opts.working_dir)
            && std::fs::create_dir_all(&dir).is_ok()
        {
            let _ = std::fs::write(dir.join(&session_ref), b"fake session");
        }
        Ok(Box::new(FakeSession {
            opts,
            session_ref,
            turn: None,
        }))
    }

    async fn purge_session(
        &self,
        working_dir: &Path,
        session_ref: &str,
    ) -> Result<(), HarnessError> {
        if !crate::harness::is_safe_session_ref(session_ref) {
            return Err(HarnessError::new(
                "invalid_session_ref",
                "unsafe session reference",
            ));
        }
        if let Some(dir) = fake_session_dir(working_dir) {
            match std::fs::remove_file(dir.join(session_ref)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(HarnessError::new("purge_failed", e.to_string())),
            }
        }
        Ok(())
    }
}

/// Where the fake harness keeps its session marker files (`<home>/.fake-sessions`).
pub fn fake_session_dir(working_dir: &Path) -> Option<PathBuf> {
    Some(working_dir.join(".fake-sessions"))
}

struct ActiveTurn {
    cancel: watch::Sender<bool>,
    steer_tx: mpsc::Sender<SteerInput>,
    accepts_steer: bool,
    running: Arc<AtomicBool>,
    sleeping: Arc<AtomicBool>,
    send_now: Arc<Notify>,
}

pub struct FakeSession {
    opts: SessionOptions,
    session_ref: String,
    turn: Option<ActiveTurn>,
}

#[derive(Debug, Clone, PartialEq)]
enum Directive {
    Say(String),
    Tool(String, Value),
    Http(String, String),
    Sleep(Duration),
    Fail(String),
    SteerAccept,
}

fn parse_duration(s: &str) -> Duration {
    let s = s.trim();
    if let Some(ms) = s.strip_suffix("ms") {
        Duration::from_millis(ms.trim().parse().unwrap_or(0))
    } else {
        let n = s.strip_suffix('s').unwrap_or(s).trim();
        Duration::from_secs_f64(n.parse().unwrap_or(0.0))
    }
}

fn parse(text: &str) -> Vec<Directive> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else { break };
        let body = after[..end].trim();
        rest = &after[end + 2..];
        let (name, arg) = body
            .split_once(':')
            .map_or((body, ""), |(n, a)| (n.trim(), a.trim()));
        match name {
            "say" => out.push(Directive::Say(arg.to_owned())),
            "tool" => {
                let (tool, args) = arg.split_once(char::is_whitespace).unwrap_or((arg, ""));
                let args = serde_json::from_str(args.trim()).unwrap_or(Value::Null);
                out.push(Directive::Tool(tool.to_owned(), args));
            }
            "http" => {
                let (m, u) = arg.split_once(char::is_whitespace).unwrap_or(("GET", arg));
                out.push(Directive::Http(m.to_owned(), u.trim().to_owned()));
            }
            "sleep" => out.push(Directive::Sleep(parse_duration(arg))),
            "fail" => out.push(Directive::Fail(arg.to_owned())),
            "steer-accept" => out.push(Directive::SteerAccept),
            _ => {}
        }
    }
    out
}

#[async_trait]
impl HarnessSession for FakeSession {
    fn session_ref(&self) -> Option<String> {
        Some(self.session_ref.clone())
    }

    async fn run_turn(&mut self, input: TurnInput) -> Result<HarnessEventStream, HarnessError> {
        if self
            .turn
            .as_ref()
            .is_some_and(|t| t.running.load(Ordering::SeqCst))
        {
            return Err(HarnessError::new(
                "harness_failed",
                "a turn is already running",
            ));
        }
        let mut script = parse(&input.text);
        if script.is_empty() {
            script = input
                .text
                .split_whitespace()
                .enumerate()
                .map(|(i, w)| {
                    Directive::Say(if i == 0 {
                        format!("echo: {w}")
                    } else {
                        format!(" {w}")
                    })
                })
                .collect();
        }
        let (tx, rx) = mpsc::channel(64);
        let (cancel, cancel_rx) = watch::channel(false);
        let (steer_tx, steer_rx) = mpsc::channel(8);
        let running = Arc::new(AtomicBool::new(true));
        let turn = ActiveTurn {
            cancel,
            steer_tx,
            accepts_steer: script.contains(&Directive::SteerAccept),
            running: running.clone(),
            sleeping: Arc::new(AtomicBool::new(false)),
            send_now: Arc::new(Notify::new()),
        };
        let ctx = Script {
            tx,
            cancel: cancel_rx,
            steer_rx,
            sleeping: turn.sleeping.clone(),
            send_now: turn.send_now.clone(),
            proxy_url: self.opts.proxy_url.clone(),
            ca_path: self
                .opts
                .env
                .iter()
                .find(|(k, _)| k == "SSL_CERT_FILE")
                .map(|(_, v)| v.clone())
                .or_else(|| std::env::var("SSL_CERT_FILE").ok())
                .unwrap_or_else(|| crate::boot::DEFAULT_CA_PATH.to_owned()),
        };
        self.turn = Some(turn);
        tokio::spawn(async move {
            ctx.run(script).await;
            running.store(false, Ordering::SeqCst);
        });
        Ok(rx)
    }

    async fn steer(&mut self, input: SteerInput) -> Result<SteerAck, HarnessError> {
        match &self.turn {
            Some(t) if t.running.load(Ordering::SeqCst) && t.accepts_steer => {
                Ok(if t.steer_tx.try_send(input).is_ok() {
                    SteerAck::Accepted
                } else {
                    SteerAck::Refused
                })
            }
            _ => Ok(SteerAck::Refused),
        }
    }

    async fn background_running_tool(&mut self) -> Result<SendNowOutcome, HarnessError> {
        match &self.turn {
            Some(t) if t.running.load(Ordering::SeqCst) && t.sleeping.load(Ordering::SeqCst) => {
                t.send_now.notify_one();
                Ok(SendNowOutcome::Moved)
            }
            _ => Ok(SendNowOutcome::NothingRunning),
        }
    }

    async fn abort(&mut self) -> Result<(), HarnessError> {
        if let Some(t) = &self.turn {
            let _ = t.cancel.send(true);
        }
        Ok(())
    }

    async fn release(self: Box<Self>) -> Result<(), HarnessError> {
        if let Some(t) = &self.turn {
            let _ = t.cancel.send(true);
        }
        Ok(())
    }
}

async fn cancelled(mut rx: watch::Receiver<bool>) {
    let _ = rx.wait_for(|c| *c).await;
}

struct Script {
    tx: mpsc::Sender<HarnessEvent>,
    cancel: watch::Receiver<bool>,
    steer_rx: mpsc::Receiver<SteerInput>,
    sleeping: Arc<AtomicBool>,
    send_now: Arc<Notify>,
    proxy_url: String,
    ca_path: String,
}

impl Script {
    async fn emit(&self, ev: HarnessEvent) -> bool {
        self.tx.send(ev).await.is_ok()
    }

    async fn run(mut self, script: Vec<Directive>) {
        let end = self.play(script).await;
        let _ = self.tx.send(end).await;
    }

    /// Returns the terminal event.
    async fn play(&mut self, script: Vec<Directive>) -> HarnessEvent {
        let aborted = || HarnessEvent::Error {
            code: "aborted".into(),
            message: "turn aborted".into(),
        };
        let (mut msg_n, mut call_n, mut out_tokens) = (0u32, 0u32, 0u64);
        for d in script {
            if *self.cancel.borrow() {
                return aborted();
            }
            let ok = match d {
                Directive::Say(text) => {
                    msg_n += 1;
                    out_tokens += 1;
                    self.emit(HarnessEvent::TextDelta {
                        message_id: "m1".into(),
                        delta: text,
                        reply_to: None,
                    })
                    .await
                }
                Directive::Tool(name, args) => {
                    call_n += 1;
                    let call_id = format!("call-{call_n}");
                    let summary = format!("{name} ({} bytes of args)", args.to_string().len());
                    self.emit(HarnessEvent::ToolStarted {
                        call_id: call_id.clone(),
                        name,
                        summary,
                    })
                    .await
                        && self
                            .emit(HarnessEvent::ToolFinished {
                                call_id,
                                ok: true,
                                summary: None,
                            })
                            .await
                }
                Directive::Http(method, url) => {
                    let line = format!("{method} {url}");
                    let r = self.http(&method, &url).await;
                    self.emit(HarnessEvent::Activity { line }).await
                        && self
                            .emit(HarnessEvent::TextDelta {
                                message_id: format!("m{}", msg_n + 1),
                                delta: r,
                                reply_to: None,
                            })
                            .await
                }
                Directive::Sleep(dur) => {
                    self.sleeping.store(true, Ordering::SeqCst);
                    let cancel = self.cancel.clone();
                    let sent_now = tokio::select! {
                        _ = tokio::time::sleep(dur) => false,
                        _ = cancelled(cancel) => {
                            self.sleeping.store(false, Ordering::SeqCst);
                            return aborted();
                        }
                        _ = self.send_now.notified() => true,
                    };
                    self.sleeping.store(false, Ordering::SeqCst);
                    !sent_now || self.emit(HarnessEvent::SentNow).await
                }
                Directive::Fail(code) => {
                    return HarnessEvent::Error {
                        message: format!("scripted failure: {code}"),
                        code,
                    };
                }
                Directive::SteerAccept => {
                    let cancel = self.cancel.clone();
                    tokio::select! {
                        s = tokio::time::timeout(STEER_WAIT, self.steer_rx.recv()) => {
                            if let Ok(Some(s)) = s {
                                self.emit(HarnessEvent::MessageJoined { followup_id: s.followup_id }).await
                                    && self.emit(HarnessEvent::TextDelta {
                                        message_id: "m1".into(),
                                        delta: format!(" steered: {}", s.text),
                                        reply_to: Some(s.followup_id.to_string()),
                                    }).await
                            } else {
                                true
                            }
                        }
                        _ = cancelled(cancel) => return aborted(),
                    }
                }
            };
            if !ok {
                return aborted(); // receiver gone
            }
        }
        HarnessEvent::Done {
            usage: Some(json!({ "input_tokens": 1, "output_tokens": out_tokens })),
        }
    }

    async fn http(&self, method: &str, url: &str) -> String {
        match self.request(method, url).await {
            Ok(s) => s,
            Err(e) => format!("http error: {e}"),
        }
    }

    async fn request(&self, method: &str, url: &str) -> Result<String, reqwest::Error> {
        let mut b = reqwest::Client::builder()
            .proxy(reqwest::Proxy::all(&self.proxy_url)?)
            .timeout(HTTP_TIMEOUT);
        if let Ok(pem) = std::fs::read(&self.ca_path)
            && let Ok(cert) = reqwest::Certificate::from_pem(&pem)
        {
            b = b.add_root_certificate(cert);
        }
        let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::GET);
        let resp = b.build()?.request(method, url).send().await?;
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        let snippet: String = body.chars().take(200).collect();
        Ok(format!("http {status}: {snippet}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_directives() {
        let d = parse(
            "hi [[say: hello]] [[tool: memory_save {\"k\":1}]] [[sleep: 250ms]] [[http: GET http://x/y]] [[steer-accept]] [[fail: provider_quota]]",
        );
        assert_eq!(d[0], Directive::Say("hello".into()));
        assert_eq!(d[1], Directive::Tool("memory_save".into(), json!({"k":1})));
        assert_eq!(d[2], Directive::Sleep(Duration::from_millis(250)));
        assert_eq!(d[3], Directive::Http("GET".into(), "http://x/y".into()));
        assert_eq!(d[4], Directive::SteerAccept);
        assert_eq!(d[5], Directive::Fail("provider_quota".into()));
    }
}
