//! The post-login relay: every statement the client sends is classified and policy-checked
//! before it is forwarded; refusals are answered locally and leave the server consistent.
//!
//! Two tasks share one [`Shared`]: `c2s` reads client messages and decides; `s2c` copies server
//! messages to the client and watches `ReadyForQuery`. They coordinate through a FIFO of
//! [`Unit`]s, one per expected `ReadyForQuery`:
//!
//! * a **refusal** pushes a `Hold` unit and sends a `Sync` to the server, so earlier responses
//!   are relayed first (order is preserved) and the server's `ReadyForQuery` is withheld from the
//!   client; the refusal `ErrorResponse` is then written at the right position;
//! * if the server is inside a transaction (`T`), a deliberately failing statement is sent and
//!   its response swallowed (`Swallow` unit), so the server enters the aborted state exactly like
//!   after a real error and `COMMIT` rolls back;
//! * messages after a refused extended-protocol message are discarded until the client's `Sync`,
//!   as PostgreSQL does after an error.
//!
//! Prepared statements and portals are tracked so that `Execute` is authorized against the text
//! the *server* will run. Name reuse that the server would reject is rejected here too, so the
//! gateway's table never diverges from the server's in a way that could authorize a statement
//! other than the one executed.

use super::wire::{self, Frame};
use crate::db::{DbAuthorizer, DbVerdict};
use aurora_sqlclass::{Class, MAX_STATEMENT_BYTES, Refusal};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf, split};
use tokio::sync::{Semaphore, oneshot, watch};

/// Largest client message accepted (statement, bind parameters, COPY chunk).
pub const MAX_CLIENT_MESSAGE: usize = 16 << 20;
/// Concurrent classifications (each may build a parse tree of several MiB).
const CLASSIFY_SLOTS: usize = 8;
const MAX_STATEMENTS: usize = 1024;
const MAX_STATEMENT_TEXT_TOTAL: usize = 32 << 20;
const MAX_PORTALS: usize = 1024;

static SLOTS: Semaphore = Semaphore::const_new(CLASSIFY_SLOTS);

/// Classification off the async runtime, bounded in concurrency and size; anything that goes
/// wrong inside the classifier is a refusal.
pub async fn classify_async(sql: String) -> Result<aurora_sqlclass::Classified, Refusal> {
    if sql.len() > MAX_STATEMENT_BYTES {
        return Err(Refusal::TooLarge);
    }
    let Ok(_permit) = SLOTS.acquire().await else {
        return Err(Refusal::Unparsable("classifier unavailable".into()));
    };
    tokio::task::spawn_blocking(move || aurora_sqlclass::classify(&sql))
        .await
        .unwrap_or_else(|_| Err(Refusal::Unparsable("classifier failed".into())))
}

/// Everything the relay needs from the outside.
pub struct Relay {
    pub authorizer: Arc<dyn DbAuthorizer>,
    pub stop: watch::Receiver<bool>,
    pub kill: Option<watch::Receiver<bool>>,
}

enum Unit {
    Normal,
    /// Withhold the server's `ReadyForQuery`; report `(status, an error was forwarded)`.
    Hold(oneshot::Sender<(u8, bool)>),
    /// Discard everything up to and including the `ReadyForQuery`; report its status.
    Swallow(oneshot::Sender<u8>),
}

struct Shared<C, S> {
    client: tokio::sync::Mutex<WriteHalf<C>>,
    server: tokio::sync::Mutex<WriteHalf<S>>,
    queue: Mutex<VecDeque<Unit>>,
    status: AtomicU8,
}

impl<C: AsyncWrite, S: AsyncWrite> Shared<C, S> {
    fn push(&self, u: Unit) {
        if let Ok(mut q) = self.queue.lock() {
            q.push_back(u);
        }
    }
    fn pop(&self) -> Option<Unit> {
        self.queue.lock().ok().and_then(|mut q| q.pop_front())
    }
    fn swallowing(&self) -> bool {
        self.queue
            .lock()
            .is_ok_and(|q| matches!(q.front(), Some(Unit::Swallow(_))))
    }
}

#[derive(Clone)]
struct Stmt {
    class: Class,
    statements: usize,
    text: Arc<str>,
}

