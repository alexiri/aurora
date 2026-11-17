//! In-process mock **push service** for the Web Push tests (P4-011B-3, C-839, D-267(b)).
//!
//! It listens on a loopback port; the API reaches it through `push.dev_endpoint_overrides`
//! (`{ "<allow-listed push host>": "http://127.0.0.1:<port>" }`), so it checks exactly what a real
//! push service does, with an independent implementation of the receiving side:
//!
//! * the VAPID JWT (RFC 8292): ES256 signature with the `k=` key, header `{typ, alg}`, `aud` equal
//!   to the real origin the subscription names, `exp` in the future and at most one hour (plus a
//!   little margin) away, `sub`, and the `k=` key equal to the expected application-server key;
//! * `aes128gcm` decryption (RFC 8291) with the test subscription's private key and auth secret;
//! * it records `TTL`, `Urgency`, `Topic`, the path and the plaintext of every request, and
//!   answers what the test scripted: a status (201/404/410/400/403/413/429/5xx), a `Retry-After`,
//!   a delay (a slow service), a redirect, or an oversized body.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aws_lc_rs::aead::{AES_128_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use aws_lc_rs::hkdf::{HKDF_SHA256, KeyType, Salt};
use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::Response;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use bytes::Bytes;
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use p256::{PublicKey, SecretKey};

/// A test browser's subscription keys.
#[derive(Clone)]
pub struct SubscriberKeys {
    secret: SecretKey,
    auth: [u8; 16],
}

impl SubscriberKeys {
    fn generate() -> Self {
        let mut auth = [0u8; 16];
        aws_lc_rs::rand::fill(&mut auth).expect("rng");
        let secret = loop {
            let mut b = [0u8; 32];
            aws_lc_rs::rand::fill(&mut b).expect("rng");
            if let Ok(k) = SecretKey::from_slice(&b) {
                break k;
            }
        };
        Self { secret, auth }
    }

    /// `keys.p256dh` as a browser reports it: the 65-byte uncompressed point, base64url.
    pub fn p256dh(&self) -> String {
        use p256::elliptic_curve::sec1::ToSec1Point;
        B64.encode(self.secret.public_key().to_sec1_point(false).as_bytes())
    }

    /// `keys.auth`: 16 bytes, base64url.
    pub fn auth(&self) -> String {
        B64.encode(self.auth)
    }
}

/// What the mock should expect of the VAPID header.
#[derive(Debug, Clone)]
pub struct VapidExpect {
    /// The real origin of the subscription, e.g. `https://fcm.googleapis.com`.
    pub aud: String,
    /// `push.vapid.subject`.
    pub subject: String,
    /// The application-server public key, base64url (what `GET /v1/push/config` serves).
    pub public_key: String,
}

/// How the mock answers.
#[derive(Debug, Clone)]
pub struct Answer {
    pub status: u16,
    pub retry_after: Option<u64>,
    /// Wait this long before answering (a slow service).
    pub delay: Duration,
    /// Answer 307 to this location instead of `status`.
    pub redirect_to: Option<String>,
    /// Send a body of this many bytes (an oversized answer).
    pub body_len: usize,
}

impl Answer {
    pub fn status(status: u16) -> Self {
        Self {
            status,
            retry_after: None,
            delay: Duration::ZERO,
            redirect_to: None,
            body_len: 0,
        }
    }

    pub fn created() -> Self {
        Self::status(201)
    }
}

/// One request the mock received.
#[derive(Debug, Clone)]
pub struct Received {
    pub path: String,
    pub ttl: Option<String>,
    pub urgency: Option<String>,
    pub topic: Option<String>,
    pub content_encoding: Option<String>,
    /// `Ok(())` when the VAPID header verified against the expectation, else why not.
    pub vapid: Result<(), String>,
    /// The decrypted plaintext (`None` when no subscriber is registered for the path or the
    /// ciphertext did not decrypt).
    pub plaintext: Option<Vec<u8>>,
    pub body_len: usize,
}

impl Received {
    /// The plaintext as JSON.
    pub fn json(&self) -> Option<serde_json::Value> {
        serde_json::from_slice(self.plaintext.as_ref()?).ok()
    }
}

