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

// ---- Signing-key / internal-secret rings (RFE-007, SEC-46) ----

mod rings {
    use aurora_core::secret::Secret;
    use aurora_crypto::{CryptoError, DerivedKeys, Purpose, RingKind, SecretRing};
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;

    fn b(byte: u8, n: usize) -> String {
        STANDARD.encode(vec![byte; n])
    }

    fn list(ids: &[(&str, u8)]) -> String {
        ids.iter()
            .map(|(i, v)| format!("{i}:{}", b(*v, 32)))
            .collect::<Vec<_>>()
            .join(",")
    }

    #[test]
    fn bare_value_is_ring_v0() {
        let r = SecretRing::parse(&b(1, 32), None, RingKind::Signing).unwrap();
        assert_eq!(r.ids().collect::<Vec<_>>(), ["v0"]);
        assert_eq!(r.active_id(), "v0");
        // Explicit active id must be v0.
        assert!(SecretRing::parse(&b(1, 32), Some("v0"), RingKind::Signing).is_ok());
        assert!(SecretRing::parse(&b(1, 32), Some("k1"), RingKind::Signing).is_err());
    }

    #[test]
    fn list_needs_active_id_in_ring() {
        let l = list(&[("a1", 1), ("b2", 2)]);
        assert_eq!(
            SecretRing::parse(&l, None, RingKind::Signing).unwrap_err(),
            CryptoError::InvalidKeyring("active key id is required")
        );
        assert!(SecretRing::parse(&l, Some("zz"), RingKind::Signing).is_err());
        let r = SecretRing::parse(&l, Some("b2"), RingKind::Signing).unwrap();
        assert_eq!(r.active_id(), "b2");
        assert_eq!(r.active_value(), &[2u8; 32]);
        assert_eq!(r.ids().collect::<Vec<_>>(), ["a1", "b2"]);
    }

    #[test]
    fn at_most_four_entries_no_duplicates_valid_ids() {
        let four = list(&[("a", 1), ("b", 2), ("c", 3), ("d", 4)]);
        assert!(SecretRing::parse(&four, Some("a"), RingKind::Signing).is_ok());
        let five = format!("{four},e:{}", b(5, 32));
        assert!(SecretRing::parse(&five, Some("a"), RingKind::Signing).is_err());
        let dup = list(&[("a", 1), ("a", 2)]);
        assert!(SecretRing::parse(&dup, Some("a"), RingKind::Signing).is_err());
        let bad = list(&[("A", 1)]);
        assert!(SecretRing::parse(&bad, Some("A"), RingKind::Signing).is_err());
    }

    #[test]
    fn value_lengths_per_kind() {
        assert!(SecretRing::parse(&b(1, 31), None, RingKind::Signing).is_err());
        assert!(SecretRing::parse(&b(1, 33), None, RingKind::Signing).is_err());
        assert!(SecretRing::parse(&b(1, 31), None, RingKind::Internal).is_err());
        assert!(SecretRing::parse(&b(1, 48), None, RingKind::Internal).is_ok());
        assert!(SecretRing::parse("not base64 !!", None, RingKind::Signing).is_err());
    }

    #[test]
    fn debug_never_prints_values() {
        let l = list(&[("a1", 9)]);
        let r = SecretRing::parse(&l, Some("a1"), RingKind::Signing).unwrap();
        let d = format!("{r:?}");
        assert!(d.contains("a1") && !d.contains(&b(9, 32)));
    }

    #[test]
    fn presented_secret_with_and_without_key_id() {
        let l = list(&[("a1", 1), ("b2", 2)]);
        let r = SecretRing::parse(&l, Some("a1"), RingKind::Internal).unwrap();
        assert!(r.verify_presented(Some("b2"), &[2u8; 32]));
        assert!(
            !r.verify_presented(Some("a1"), &[2u8; 32]),
            "wrong key for id"
        );
        assert!(
            !r.verify_presented(Some("zz"), &[2u8; 32]),
            "id outside ring"
        );
        assert!(
            r.verify_presented(None, &[1u8; 32]),
            "header absent: any key"
        );
        assert!(!r.verify_presented(None, &[3u8; 32]));
        let hdr = format!("Bearer {}", b(2, 32));
        assert!(r.verify_bearer_header(Some("b2"), Some(&hdr)));
        assert!(!r.verify_bearer_header(Some("a1"), Some(&hdr)));
        assert!(!r.verify_bearer_header(None, Some("Basic x")));
        assert!(!r.verify_bearer_header(None, None));
        assert!(!SecretRing::default().verify_bearer_header(None, Some("Bearer ")));
    }

    fn dk(ids: &[(&str, u8)], active: &str) -> DerivedKeys {
        let r = SecretRing::parse(&list(ids), Some(active), RingKind::Signing).unwrap();
        DerivedKeys::from_ring(&r).unwrap()
    }

    #[test]
    fn seal_uses_active_and_open_accepts_any() {
        let old = dk(&[("a1", 1)], "a1");
        let new = dk(&[("a1", 1), ("b2", 2)], "b2");
        let s_old = old.seal(Purpose::OidcState, b"x", b"p").unwrap();
        assert!(s_old.starts_with("a1."));
        let s_new = new.seal(Purpose::OidcState, b"x", b"p").unwrap();
        assert!(s_new.starts_with("b2."));
        assert_eq!(
            new.open(Purpose::OidcState, b"x", &s_old).unwrap().expose(),
            b"p"
        );
        // Old receivers without the new key refuse it (id outside the ring).
        assert_eq!(
            old.open(Purpose::OidcState, b"x", &s_new).unwrap_err(),
            CryptoError::UnknownKey
        );
        // Retired key removed: its states stop working.
        let retired = dk(&[("b2", 2)], "b2");
        assert!(retired.open(Purpose::OidcState, b"x", &s_old).is_err());
    }

    #[test]
    fn unprefixed_values_are_tried_against_each_key() {
        // Phase 0 value: bare b64url sealed by the single key (now ring v0).
        let legacy = DerivedKeys::new(Secret::new([1u8; 32])).unwrap();
        let sealed = legacy.seal(Purpose::LinkState, b"x", b"p").unwrap();
        let bare = sealed.split_once('.').unwrap().1.to_owned();
        let ring = dk(&[("a1", 9), ("b2", 1)], "a1");
        assert_eq!(
            ring.open(Purpose::LinkState, b"x", &bare).unwrap().expose(),
            b"p"
        );
        let none = dk(&[("a1", 9), ("b2", 8)], "a1");
        assert!(none.open(Purpose::LinkState, b"x", &bare).is_err());
    }

    #[test]
    fn hmac_by_id_and_by_trial() {
        let old = dk(&[("a1", 1)], "a1");
        let new = dk(&[("a1", 1), ("b2", 2)], "b2");
        let t_old = old.sign(Purpose::ActionToken, b"d");
        let t_new = new.sign(Purpose::ActionToken, b"d");
        assert!(new.verify_with("a1", Purpose::ActionToken, b"d", &t_old));
        assert!(!new.verify_with("b2", Purpose::ActionToken, b"d", &t_old));
        assert!(!old.verify_with("b2", Purpose::ActionToken, b"d", &t_new));
        assert!(new.verify(Purpose::ActionToken, b"d", &t_old));
        assert!(new.verify(Purpose::ActionToken, b"d", &t_new));
        assert!(!old.verify(Purpose::ActionToken, b"d", &t_new));
        assert_eq!(new.active_id(), "b2");
    }
}