struct Portal {
    stmt: Stmt,
    authorized: bool,
}

/// The refusal reported to the client.
struct Refused {
    sqlstate: &'static str,
    message: String,
    hint: Option<String>,
}

fn policy_refusal(e: &aurora_core::ApiError) -> Refused {
    Refused {
        sqlstate: "42501",
        message: e.message().to_owned(),
        hint: None,
    }
}

fn classifier_refusal(r: &Refusal) -> Refused {
    Refused {
        sqlstate: "42501",
        message: r.message(),
        hint: Some("This statement is never sent to the database by the Aurora gateway.".into()),
    }
}

fn cstr_at(b: &[u8]) -> Option<(&str, &[u8])> {
    let n = b.iter().position(|c| *c == 0)?;
    Some((std::str::from_utf8(&b[..n]).ok()?, &b[n + 1..]))
}

impl Relay {
    /// Runs until either side closes, the gateway stops or the session is killed.
    pub async fn run<C, S>(mut self, client: C, server: S)
    where
        C: AsyncRead + AsyncWrite + Unpin + Send + 'static,
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (cr, cw) = split(client);
        let (sr, sw) = split(server);
        let shared = Arc::new(Shared {
            client: tokio::sync::Mutex::new(cw),
            server: tokio::sync::Mutex::new(sw),
            queue: Mutex::new(VecDeque::new()),
            status: AtomicU8::new(b'I'),
        });
        let authorizer = self.authorizer.clone();
        let c2s = c2s(shared.clone(), cr, authorizer);
        let s2c = s2c(shared.clone(), sr);
        let mut stop = self.stop.clone();
        let mut kill = self.kill.take();
        let ended = async {
            tokio::select! {
                _ = c2s => None,
                _ = s2c => None,
                () = async { let _ = stop.wait_for(|s| *s).await; } => Some((b"57P01", "the gateway is shutting down")),
                () = async {
                    match kill.as_mut() {
                        Some(k) => { let _ = k.wait_for(|x| *x).await; }
                        None => std::future::pending::<()>().await,
                    }
                } => Some((b"57P01", "this database session was ended because access changed")),
            }
        }
        .await;
        if let Some((code, msg)) = ended {
            let mut w = shared.client.lock().await;
            let _ = wire::write_all(
                &mut *w,
                &wire::error_response(
                    "FATAL",
                    std::str::from_utf8(code).unwrap_or("57P01"),
                    msg,
                    None,
                ),
            )
            .await;
            let _ = w.shutdown().await;
        }
    }
}

