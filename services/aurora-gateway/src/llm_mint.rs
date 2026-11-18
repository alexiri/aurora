//! (P4-LLM-2, D-261(e), C-807, SEC-97) The gateway's one LLM credential mint: Google tokens today, Entra tokens and STS
//! sessions with their providers. A minted value is produced only here, only from the org's own stored secret, only at the
//! mode's pinned endpoint (the request builder in `aurora-llm` fixes it; `llm.dev_upstream_overrides` may only replace it
//! with a loopback target), through the destination guard, and lives only in this cache's zeroizing memory.
//!
//! Cache key `(secret_id, value_version, mode, region)`, single-flight per key, reuse until `LLM_TOKEN_SKEW` before expiry
//! (never longer than 55 minutes), a failed mint held `LLM_TOKEN_MINT_FAILURE_HOLD` (a burst makes one upstream call), at
//! most `LLM_MINT_CACHE_MAX` entries (least recently used evicted). A value change is a new version, a new key; a
//! deletion, an org or workspace change and `all` drop entries through the invalidation hub.

use crate::invalidation::{CacheStats, Invalidatable, Invalidate, Scope};
use crate::pipeline::{PreparedRequest, Scheme};
use crate::upstream::Upstream;
use aurora_core::Secret;
use aurora_core::limits::{
    LLM_MINT_CACHE_MAX, LLM_TOKEN_MINT_FAILURE_HOLD, LLM_TOKEN_MINT_TIMEOUT, LLM_TOKEN_SKEW,
};
use aurora_llm::mint::{self, MAX_RESPONSE_BYTES, MintCtx, MintError};
use aurora_llm::{AuthMode, Endpoint, Overrides};
use http_body_util::BodyExt;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use uuid::Uuid;

/// A minted credential is never reused for longer than this after it was minted (the `min(expires_in − skew, 55 min)`).
pub const LLM_TOKEN_MAX_AGE: Duration = Duration::from_secs(55 * 60);

type Key = (Uuid, i32, &'static str, String);

enum Slot {
    Ready(Arc<Secret<String>>, Instant),
    /// A failed mint, answered again until the instant.
    Held(MintError, Instant),
}

struct Entry {
    slot: Slot,
    org: Uuid,
    workspace: Option<Uuid>,
    /// Least-recently-used stamp.
    used: u64,
    born: Instant,
}

#[derive(Default)]
struct State {
    entries: HashMap<Key, Entry>,
    inflight: HashMap<Key, Arc<tokio::sync::Mutex<()>>>,
    clock: u64,
}

/// What a request needs minted.
pub struct MintRequest<'a> {
    pub secret_id: Uuid,
    pub version: i32,
    pub org_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub endpoint: &'a Endpoint,
    pub mode: AuthMode,
    /// The secret's opened value.
    pub value: &'a [u8],
    /// The organization's own `external_id` (`aws_assume_role` only: the `ExternalId` of `AssumeRole`, D-261(b)).
    pub org_external_id: Option<&'a str>,
}

pub struct LlmMint {
    upstream: Arc<Upstream>,
    overrides: Overrides,
    state: Mutex<State>,
    cap: usize,
    skew: Duration,
    hold: Duration,
    timeout: Duration,
}

impl LlmMint {
    #[must_use]
    pub fn new(upstream: Arc<Upstream>, overrides: Overrides) -> Self {
        Self {
            upstream,
            overrides,
            state: Mutex::new(State::default()),
            cap: LLM_MINT_CACHE_MAX,
            skew: LLM_TOKEN_SKEW,
            hold: LLM_TOKEN_MINT_FAILURE_HOLD,
            timeout: LLM_TOKEN_MINT_TIMEOUT,
        }
    }

    /// Tests: smaller bound, skew, hold and timeout.
    #[must_use]
    pub fn with_limits(
        mut self,
        cap: usize,
        skew: Duration,
        hold: Duration,
        timeout: Duration,
    ) -> Self {
        (self.cap, self.skew, self.hold, self.timeout) = (cap, skew, hold, timeout);
        self
    }

    fn count(mode: AuthMode, outcome: &'static str) {
        metrics::counter!(
            "aurora_gateway_llm_mint_total",
            "mode" => mode.as_str(),
            "outcome" => outcome
        )
        .increment(1);
    }

    fn lookup(&self, key: &Key) -> Option<Result<Arc<Secret<String>>, MintError>> {
        let mut st = self.state.lock().ok()?;
        st.clock += 1;
        let clock = st.clock;
        let now = Instant::now();
        let e = st.entries.get_mut(key)?;
        let live = match &e.slot {
            Slot::Ready(_, until) | Slot::Held(_, until) => *until > now,
        };
        if !live {
            st.entries.remove(key);
            return None;
        }
        e.used = clock;
        Some(match &e.slot {
            Slot::Ready(t, _) => Ok(t.clone()),
            Slot::Held(err, _) => Err(err.clone()),
        })
    }

