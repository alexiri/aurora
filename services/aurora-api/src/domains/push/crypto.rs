//! Web Push cryptography (phase 5, P4-011B-3, D-134, C-832, C-833; SEC-75, SEC-76): the VAPID ES256
//! key and its JWT (RFC 8292) and the `aes128gcm` payload encryption (RFC 8291). Nothing here logs
//! or returns key material; the private key is a [`p256::SecretKey`] that zeroizes on drop.

use aurora_core::limits::PUSH_VAPID_JWT_TTL;
use aws_lc_rs::aead::{AES_128_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use aws_lc_rs::hkdf::{HKDF_SHA256, KeyType, Salt};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use p256::ecdsa::SigningKey;
use p256::ecdsa::signature::Signer;
use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey};
use p256::{PublicKey, SecretKey};

/// RFC 8291 single-record size (the largest ciphertext record of one message).
const RECORD_SIZE: u32 = 4096;

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("random generation failed")]
    Rng,
    #[error("invalid key")]
    Key,
    #[error("encryption failed")]
    Encrypt,
}

pub fn b64u(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Decode unpadded (or padded) base64url or standard base64, as browsers produce either.
pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let t = s.trim().trim_end_matches('=');
    URL_SAFE_NO_PAD.decode(t).ok().or_else(|| {
        base64::engine::general_purpose::STANDARD_NO_PAD
            .decode(t)
            .ok()
    })
}

fn random<const N: usize>() -> Result<[u8; N], CryptoError> {
    let mut b = [0u8; N];
    aws_lc_rs::rand::fill(&mut b).map_err(|_| CryptoError::Rng)?;
    Ok(b)
}

fn new_secret() -> Result<SecretKey, CryptoError> {
    // A uniformly random 32-byte string is a valid scalar except with probability ~2^-32.
    for _ in 0..8 {
        let b: [u8; 32] = random()?;
        if let Ok(k) = SecretKey::from_slice(&b) {
            return Ok(k);
        }
    }
    Err(CryptoError::Rng)
}

fn public_bytes(k: &PublicKey) -> Vec<u8> {
    use p256::elliptic_curve::sec1::ToSec1Point;
    k.to_sec1_point(false).as_bytes().to_vec()
}

/// An instance VAPID key (RFC 8292): the P-256 signing key and its 65-byte public point.
#[derive(Clone)]
pub struct VapidKey {
    secret: SecretKey,
    public: Vec<u8>,
}

impl std::fmt::Debug for VapidKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VapidKey").finish_non_exhaustive()
    }
}

impl VapidKey {
    pub fn generate() -> Result<Self, CryptoError> {
        Ok(Self::from_secret(new_secret()?))
    }

    fn from_secret(secret: SecretKey) -> Self {
        let public = public_bytes(&secret.public_key());
        Self { secret, public }
    }

    /// PKCS#8 DER, the form sealed in the inline store.
    pub fn from_pkcs8_der(der: &[u8]) -> Result<Self, CryptoError> {
        SecretKey::from_pkcs8_der(der)
            .map(Self::from_secret)
            .map_err(|_| CryptoError::Key)
    }

    pub fn to_pkcs8_der(&self) -> Result<Vec<u8>, CryptoError> {
        self.secret
            .to_pkcs8_der()
            .map(|d| d.as_bytes().to_vec())
            .map_err(|_| CryptoError::Key)
    }

    /// An operator-managed key (C-833): PKCS#8 PEM or the base64url raw 32-byte scalar.
    pub fn from_operator_value(v: &str) -> Result<Self, CryptoError> {
        let v = v.trim();
        if v.starts_with("-----BEGIN") {
            return SecretKey::from_pkcs8_pem(v)
                .map(Self::from_secret)
                .map_err(|_| CryptoError::Key);
        }
        let raw = b64_decode(v).ok_or(CryptoError::Key)?;
        if raw.len() != 32 {
            return Err(CryptoError::Key);
        }
        SecretKey::from_slice(&raw)
            .map(Self::from_secret)
            .map_err(|_| CryptoError::Key)
    }

    /// base64url of the 65-byte uncompressed point (`applicationServerKey`, `k=`).
    pub fn public_b64u(&self) -> String {
        b64u(&self.public)
    }

