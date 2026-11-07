use aurora_channels::*;
use aurora_core::secret::Secret;
use aurora_crypto::{DerivedKeys, Purpose};
use uuid::Uuid;

type R = Result<(), Box<dyn std::error::Error>>;

fn keys(b: u8) -> Result<DerivedKeys, Box<dyn std::error::Error>> {
    Ok(DerivedKeys::new(Secret::new([b; 32]))?)
}

fn claims(integ: Uuid, exp: i64) -> Claims {
    Claims::new(
        integ,
        Uuid::now_v7(),
        exp,
        ActionKind::ApprovalDecide {
            decision: Decision::Approve,
        },
    )
}

#[test]
fn untrusted_text_is_data() {
    let c = Content::untrusted("hi \u{202E}evil\u{0007} <@here> [x](http://a) **b**");
    let t = c.text.unwrap_or_default();
    assert!(!t.contains('\u{202E}') && !t.contains('\u{7}'));
    // every markup-active char is backslash-escaped
    assert!(t.contains("\\<\\@here") || t.contains("\\<@here\\>"), "{t}");
    assert!(!t.contains("](") && !t.contains("<@here>"));
}

#[test]
fn platform_text_cannot_here() {
    let t = Content::platform("x <@here>").text.unwrap_or_default();
    assert!(!t.contains("<@here>"));
    let h = Content::platform("go").with_here().text.unwrap_or_default();
    assert!(h.starts_with("<@here>"));
}

#[test]
fn mention_refs_cannot_break_out() {
    assert_eq!(user_mention_ok("a>b <@here>"), "<@user:ab@here>");
}

fn user_mention_ok(s: &str) -> String {
    aurora_channels::content::user_mention(s)
}