/// Server -> client.
async fn s2c<C, S>(sh: Arc<Shared<C, S>>, mut sr: ReadHalf<S>)
where
    C: AsyncRead + AsyncWrite + Unpin + Send,
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let mut errored = false;
    loop {
        let Ok(Some((tag, len))) = wire::read_header(&mut sr).await else {
            return;
        };
        match tag {
            b'Z' if len == 1 => {
                let mut b = [0u8; 1];
                if sr.read_exact(&mut b).await.is_err() {
                    return;
                }
                sh.status.store(b[0], Ordering::SeqCst);
                match sh.pop() {
                    Some(Unit::Hold(tx)) => {
                        let _ = tx.send((b[0], errored));
                    }
                    Some(Unit::Swallow(tx)) => {
                        let _ = tx.send(b[0]);
                    }
                    Some(Unit::Normal) | None => {
                        let mut w = sh.client.lock().await;
                        if wire::write_all(&mut *w, &wire::ready_for_query(b[0]))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                }
                errored = false;
            }
            _ if sh.swallowing() => {
                if skip(&mut sr, len).await.is_err() {
                    return;
                }
            }
            b'E' if len <= (1 << 20) => {
                let mut body = vec![0u8; len];
                if sr.read_exact(&mut body).await.is_err() {
                    return;
                }
                errored = true;
                let mut w = sh.client.lock().await;
                if wire::write_all(&mut *w, &wire::encode(b'E', &body))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            _ => {
                // Stream the body without buffering it.
                let mut w = sh.client.lock().await;
                let mut head = vec![tag];
                head.extend_from_slice(&u32::try_from(len + 4).unwrap_or(u32::MAX).to_be_bytes());
                if w.write_all(&head).await.is_err() {
                    return;
                }
                let mut limited = (&mut sr).take(len as u64);
                match tokio::io::copy(&mut limited, &mut *w).await {
                    Ok(n) if n == len as u64 => {}
                    _ => return,
                }
                if w.flush().await.is_err() {
                    return;
                }
                if tag == b'E' {
                    errored = true;
                }
            }
        }
    }
}

async fn skip<R: AsyncRead + Unpin>(r: &mut R, n: usize) -> std::io::Result<()> {
    let mut limited = r.take(n as u64);
    let c = tokio::io::copy(&mut limited, &mut tokio::io::sink()).await?;
    if c == n as u64 {
        Ok(())
    } else {
        Err(std::io::ErrorKind::UnexpectedEof.into())
    }
}

struct Client<C, S> {
    sh: Arc<Shared<C, S>>,
    authorizer: Arc<dyn DbAuthorizer>,
    stmts: HashMap<String, Stmt>,
    stmt_bytes: usize,
    portals: HashMap<String, Portal>,
    /// After a refusal in the extended protocol: the status to report at the next `Sync`.
    skip_until_sync: Option<u8>,
    /// A `Sync` or simple query was sent since the portals were last pruned.
    synced: bool,
}

/// Client -> server.
async fn c2s<C, S>(sh: Arc<Shared<C, S>>, mut cr: ReadHalf<C>, authorizer: Arc<dyn DbAuthorizer>)
where
    C: AsyncRead + AsyncWrite + Unpin + Send,
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let mut st = Client {
        sh,
        authorizer,
        stmts: HashMap::new(),
        stmt_bytes: 0,
        portals: HashMap::new(),
        skip_until_sync: None,
        synced: false,
    };
    loop {
        let frame = match wire::read_frame(&mut cr, MAX_CLIENT_MESSAGE, MAX_CLIENT_MESSAGE).await {
            Ok(Some(f)) if !f.oversize => f,
            Ok(Some(_)) | Err(_) => {
                st.fatal("54000", "message too large or malformed").await;
                return;
            }
            Ok(None) => return,
        };
        if !st.handle(frame).await {
            return;
        }
    }
}