    /// A signed VAPID JWT (`typ` JWT, `alg` ES256, claims `aud`, `exp`, `sub`), ES256 in the JWS
    /// fixed `r ‖ s` form.
    pub fn jwt(&self, aud: &str, exp_unix: i64, sub: &str) -> String {
        let header = b64u(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = b64u(
            serde_json::json!({ "aud": aud, "exp": exp_unix, "sub": sub })
                .to_string()
                .as_bytes(),
        );
        let input = format!("{header}.{claims}");
        let sig: p256::ecdsa::Signature = SigningKey::from(&self.secret).sign(input.as_bytes());
        format!("{input}.{}", b64u(&sig.to_bytes()))
    }
}

/// The lifetime of one cached JWT: it is reused until 5 minutes before `exp` (C-833).
pub const JWT_REUSE_MARGIN_SECS: i64 = 300;

/// `exp` for a JWT created at `now_unix`.
pub fn jwt_exp(now_unix: i64) -> i64 {
    now_unix + PUSH_VAPID_JWT_TTL.as_secs() as i64
}

/// A validated subscription key pair: `p256dh` a 65-byte point on P-256, `auth` 16 bytes.
pub struct SubscriptionKeys {
    pub public: PublicKey,
    pub public_raw: Vec<u8>,
    pub auth: [u8; 16],
}

pub fn parse_subscription_keys(p256dh: &str, auth: &str) -> Result<SubscriptionKeys, CryptoError> {
    let raw = b64_decode(p256dh).ok_or(CryptoError::Key)?;
    if raw.len() != 65 || raw[0] != 0x04 {
        return Err(CryptoError::Key);
    }
    let public = PublicKey::from_sec1_bytes(&raw).map_err(|_| CryptoError::Key)?;
    let a = b64_decode(auth).ok_or(CryptoError::Key)?;
    let auth: [u8; 16] = a.try_into().map_err(|_| CryptoError::Key)?;
    Ok(SubscriptionKeys {
        public,
        public_raw: raw,
        auth,
    })
}

struct Len(usize);
impl KeyType for Len {
    fn len(&self) -> usize {
        self.0
    }
}

fn expand(prk: &aws_lc_rs::hkdf::Prk, info: &[&[u8]], out: &mut [u8]) -> Result<(), CryptoError> {
    prk.expand(info, Len(out.len()))
        .and_then(|o| o.fill(out))
        .map_err(|_| CryptoError::Encrypt)
}

/// RFC 8291 `aes128gcm` encryption of `plaintext` for one subscription with a fresh ephemeral key
/// and salt (one record, padding delimiter `0x02`).
pub fn encrypt(
    plaintext: &[u8],
    ua_public: &PublicKey,
    auth: &[u8; 16],
) -> Result<Vec<u8>, CryptoError> {
    let as_secret = new_secret()?;
    let salt: [u8; 16] = random()?;
    encrypt_with(plaintext, ua_public, auth, &as_secret, &salt)
}

/// [`encrypt`] with the sender's ephemeral key and salt given (RFC 8291 Appendix A vector).
pub fn encrypt_with(
    plaintext: &[u8],
    ua_public: &PublicKey,
    auth: &[u8; 16],
    as_secret: &SecretKey,
    salt: &[u8; 16],
) -> Result<Vec<u8>, CryptoError> {
    let as_public = public_bytes(&as_secret.public_key());
    let ua_raw = public_bytes(ua_public);
    let shared = p256::ecdh::diffie_hellman(as_secret.to_nonzero_scalar(), ua_public.as_affine());
    let prk_key = Salt::new(HKDF_SHA256, auth).extract(shared.raw_secret_bytes().as_slice());
    let mut ikm = [0u8; 32];
    expand(
        &prk_key,
        &[b"WebPush: info\0", &ua_raw, &as_public],
        &mut ikm,
    )?;
    let prk = Salt::new(HKDF_SHA256, salt).extract(&ikm);
    let mut cek = [0u8; 16];
    expand(&prk, &[b"Content-Encoding: aes128gcm\0"], &mut cek)?;
    let mut nonce = [0u8; 12];
    expand(&prk, &[b"Content-Encoding: nonce\0"], &mut nonce)?;

    let mut record = Vec::with_capacity(plaintext.len() + 1 + 16);
    record.extend_from_slice(plaintext);
    record.push(0x02);
    let key =
        LessSafeKey::new(UnboundKey::new(&AES_128_GCM, &cek).map_err(|_| CryptoError::Encrypt)?);
    key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(nonce),
        Aad::empty(),
        &mut record,
    )
    .map_err(|_| CryptoError::Encrypt)?;