#[test]
fn card_limits_and_round_trip() -> R {
    assert!(Block::fields((0..13).map(|_| ("a", "b"))).is_err());
    assert!(Block::fields((0..12).map(|_| ("a", "b"))).is_ok());
    let btn = |i: usize| ButtonItem::new(&format!("b{i}"), ButtonStyle::Primary, "t".into());
    assert!(Block::buttons((0..6).map(btn).collect()).is_err());
    assert!(
        Block::select(
            "p",
            (0..26).map(|i| (i.to_string(), i.to_string())),
            "t".into()
        )
        .is_err()
    );
    assert!(Block::link("l", "javascript:alert(1)").is_err());
    let card = Card::new(vec![
        Block::header("Approve?\u{202E}"),
        Block::data("*raw*"),
        Block::link("Open", "https://example.test/x")?,
        Block::buttons((0..5).map(btn).collect())?,
        Block::input("why", "tok".into()),
    ])?;
    let c = Content::card(card);
    let back: Content = serde_json::from_str(&serde_json::to_string(&c)?)?;
    assert_eq!(back, c);
    assert!(serde_json::to_string(&c)?.contains(r#""type":"header""#));
    assert!(!serde_json::to_string(&c)?.contains('\u{202E}'));
    let bad = Card {
        blocks: vec![Block::Input {
            label: "x".into(),
            token: String::new(),
        }],
    };
    assert!(bad.validate().is_err());
    Ok(())
}

#[test]
fn token_round_trip_and_stable_id() -> R {
    let k = keys(1)?;
    let integ = Uuid::now_v7();
    let c = claims(integ, 100);
    let tok = ActionToken::sign(&k, &c)?;
    assert!(tok.starts_with("v2.v0."));
    let v = ActionToken::verify(&k, &tok, integ, 50)?;
    assert_eq!(v, c);
    assert_eq!(ActionToken::verify(&k, &tok, integ, 50)?.jti, c.jti);
    Ok(())
}

#[test]
fn token_rejections() -> R {
    let k = keys(1)?;
    let integ = Uuid::now_v7();
    let tok = ActionToken::sign(&k, &claims(integ, 100))?;
    assert_eq!(
        ActionToken::verify(&k, &tok, integ, 100),
        Err(TokenError::Expired)
    );
    assert_eq!(
        ActionToken::verify(&k, &tok, Uuid::now_v7(), 1),
        Err(TokenError::WrongIntegration)
    );
    assert_eq!(
        ActionToken::verify(&keys(2)?, &tok, integ, 1),
        Err(TokenError::BadSignature)
    );
    // tampered payload (swap in another valid payload, keep old tag)
    let other = ActionToken::sign(&k, &claims(integ, 9999))?;
    let (p2, t1) = (
        other.split('.').nth(2).unwrap_or(""),
        tok.split('.').nth(3).unwrap_or(""),
    );
    assert_eq!(
        ActionToken::verify(&k, &format!("v2.v0.{p2}.{t1}"), integ, 1),
        Err(TokenError::BadSignature)
    );
    // wrong purpose key
    let payload = tok.split('.').nth(2).unwrap_or("");
    use base64::Engine;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload)?;
    let tag = k.sign(Purpose::LinkState, &raw);
    let forged = format!(
        "v1.{payload}.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(tag)
    );
    assert_eq!(
        ActionToken::verify(&k, &forged, integ, 1),
        Err(TokenError::BadSignature)
    );
    for bad in ["", "v1", "v2.a.b", "v1.a.b.c", "v1.!!.!!", "v2.v0.!!.!!"] {
        assert_eq!(
            ActionToken::verify(&k, bad, integ, 1),
            Err(TokenError::Malformed)
        );
    }
    Ok(())
}

#[test]
fn token_size_cap() -> R {
    let k = keys(1)?;
    let c = Claims::new(
        Uuid::now_v7(),
        Uuid::now_v7(),
        10,
        ActionKind::DmDefaultChoose {
            allowed_agent_ids: (0..26).map(|_| Uuid::now_v7()).collect(),
        },
    );
    assert_eq!(ActionToken::sign(&k, &c), Err(TokenError::TooLarge));
    let ok = Claims {
        action: ActionKind::DmDefaultChoose {
            allowed_agent_ids: (0..25).map(|_| Uuid::now_v7()).collect(),
        },
        ..c
    };
    assert!(ActionToken::sign(&k, &ok).is_ok());
    Ok(())
}

fn approval_card() -> Result<Card, Box<dyn std::error::Error>> {
    Ok(Card::new(vec![
        Block::header("Approve?"),
        Block::buttons(vec![
            ButtonItem::new("Approve", ButtonStyle::Primary, "a".into()),
            ButtonItem::new("Deny", ButtonStyle::Danger, "d".into()),
        ])?,
    ])?)
}

#[test]
fn fallback_table() -> R {
    use Capability::*;
    let card = approval_card()?;
    let ctx = FallbackContext {
        dashboard_url: Some("https://d.test/a/1"),
        approval_code: Some("ABCDEFGH"),
    };
    // full support: untouched
    let full = CapabilitySet::new([Buttons, Menus, TextInput]);
    let r = render_card(&card, &full, &ctx);
    assert_eq!(r.card.as_ref(), Some(&card));
    assert!(r.text.is_none());
    // no buttons + commands: dashboard + approve/deny
    let r = render_card(&card, &CapabilitySet::new([Commands]), &ctx);
    assert_eq!(r.card.map(|c| c.blocks.len()), Some(1));
    let t = r.text.unwrap_or_default();
    assert!(
        t.contains("https://d.test/a/1")
            && t.contains("approve ABCDEFGH")
            && t.contains("deny ABCDEFGH")
    );
    // no buttons, no commands: dashboard only
    let t = render_card(&card, &CapabilitySet::default(), &ctx)
        .text
        .unwrap_or_default();
    assert!(t.contains("dashboard") && !t.contains("approve"));
    // no approval code: numbered options
    let t = render_card(
        &card,
        &CapabilitySet::new([Commands]),
        &FallbackContext::default(),
    )
    .text
    .unwrap_or_default();
    assert!(t.contains("1. Approve") && t.contains("2. Deny"));
    Ok(())
}

#[test]
fn capability_decisions() {
    use Capability::*;
    let none = CapabilitySet::default();
    assert_eq!(none.thread_ref("t1"), "");
    assert_eq!(CapabilitySet::new([Threads]).thread_ref("t1"), "t1");
    assert_eq!(
        none.settle_mode(),
        capability::SettleMode::PostSettledMessage
    );
    assert_eq!(
        none.seen_receipt(),
        capability::SeenReceipt::PlaceholderPost
    );
    assert_eq!(
        none.private_notice(),
        capability::PrivateNotice::DirectMessage
    );
    assert_eq!(none.link_method(), capability::LinkMethod::SignInLink);
    assert_eq!(
        CapabilitySet::new([PresenceShared]).presence_modes(),
        (false, true)
    );
    assert_eq!(CapabilitySet::new([Files]).file_limit(10, 5), 5);
    assert!(
        !none.can_read_history() && !none.settings_via_commands() && !none.user_token_features()
    );
}

#[test]
fn unknown_capability_is_carried() -> R {
    let s: CapabilitySet = serde_json::from_str(r#"["threads","future_thing"]"#)?;
    assert!(s.has(&Capability::Threads));
    assert_eq!(serde_json::to_string(&s)?, r#"["threads","future_thing"]"#);
    Ok(())
}

// ---- RFE-007: token key ring (SEC-46) ----

fn ring_keys(ids: &[(&str, u8)], active: &str) -> Result<DerivedKeys, Box<dyn std::error::Error>> {
    use aurora_crypto::{RingKind, SecretRing};
    use base64::Engine;
    let list = ids
        .iter()
        .map(|(i, b)| {
            format!(
                "{i}:{}",
                base64::engine::general_purpose::STANDARD.encode([*b; 32])
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    Ok(DerivedKeys::from_ring(&SecretRing::parse(
        &list,
        Some(active),
        RingKind::Signing,
    )?)?)
}

#[test]
fn v2_token_names_active_key_and_verifies_by_id() -> R {
    let integ = Uuid::now_v7();
    let k = ring_keys(&[("a1", 1), ("b2", 2)], "b2")?;
    let tok = ActionToken::sign(&k, &claims(integ, 100))?;
    assert!(tok.starts_with("v2.b2."), "{tok}");
    assert!(ActionToken::verify(&k, &tok, integ, 1).is_ok());
    // A receiver whose ring lacks the id refuses (no trial fallback for v2).
    let without = ring_keys(&[("a1", 1)], "a1")?;
    assert_eq!(
        ActionToken::verify(&without, &tok, integ, 1),
        Err(TokenError::BadSignature)
    );
    // Rewriting the id to another ring entry does not verify.
    let swapped = tok.replacen("v2.b2.", "v2.a1.", 1);
    assert_eq!(
        ActionToken::verify(&k, &swapped, integ, 1),
        Err(TokenError::BadSignature)
    );
    Ok(())
}

#[test]
fn rotation_keeps_old_tokens_and_removal_revokes_them() -> R {
    let integ = Uuid::now_v7();
    // phase 0 token (v1, single static key) minted before rotation.
    let legacy_keys = keys(1)?;
    let (p, t) = {
        let v2 = ActionToken::sign(&legacy_keys, &claims(integ, 100))?;
        let mut it = v2.split('.').skip(2);
        (
            it.next().unwrap_or("").to_owned(),
            it.next().unwrap_or("").to_owned(),
        )
    };
    let v1 = format!("v1.{p}.{t}");
    // step 1: new key appended, old still active.
    let appended = ring_keys(&[("v0", 1), ("n2", 2)], "v0")?;
    // step 2: new key active.
    let activated = ring_keys(&[("v0", 1), ("n2", 2)], "n2")?;
    // step 3: old key removed.
    let removed = ring_keys(&[("n2", 2)], "n2")?;
    let old_v2 = ActionToken::sign(&appended, &claims(integ, 100))?;
    let new_v2 = ActionToken::sign(&activated, &claims(integ, 100))?;
    assert!(old_v2.starts_with("v2.v0."));
    assert!(new_v2.starts_with("v2.n2."));
    for tok in [&v1, &old_v2, &new_v2] {
        assert!(
            ActionToken::verify(&activated, tok, integ, 1).is_ok(),
            "{tok}"
        );
    }
    // An api still on step 1 accepts the new key only if it holds it: it does (appended).
    assert!(ActionToken::verify(&appended, &new_v2, integ, 1).is_ok());
    // Removed key: old tokens fail, new ones keep working.
    assert!(ActionToken::verify(&removed, &v1, integ, 1).is_err());
    assert!(ActionToken::verify(&removed, &old_v2, integ, 1).is_err());
    assert!(ActionToken::verify(&removed, &new_v2, integ, 1).is_ok());
    Ok(())
}