    /// The credential for the request: cached, or minted once however many requests ask at the same time.
    ///
    /// # Errors
    /// [`MintError`]: `Rejected` (the secret or provider refused it) or `Unavailable`; the same error is answered
    /// for `LLM_TOKEN_MINT_FAILURE_HOLD` without calling the endpoint again.
    pub async fn token(&self, r: &MintRequest<'_>) -> Result<Arc<Secret<String>>, MintError> {
        let spec = mint::spec(r.mode).ok_or_else(MintError::unavailable)?;
        let key: Key = (
            r.secret_id,
            r.version,
            r.mode.as_str(),
            spec.region(r.endpoint),
        );
        match self.lookup(&key) {
            Some(Ok(t)) => {
                Self::count(r.mode, "cached");
                return Ok(t);
            }
            Some(Err(e)) => {
                Self::count(r.mode, "held");
                return Err(e);
            }
            None => {}
        }
        let gate = {
            let mut st = self.state.lock().map_err(|_| MintError::unavailable())?;
            st.inflight.entry(key.clone()).or_default().clone()
        };
        let _flight = gate.lock().await;
        // Another request may have minted (or failed) while this one waited.
        match self.lookup(&key) {
            Some(Ok(t)) => {
                Self::count(r.mode, "cached");
                return Ok(t);
            }
            Some(Err(e)) => {
                Self::count(r.mode, "held");
                return Err(e);
            }
            None => {}
        }
        let result = self.call(spec, r).await;
        self.store(key, r, &result);
        match &result {
            Ok(_) => Self::count(r.mode, "minted"),
            Err(e) => Self::count(
                r.mode,
                match e.class {
                    mint::MintClass::Rejected => "rejected",
                    mint::MintClass::Unavailable => "unavailable",
                },
            ),
        }
        result.map(|(t, _)| t)
    }

    fn store(
        &self,
        key: Key,
        r: &MintRequest<'_>,
        result: &Result<(Arc<Secret<String>>, Duration), MintError>,
    ) {
        let Ok(mut st) = self.state.lock() else {
            return;
        };
        st.inflight.remove(&key);
        let now = Instant::now();
        let slot = match result {
            Ok((t, ttl)) => Slot::Ready(t.clone(), now + *ttl),
            Err(e) => Slot::Held(e.clone(), now + self.hold),
        };
        while st.entries.len() >= self.cap && !st.entries.contains_key(&key) {
            let Some(oldest) = st
                .entries
                .iter()
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            st.entries.remove(&oldest);
        }
        st.clock += 1;
        let used = st.clock;
        st.entries.insert(
            key,
            Entry {
                slot,
                org: r.org_id,
                workspace: r.workspace_id,
                used,
                born: now,
            },
        );
    }

    /// One call to the pinned endpoint; the reuse window of the credential comes with it.
    async fn call(
        &self,
        spec: &dyn mint::MintSpec,
        r: &MintRequest<'_>,
    ) -> Result<(Arc<Secret<String>>, Duration), MintError> {
        let now = SystemTime::now();
        let org = r.org_id.to_string();
        let ctx = MintCtx {
            endpoint: r.endpoint,
            value: r.value,
            org_id: &org,
            org_external_id: r.org_external_id,
            now,
            overrides: &self.overrides,
        };
        let req = spec.build(&ctx)?;
        let prepared = prepared(req).ok_or_else(MintError::unavailable)?;
        let sent = tokio::time::timeout(self.timeout, async {
            let resp = self
                .upstream
                .send(prepared)
                .await
                .map_err(|_| MintError::unavailable())?;
            let (parts, mut body) = resp.into_parts();
            let mut buf: Vec<u8> = Vec::new();
            while let Some(frame) = body.frame().await {
                let frame = frame.map_err(|_| MintError::unavailable())?;
                if let Some(d) = frame.data_ref() {
                    if buf.len() + d.len() > MAX_RESPONSE_BYTES {
                        return Err(MintError::unavailable());
                    }
                    buf.extend_from_slice(d);
                }
            }
            Ok((parts.status.as_u16(), parts.headers, buf))
        })
        .await
        .map_err(|_| MintError::unavailable())??;
        let (status, headers, body) = sent;
        let minted = spec.parse(&ctx, status, &headers, &body)?;
        let ttl = minted
            .expires_at
            .duration_since(SystemTime::now())
            .unwrap_or_default()
            .saturating_sub(self.skew)
            .min(LLM_TOKEN_MAX_AGE);
        Ok((Arc::new(minted.material), ttl))
    }
}

/// The built request as a guarded upstream request (scheme, host, port, path, query, headers, body).
fn prepared(req: http::Request<Vec<u8>>) -> Option<PreparedRequest> {
    let (parts, body) = req.into_parts();
    let scheme = match parts.uri.scheme_str()? {
        "https" => Scheme::Https,
        "http" => Scheme::Http,
        _ => return None,
    };
    let host = parts.uri.host()?.trim_matches(['[', ']']).to_owned();
    let port = parts
        .uri
        .port_u16()
        .unwrap_or(if scheme == Scheme::Https { 443 } else { 80 });
    Some(PreparedRequest {
        scheme,
        host,
        port,
        method: parts.method,
        path: parts.uri.path().to_owned(),
        query: parts.uri.query().map(str::to_owned),
        headers: parts.headers,
        upgrade: None,
        body: crate::full_body(body),
        hook: None,
        response_headers: http::HeaderMap::new(),
        retry: None,
        ldap: None,
        link: None,
    })
}

impl Invalidatable for LlmMint {
    fn invalidate(&self, ev: &Invalidate) {
        let Ok(mut st) = self.state.lock() else {
            return;
        };
        match ev.scope {
            Scope::Org(id) => st.entries.retain(|_, e| e.org != id),
            Scope::Workspace(id) => st.entries.retain(|_, e| e.workspace != Some(id)),
            Scope::Connection(_) | Scope::All => st.entries.clear(),
            Scope::Agent(_) | Scope::Credential(_) | Scope::User(_) => {}
        }
    }

    fn flush_all(&self) {
        if let Ok(mut st) = self.state.lock() {
            st.entries.clear();
        }
    }

    fn stats(&self) -> CacheStats {
        let Ok(st) = self.state.lock() else {
            return CacheStats::default();
        };
        let now = Instant::now();
        CacheStats {
            entries: st.entries.len(),
            oldest_age: st.entries.values().map(|e| now - e.born).max(),
        }
    }
}