impl<C, S> Client<C, S>
where
    C: AsyncRead + AsyncWrite + Unpin + Send,
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    async fn fatal(&self, code: &str, msg: &str) {
        let mut w = self.sh.client.lock().await;
        let _ = wire::write_all(&mut *w, &wire::error_response("FATAL", code, msg, None)).await;
        let _ = w.shutdown().await;
    }

    async fn to_server(&self, bytes: &[u8]) -> bool {
        let mut w = self.sh.server.lock().await;
        wire::write_all(&mut *w, bytes).await.is_ok()
    }

    async fn forward(&self, f: &Frame) -> bool {
        self.to_server(&wire::encode(f.tag, &f.body)).await
    }

    /// Returns `false` when the session is over.
    async fn handle(&mut self, f: Frame) -> bool {
        if self.skip_until_sync.is_some() {
            return match f.tag {
                b'S' => {
                    let status = self.skip_until_sync.take().unwrap_or(b'I');
                    let mut w = self.sh.client.lock().await;
                    wire::write_all(&mut *w, &wire::ready_for_query(status))
                        .await
                        .is_ok()
                }
                b'X' => false,
                _ => true, // discarded, as the server does after an error
            };
        }
        match f.tag {
            b'Q' => self.simple_query(&f).await,
            b'P' => self.parse(&f).await,
            b'B' => self.bind(&f).await,
            b'E' => self.execute(&f).await,
            b'C' => {
                if let Some((&kind, rest)) = f.body.split_first()
                    && let Some((name, _)) = cstr_at(rest)
                {
                    match kind {
                        b'S' => {
                            if let Some(s) = self.stmts.remove(name) {
                                self.stmt_bytes = self.stmt_bytes.saturating_sub(s.text.len());
                            }
                        }
                        b'P' => {
                            self.portals.remove(name);
                        }
                        _ => {}
                    }
                }
                self.forward(&f).await
            }
            b'S' => {
                self.sh.push(Unit::Normal);
                self.synced = true;
                self.forward(&f).await
            }
            b'D' | b'H' | b'd' | b'c' | b'f' => self.forward(&f).await,
            b'X' => {
                let _ = self.forward(&f).await;
                false
            }
            b'F' => {
                // Fast-path function calls reach any function by OID: never allowed.
                self.refuse_simple(Refused {
                    sqlstate: "42501",
                    message: "The fast-path function call interface is not available through the Aurora database lane.".into(),
                    hint: None,
                })
                .await
            }
            _ => {
                self.fatal("08P01", "unsupported frontend message").await;
                false
            }
        }
    }

    fn statement_text(body: &[u8]) -> Result<&str, Refusal> {
        let (s, _) =
            cstr_at(body).ok_or_else(|| Refusal::Unparsable("not valid UTF-8 text".into()))?;
        Ok(s)
    }

    /// Classification and (for non-transaction classes) policy for text about to be executed.
    async fn check(&self, sql: &str) -> Result<Stmt, Refused> {
        let c = classify_async(sql.to_owned())
            .await
            .map_err(|r| classifier_refusal(&r))?;
        Ok(Stmt {
            class: c.class,
            statements: c.statements,
            text: Arc::from(sql),
        })
    }

    async fn authorize(&self, s: &Stmt) -> Result<(), Refused> {
        if s.statements == 0 || s.class == Class::Transaction {
            return Ok(());
        }
        match self.authorizer.authorize(s.class, &s.text).await {
            DbVerdict::Allow => Ok(()),
            DbVerdict::Deny(e) => Err(policy_refusal(&e)),
            DbVerdict::Unavailable(e) => Err(Refused {
                sqlstate: "57P03",
                message: e.message().to_owned(),
                hint: None,
            }),
        }
    }

    async fn simple_query(&mut self, f: &Frame) -> bool {
        let sql = match Self::statement_text(&f.body) {
            Ok(s) => s,
            Err(r) => return self.refuse_simple(classifier_refusal(&r)).await,
        };
        let stmt = match self.check(sql).await {
            Ok(s) => s,
            Err(r) => return self.refuse_simple(r).await,
        };
        if let Err(r) = self.authorize(&stmt).await {
            return self.refuse_simple(r).await;
        }
        self.sh.push(Unit::Normal);
        self.synced = true;
        self.forward(f).await
    }

    /// Portals die with their transaction. Once everything sent has been answered and the
    /// server is idle (`I`), none is left; forgetting them lets the client reuse names.
    fn prune_portals(&mut self) {
        if self.synced
            && self.sh.status.load(Ordering::SeqCst) == b'I'
            && self.sh.queue.lock().is_ok_and(|q| q.is_empty())
        {
            self.portals.clear();
            self.synced = false;
        }
    }

    async fn parse(&mut self, f: &Frame) -> bool {
        let Some((name, rest)) = cstr_at(&f.body) else {
            return self
                .refuse_ext(classifier_refusal(&Refusal::Unparsable(
                    "malformed Parse".into(),
                )))
                .await;
        };
        let name = name.to_owned();
        let sql = match Self::statement_text(rest) {
            Ok(s) => s.to_owned(),
            Err(r) => return self.refuse_ext(classifier_refusal(&r)).await,
        };
        if !name.is_empty() && self.stmts.contains_key(&name) {
            return self
                .refuse_ext(Refused {
                    sqlstate: "42P05",
                    message: format!("prepared statement \"{name}\" already exists"),
                    hint: None,
                })
                .await;
        }
        let stmt = match self.check(&sql).await {
            Ok(s) => s,
            Err(r) => return self.refuse_ext(r).await,
        };
        if self.stmts.len() >= MAX_STATEMENTS
            || self.stmt_bytes + stmt.text.len() > MAX_STATEMENT_TEXT_TOTAL
        {
            return self
                .refuse_ext(Refused {
                    sqlstate: "54000",
                    message: "too many prepared statements on this session".into(),
                    hint: Some("Close prepared statements you no longer need.".into()),
                })
                .await;
        }
        // The unnamed statement is replaced (the server drops it first, even if the new Parse
        // fails, so a stale entry can only lead to an error, never to a wrong authorization).
        if let Some(old) = self.stmts.insert(name, stmt.clone()) {
            self.stmt_bytes = self.stmt_bytes.saturating_sub(old.text.len());
        }
        self.stmt_bytes += stmt.text.len();
        self.forward(f).await
    }

    async fn bind(&mut self, f: &Frame) -> bool {
        self.prune_portals();
        let parsed = cstr_at(&f.body).and_then(|(portal, rest)| {
            let (stmt, _) = cstr_at(rest)?;
            Some((portal.to_owned(), stmt.to_owned()))
        });
        let Some((portal, stmt_name)) = parsed else {
            return self
                .refuse_ext(classifier_refusal(&Refusal::Unparsable(
                    "malformed Bind".into(),
                )))
                .await;
        };
        let Some(stmt) = self.stmts.get(&stmt_name).cloned() else {
            return self
                .refuse_ext(Refused {
                    sqlstate: "26000",
                    message: format!("prepared statement \"{stmt_name}\" does not exist"),
                    hint: None,
                })
                .await;
        };
        if !portal.is_empty() && self.portals.contains_key(&portal) {
            return self
                .refuse_ext(Refused {
                    sqlstate: "42P03",
                    message: format!("cursor \"{portal}\" already exists"),
                    hint: None,
                })
                .await;
        }
        if self.portals.len() >= MAX_PORTALS {
            return self
                .refuse_ext(Refused {
                    sqlstate: "54000",
                    message: "too many portals on this session".into(),
                    hint: None,
                })
                .await;
        }
        self.portals.insert(
            portal,
            Portal {
                stmt,
                authorized: false,
            },
        );
        self.forward(f).await
    }

    async fn execute(&mut self, f: &Frame) -> bool {
        let Some((portal, _)) = cstr_at(&f.body) else {
            return self
                .refuse_ext(classifier_refusal(&Refusal::Unparsable(
                    "malformed Execute".into(),
                )))
                .await;
        };
        let portal = portal.to_owned();
        let Some(p) = self.portals.get(&portal) else {
            return self
                .refuse_ext(Refused {
                    sqlstate: "34000",
                    message: format!("portal \"{portal}\" does not exist"),
                    hint: None,
                })
                .await;
        };
        if !p.authorized {
            let stmt = p.stmt.clone();
            if let Err(r) = self.authorize(&stmt).await {
                return self.refuse_ext(r).await;
            }
            if let Some(p) = self.portals.get_mut(&portal) {
                p.authorized = true;
            }
        }
        self.forward(f).await
    }

    /// Waits for the server to finish everything sent so far (a `Hold` on a synthetic `Sync`).
    async fn quiesce(&self) -> Option<(u8, bool)> {
        let (tx, rx) = oneshot::channel();
        self.sh.push(Unit::Hold(tx));
        if !self.to_server(&wire::encode(b'S', &[])).await {
            return None;
        }
        rx.await.ok()
    }

    /// Refuses in the middle of a pipeline: error now, `ReadyForQuery` at the client's `Sync`.
    async fn refuse_ext(&mut self, r: Refused) -> bool {
        match self.deliver(&r).await {
            Some(status) => {
                self.skip_until_sync = Some(status);
                true
            }
            None => false,
        }
    }

    /// Refuses a simple query (or a function call): error and `ReadyForQuery` together.
    async fn refuse_simple(&mut self, r: Refused) -> bool {
        match self.deliver(&r).await {
            Some(status) => {
                let mut w = self.sh.client.lock().await;
                wire::write_all(&mut *w, &wire::ready_for_query(status))
                    .await
                    .is_ok()
            }
            None => false,
        }
    }

    /// Writes the refusal in order and returns the transaction status to report.
    async fn deliver(&mut self, r: &Refused) -> Option<u8> {
        let (mut status, errored) = self.quiesce().await?;
        if !errored {
            {
                let mut w = self.sh.client.lock().await;
                wire::write_all(
                    &mut *w,
                    &wire::error_response("ERROR", r.sqlstate, &r.message, r.hint.as_deref()),
                )
                .await
                .ok()?;
            }
            if status == b'T' {
                // Abort the server transaction as a real error would.
                let (tx, rx) = oneshot::channel();
                self.sh.push(Unit::Swallow(tx));
                if !self.to_server(&wire::encode(b'Q', b"select 1/0\0")).await {
                    return None;
                }
                status = rx.await.ok()?;
            }
        }
        Some(status)
    }
}