#[derive(Default)]
struct Inner {
    subscribers: Mutex<HashMap<String, SubscriberKeys>>,
    expect: Mutex<Option<VapidExpect>>,
    standing: Mutex<Option<Answer>>,
    queue: Mutex<VecDeque<Answer>>,
    received: Mutex<Vec<Received>>,
}

/// The mock push service; dropping it stops the server.
pub struct MockPush {
    inner: Arc<Inner>,
    addr: std::net::SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockPush {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockPush {
    pub async fn start() -> Self {
        let inner = Arc::new(Inner::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the mock push service");
        let addr = listener.local_addr().expect("addr");
        let app = Router::new().fallback(handle).with_state(inner.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self { inner, addr, task }
    }

    /// The loopback URL for `push.dev_endpoint_overrides`.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The loopback port.
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// A new test browser reachable at `path`: its keys are what the API is given, and the mock
    /// decrypts what arrives there with them.
    pub fn subscribe(&self, path: &str) -> SubscriberKeys {
        let k = SubscriberKeys::generate();
        self.inner
            .subscribers
            .lock()
            .expect("subscribers")
            .insert(path.to_owned(), k.clone());
        k
    }

    /// Verify the VAPID header of every request against this expectation.
    pub fn expect_vapid(&self, e: VapidExpect) {
        *self.inner.expect.lock().expect("expect") = Some(e);
    }

    /// Answer every request like this from now on (default: 201).
    pub fn respond(&self, a: Answer) {
        *self.inner.standing.lock().expect("standing") = Some(a);
    }

    /// Answer the next request like this (queued in order, before the standing answer).
    pub fn respond_once(&self, a: Answer) {
        self.inner.queue.lock().expect("queue").push_back(a);
    }

    pub fn received(&self) -> Vec<Received> {
        self.inner.received.lock().expect("received").clone()
    }

    pub fn clear(&self) {
        self.inner.received.lock().expect("received").clear();
    }

    /// Wait until at least `n` requests arrived (polling, bounded), then return them all.
    pub async fn wait_for(&self, n: usize, within: Duration) -> Vec<Received> {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let r = self.received();
            if r.len() >= n || tokio::time::Instant::now() >= deadline {
                return r;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

async fn handle(
    State(s): State<Arc<Inner>>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = uri.path().to_owned();
    let h = |n: &str| {
        headers
            .get(n)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let expect = s.expect.lock().expect("expect").clone();
    let vapid = match (&expect, h("authorization")) {
        (_, None) => Err("no Authorization header".to_owned()),
        (None, Some(_)) => Ok(()),
        (Some(e), Some(a)) => verify_vapid(&a, e),
    };
    let sub = s
        .subscribers
        .lock()
        .expect("subscribers")
        .get(&path)
        .cloned();
    let plaintext = sub.and_then(|k| decrypt(&k, &body).ok());
    s.received.lock().expect("received").push(Received {
        path,
        ttl: h("ttl"),
        urgency: h("urgency"),
        topic: h("topic"),
        content_encoding: h("content-encoding"),
        vapid,
        plaintext,
        body_len: body.len(),
    });
    let answer = {
        let queued = s.queue.lock().expect("queue").pop_front();
        queued
            .or_else(|| s.standing.lock().expect("standing").clone())
            .unwrap_or_else(Answer::created)
    };
    if !answer.delay.is_zero() {
        tokio::time::sleep(answer.delay).await;
    }
    let mut r = Response::builder();
    if let Some(to) = &answer.redirect_to {
        r = r
            .status(StatusCode::TEMPORARY_REDIRECT)
            .header(header::LOCATION, to);
    } else {
        r = r.status(
            StatusCode::from_u16(answer.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        );
    }
    if let Some(secs) = answer.retry_after {
        r = r.header(header::RETRY_AFTER, secs.to_string());
    }
    r.body(Body::from(vec![b'x'; answer.body_len]))
        .expect("response")
}

fn verify_vapid(authorization: &str, e: &VapidExpect) -> Result<(), String> {
    let rest = authorization
        .strip_prefix("vapid ")
        .ok_or("scheme is not vapid")?;
    let (mut t, mut k) = (None, None);
    for part in rest.split(',') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix("t=") {
            t = Some(v);
        } else if let Some(v) = part.strip_prefix("k=") {
            k = Some(v);
        }
    }
    let (jwt, key) = (t.ok_or("no t=")?, k.ok_or("no k=")?);
    if key != e.public_key {
        return Err("k= is not the expected application-server key".into());
    }
    let parts: Vec<&str> = jwt.split('.').collect();
    if parts.len() != 3 {
        return Err("jwt is not three parts".into());
    }
    let decode = |s: &str| {
        B64.decode(s)
            .map_err(|_| "jwt part is not base64url".to_owned())
    };
    let header: serde_json::Value =
        serde_json::from_slice(&decode(parts[0])?).map_err(|_| "bad header")?;
    if header["alg"] != "ES256" || header["typ"] != "JWT" {
        return Err("header is not {typ: JWT, alg: ES256}".into());
    }
    let claims: serde_json::Value =
        serde_json::from_slice(&decode(parts[1])?).map_err(|_| "bad claims")?;
    if claims["aud"] != e.aud.as_str() {
        return Err(format!("aud is {} (expected {})", claims["aud"], e.aud));
    }
    if claims["sub"] != e.subject.as_str() {
        return Err("sub is not the configured subject".into());
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs() as i64;
    let exp = claims["exp"].as_i64().ok_or("no exp")?;
    if exp <= now || exp > now + 3600 + 5 {
        return Err(format!("exp {exp} is not within an hour of {now}"));
    }
    let pk = PublicKey::from_sec1_bytes(&decode(key)?).map_err(|_| "k= is not a P-256 point")?;
    let vk = VerifyingKey::from(&pk);
    let sig = Signature::from_slice(&decode(parts[2])?).map_err(|_| "signature is not r||s")?;
    vk.verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig)
        .map_err(|_| "ES256 signature does not verify".to_owned())
}

struct Len(usize);
impl KeyType for Len {
    fn len(&self) -> usize {
        self.0
    }
}

fn hkdf(salt: &[u8], ikm: &[u8], info: &[&[u8]], out: &mut [u8]) -> Result<(), String> {
    Salt::new(HKDF_SHA256, salt)
        .extract(ikm)
        .expand(info, Len(out.len()))
        .and_then(|o| o.fill(out))
        .map_err(|_| "hkdf".to_owned())
}

/// RFC 8291 `aes128gcm` decryption of one record with the subscriber's keys.
fn decrypt(k: &SubscriberKeys, body: &[u8]) -> Result<Vec<u8>, String> {
    use p256::elliptic_curve::sec1::ToSec1Point;
    if body.len() < 16 + 4 + 1 {
        return Err("short".into());
    }
    let salt = &body[..16];
    let idlen = body[20] as usize;
    if body.len() < 21 + idlen + 16 {
        return Err("short".into());
    }
    let as_public = &body[21..21 + idlen];
    let record = &body[21 + idlen..];
    let as_pk = PublicKey::from_sec1_bytes(as_public).map_err(|_| "as key")?;
    let shared = p256::ecdh::diffie_hellman(k.secret.to_nonzero_scalar(), as_pk.as_affine());
    let ua_public = k.secret.public_key().to_sec1_point(false);
    let mut ikm = [0u8; 32];
    hkdf(
        &k.auth,
        shared.raw_secret_bytes().as_slice(),
        &[b"WebPush: info\0", ua_public.as_bytes(), as_public],
        &mut ikm,
    )?;
    let mut cek = [0u8; 16];
    hkdf(salt, &ikm, &[b"Content-Encoding: aes128gcm\0"], &mut cek)?;
    let mut nonce = [0u8; 12];
    hkdf(salt, &ikm, &[b"Content-Encoding: nonce\0"], &mut nonce)?;
    let key = LessSafeKey::new(UnboundKey::new(&AES_128_GCM, &cek).map_err(|_| "cek")?);
    let mut buf = record.to_vec();
    let plain = key
        .open_in_place(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut buf)
        .map_err(|_| "decrypt")?;
    // Strip the padding: zeros after the delimiter 0x02.
    let end = plain.iter().rposition(|b| *b != 0).ok_or("empty")?;
    if plain[end] != 2 {
        return Err("no delimiter".into());
    }
    Ok(plain[..end].to_vec())
}
