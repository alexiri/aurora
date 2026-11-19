//! The `X-Aurora-Signature` value (C-910): `t=<unix>,v1=<hex HMAC-SHA256(secret, "<t>.<body>")>`.
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

/// The header value for a request body sent at unix second `t`. A receiver recomputes the HMAC
/// over `"<t>.<raw body>"` with the signing secret and compares in constant time; it should also
/// reject a `t` that is too old.
#[must_use]
pub fn sign(secret: &[u8], t: u64, body: &[u8]) -> String {
    // HMAC accepts a key of any length, so `new_from_slice` cannot fail.
    let Ok(mut mac) = <Hmac<Sha256> as KeyInit>::new_from_slice(secret) else {
        return String::new();
    };
    mac.update(t.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    format!("t={t},v1={}", hex::encode(mac.finalize().into_bytes()))
}