    let mut body = Vec::with_capacity(16 + 4 + 1 + 65 + record.len());
    body.extend_from_slice(salt);
    body.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    body.push(as_public.len() as u8);
    body.extend_from_slice(&as_public);
    body.extend_from_slice(&record);
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unb64(s: &str) -> Vec<u8> {
        b64_decode(s).expect("vector decodes")
    }

    /// RFC 8291 Appendix A: the ciphertext of the worked example, byte for byte.
    #[test]
    fn rfc8291_appendix_a_vector() {
        let plaintext = b"When I grow up, I want to be a watermelon";
        let as_private = unb64("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw");
        let ua_public = unb64(
            "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4",
        );
        let auth = unb64("BTBZMqHH6r4Tts7J_aSIgg");
        let salt = unb64("DGv6ra1nlYgDCS1FRnbzlw");
        let expected = unb64(
            "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN",
        );
        let secret = SecretKey::from_slice(&as_private).expect("private");
        let ua = PublicKey::from_sec1_bytes(&ua_public).expect("point");
        let got = encrypt_with(
            plaintext,
            &ua,
            &auth.clone().try_into().expect("16"),
            &secret,
            &salt.try_into().expect("16"),
        )
        .expect("encrypts");
        assert_eq!(b64u(&got), b64u(&expected));
    }

    /// RFC 8292 §2: the example JWT's header and claims encode as published, and a JWT we sign
    /// verifies under the key's public point.
    #[test]
    fn rfc8292_jwt_shape_and_signature() {
        // §2 example (header + claims, signature differs per signer).
        let header = b64u(br#"{"typ":"JWT","alg":"ES256"}"#);
        assert_eq!(header, "eyJ0eXAiOiJKV1QiLCJhbGciOiJFUzI1NiJ9");
        let k = VapidKey::generate().expect("key");
        let jwt = k.jwt(
            "https://push.example.net",
            1_453_523_768,
            "mailto:ops@example.org",
        );
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], header);
        let claims: serde_json::Value =
            serde_json::from_slice(&unb64(parts[1])).expect("claims json");
        assert_eq!(claims["aud"], "https://push.example.net");
        assert_eq!(claims["exp"], 1_453_523_768);
        assert_eq!(claims["sub"], "mailto:ops@example.org");
        use p256::ecdsa::signature::Verifier;
        let vk = p256::ecdsa::VerifyingKey::from_sec1_bytes(&unb64(&k.public_b64u())).expect("vk");
        let sig = p256::ecdsa::Signature::from_slice(&unb64(parts[2])).expect("fixed r||s");
        vk.verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig)
            .expect("ES256 verifies");
    }

    #[test]
    fn key_round_trips_and_operator_forms_parse() {
        let k = VapidKey::generate().expect("key");
        let der = k.to_pkcs8_der().expect("der");
        let back = VapidKey::from_pkcs8_der(&der).expect("from der");
        assert_eq!(k.public_b64u(), back.public_b64u());
        assert_eq!(unb64(&k.public_b64u()).len(), 65);
        // base64url raw scalar and PEM.
        let scalar = b64u(k.secret.to_bytes().as_slice());
        assert_eq!(
            VapidKey::from_operator_value(&scalar)
                .expect("scalar")
                .public_b64u(),
            k.public_b64u()
        );
        let pem = k.secret.to_pkcs8_pem(Default::default()).expect("pem");
        assert_eq!(
            VapidKey::from_operator_value(pem.as_str())
                .expect("pem")
                .public_b64u(),
            k.public_b64u()
        );
        for bad in ["", "not a key", &b64u(&[1u8; 31]), &b64u(&[0u8; 32])] {
            assert!(VapidKey::from_operator_value(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn subscription_keys_are_validated() {
        let k = VapidKey::generate().expect("key");
        let auth = b64u(&[7u8; 16]);
        assert!(parse_subscription_keys(&k.public_b64u(), &auth).is_ok());
        assert!(parse_subscription_keys(&k.public_b64u(), &b64u(&[7u8; 15])).is_err());
        assert!(
            parse_subscription_keys(&b64u(&[4u8; 65]), &auth).is_err(),
            "off the curve"
        );
        assert!(
            parse_subscription_keys(&b64u(&[4u8; 64]), &auth).is_err(),
            "wrong length"
        );
    }
}
