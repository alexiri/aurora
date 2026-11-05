#![allow(clippy::unwrap_used, clippy::expect_used)]

use aurora_core::secret::Secret;
use aurora_crypto::*;
use uuid::Uuid;

const K1: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";
const K2: &str = "ICEiIyQlJicoKSorLC0uLzAxMjM0NTY3ODk6Ozw9Pj8=";

fn ring(active: &str) -> Keyring {
    Keyring::parse(&format!("k1:{K1},k2:{K2}"), active).unwrap()
}
fn aad(p: &str, o: u128, w: u128) -> Aad<'_> {
    Aad {
        purpose: p,
        org_id: Uuid::from_u128(o),
        owner_id: Uuid::from_u128(w),
    }
}

#[test]
fn aad_rendering() {
    assert_eq!(
        aad("conn", 1, 2).render(),
        "aurora:v1:conn:00000000-0000-0000-0000-000000000001:00000000-0000-0000-0000-000000000002"
    );
}

#[test]
fn round_trip_and_row_swap() {
    let r = ring("k1");
    let a = aad("conn", 1, 2);
    let s = r.seal(&a, b"hunter2").unwrap();
    assert!(s.starts_with("inline:v1:k1:"));
    assert_eq!(r.open(&a, &s).unwrap().expose(), b"hunter2");
    assert_ne!(s, r.seal(&a, b"hunter2").unwrap());
    for bad in [aad("other", 1, 2), aad("conn", 9, 2), aad("conn", 1, 9)] {
        assert_eq!(r.open(&bad, &s).unwrap_err(), CryptoError::Decrypt);
    }
}

#[test]
fn tamper_and_malformed() {
    let r = ring("k1");
    let a = aad("conn", 1, 2);
    let s = r.seal(&a, b"x").unwrap();
    let (head, body) = s.rsplit_once(':').unwrap();
    let mut b = body.as_bytes().to_vec();
    let i = b.len() - 3;
    b[i] = if b[i] == b'A' { b'B' } else { b'A' };
    let t = format!("{head}:{}", String::from_utf8(b).unwrap());
    assert!(r.open(&a, &t).is_err());
    assert_eq!(r.open(&a, "garbage").unwrap_err(), CryptoError::Malformed);
    assert_eq!(
        r.open(&a, "inline:v1:k1:AAAA").unwrap_err(),
        CryptoError::Malformed
    );
    let unknown = s.replacen(":k1:", ":zz:", 1);
    assert_eq!(r.open(&a, &unknown).unwrap_err(), CryptoError::UnknownKey);
}

#[test]
fn rotation() {
    let a = aad("conn", 1, 2);
    let s = ring("k1").seal(&a, b"v").unwrap();
    let r2 = ring("k2");
    assert!(r2.needs_rekey(&s));
    assert_eq!(r2.open(&a, &s).unwrap().expose(), b"v");
    let s2 = r2.seal(&a, b"v").unwrap();
    assert!(!r2.needs_rekey(&s2));
}

#[test]
fn keyring_validation() {
    assert!(Keyring::parse(&format!("k1:{K1}"), "k2").is_err());
    assert!(Keyring::parse("", "k1").is_err());
    assert!(Keyring::parse("K1:AAAA", "K1").is_err());
    assert!(Keyring::parse("k1:AAAA", "k1").is_err());
    assert!(Keyring::parse(&format!("k1:{K1},k1:{K2}"), "k1").is_err());
    assert!(Keyring::parse(&format!("{}:{K1}", "a".repeat(17)), "a").is_err());
    let e = Keyring::parse("k1:!!!", "k1").unwrap_err().to_string();
    assert!(!e.contains("!!!"));
}

#[test]
fn fixture() {
    let f: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/inline_v1.json")).unwrap();
    let s = |k: &str| f[k].as_str().unwrap().to_owned();
    let r = Keyring::parse(&s("keys"), &s("active")).unwrap();
    let a = Aad {
        purpose: f["purpose"].as_str().unwrap(),
        org_id: s("org_id").parse().unwrap(),
        owner_id: s("owner_id").parse().unwrap(),
    };
    assert_eq!(
        r.open(&a, &s("sealed")).unwrap().expose(),
        s("plaintext").as_bytes()
    );
}

#[test]
#[ignore = "prints a new fixture"]
fn print_fixture() {
    let r = ring("k1");
    let a = aad("connection", 0x11, 0x22);
    println!("{}", r.seal(&a, b"fixture-plaintext").unwrap());
}

fn dk() -> DerivedKeys {
    DerivedKeys::new(Secret::new([7u8; 32])).unwrap()
}

#[test]
fn derived_purposes_distinct() {
    let d = dk();
    let tags: Vec<_> = Purpose::ALL.iter().map(|p| d.sign(*p, b"m")).collect();
    for i in 0..tags.len() {
        for j in i + 1..tags.len() {
            assert_ne!(tags[i], tags[j]);
        }
    }
    let s = d.seal(Purpose::OidcState, b"aad", b"state").unwrap();
    assert_eq!(
        d.open(Purpose::OidcState, b"aad", &s).unwrap().expose(),
        b"state"
    );
    assert!(d.open(Purpose::OauthState, b"aad", &s).is_err());
    assert!(d.open(Purpose::OidcState, b"other", &s).is_err());
    assert!(dk().open(Purpose::OidcState, b"aad", &s).is_ok());
    let other = DerivedKeys::new(Secret::new([8u8; 32])).unwrap();
    assert!(other.open(Purpose::OidcState, b"aad", &s).is_err());
}

#[test]
fn hmac_verify() {
    let d = dk();
    let t = d.sign(Purpose::ActionToken, b"data");
    assert!(d.verify(Purpose::ActionToken, b"data", &t));
    assert!(!d.verify(Purpose::ActionToken, b"datb", &t));
    assert!(!d.verify(Purpose::AvatarKey, b"data", &t));
    assert!(!d.verify(Purpose::ActionToken, b"data", &t[..31]));
}

#[test]
fn ed25519_round_trip() {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use ed25519_dalek::Signer;
    let sk = generate_signing_key().unwrap();
    let sig = sk.sign(b"msg");
    let pk = parse_public_key_b64(&STANDARD.encode(sk.verifying_key().to_bytes())).unwrap();
    let sig2 = parse_signature_b64(&STANDARD.encode(sig.to_bytes())).unwrap();
    verify(&pk, b"msg", &sig2).unwrap();
    assert_eq!(
        verify(&pk, b"msh", &sig2).unwrap_err(),
        CryptoError::BadSignature
    );
    assert!(parse_public_key_b64("AAAA").is_err());
    assert!(parse_signature_b64("%%%").is_err());
}
