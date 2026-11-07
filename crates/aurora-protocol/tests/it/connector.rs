//! W8 connector contract: round trips, validation, open vocabularies.

use aurora_protocol::common::Validate;
use aurora_protocol::connector::*;
use serde_json::json;
use uuid::Uuid;

type R = Result<(), Box<dyn std::error::Error>>;

fn space() -> Space {
    Space {
        reference: "C1".into(),
        kind: SpaceKind::Channel,
        label: "general".into(),
    }
}

#[test]
fn message_posted_event_roundtrip() -> R {
    let body = MessagePosted {
        space: space(),
        thread_ref: Some("t1".into()),
        message_ref: "m1".into(),
        author: Author {
            reference: "U1".into(),
            kind: AuthorKind::Person,
            display_name: "Ann".into(),
            email: None,
            email_verified: Some(true),
        },
        text: "hi <@bot:B1>".into(),
        mentions: vec![Mention {
            kind: MentionKind::Bot,
            reference: "B1".into(),
        }],
        bot_mentioned: true,
        addressed_bot_ref: "B1".into(),
        files: vec![],
    };
    let ev = InboundEvent::new(
        "e1",
        Uuid::nil(),
        EventKind::MessagePosted,
        "2026-01-01T00:00:00Z",
        &body,
    )?;
    let wire = serde_json::to_string(&ev)?;
    let back: InboundEvent = serde_json::from_str(&wire)?;
    assert_eq!(back, ev);
    assert_eq!(back.parse()?, EventPayload::MessagePosted(body));
    assert_eq!(
        serde_json::to_value(&ev)?["payload"]["space"]["ref"],
        json!("C1")
    );
    Ok(())
}

#[test]
fn unknown_event_and_op_kinds_degrade() -> R {
    let ev: InboundEvent = serde_json::from_value(json!({
        "event_id": "e", "integration_id": Uuid::nil(), "kind": "pin.added",
        "occurred_at": "x", "payload": {"anything": 1}
    }))?;
    assert!(matches!(ev.kind, EventKind::Other(_)));
    assert_eq!(ev.parse()?, EventPayload::Other);
    let op: Operation = serde_json::from_value(json!({
        "op_id": Uuid::nil(), "kind": "poll.create", "payload": {}
    }))?;
    assert_eq!(op.attempt, 1);
    assert!(!op.maybe_sent);
    assert_eq!(op.parse()?, OpPayload::Other);
    Ok(())
}

#[test]
fn sender_and_target_shapes() -> R {
    let p: MessagePostOp = serde_json::from_value(json!({
        "target": {"space_ref": "C1", "thread_ref": "t"}, "sender": "shared",
        "content": {"text": "x"}, "op_marker": "m"
    }))?;
    assert_eq!(p.sender, Sender::shared());
    assert!(matches!(p.target, Target::Space { .. }));
    let q: MessagePostOp = serde_json::from_value(json!({
        "target": {"user_ref": "U1"}, "sender": {"presence_id": Uuid::nil()},
        "content": {}, "op_marker": "m"
    }))?;
    assert!(matches!(q.target, Target::User { .. }));
    assert!(matches!(q.sender, Sender::Presence { .. }));
    assert_eq!(serde_json::to_value(Sender::shared())?, json!("shared"));
    Ok(())
}

#[test]
fn validation_bounds() {
    let ev = |n: usize| EventsRequest {
        events: (0..n)
            .map(|i| InboundEvent {
                event_id: format!("e{i}"),
                integration_id: Uuid::nil(),
                kind: EventKind::MessagePosted,
                occurred_at: "t".into(),
                payload: json!({}),
            })
            .collect(),
    };
    assert!(ev(100).validate().is_ok());
    assert!(ev(101).validate().is_err());
    assert!(
        ClaimRequest {
            max: 0,
            wait_secs: 0
        }
        .validate()
        .is_err()
    );
    assert!(
        ClaimRequest {
            max: 101,
            wait_secs: 0
        }
        .validate()
        .is_err()
    );
    assert!(
        ClaimRequest {
            max: 5,
            wait_secs: 26
        }
        .validate()
        .is_err()
    );
    assert!(
        ClaimRequest {
            max: 5,
            wait_secs: 25
        }
        .validate()
        .is_ok()
    );
    let bad = OpResult {
        op_id: Uuid::nil(),
        outcome: Outcome::Error,
        data: None,
        error: None,
    };
    assert!(bad.validate().is_err());
    assert!(
        OpResult::err(Uuid::nil(), OpError::new(ErrorCode::NotFound, "gone"))
            .validate()
            .is_ok()
    );
    let both = CredentialsRequest {
        target: CredentialTarget {
            integration_id: Some(Uuid::nil()),
            presence_id: Some(Uuid::nil()),
        },
        expected_version: 1,
        credentials: json!({}),
    };
    assert!(both.validate().is_err());
}

#[test]
fn manifest_capabilities_open_vocabulary() -> R {
    let m: ConnectorManifest = serde_json::from_value(json!({
        "package": {"id": "x", "version": "1", "contract": 1, "publisher": "p"},
        "about": {"name": "X"},
        "capabilities": {"threads": true, "edits": false, "files": {"max_bytes": 1}, "future_cap": true},
        "public_callbacks": ["actions"]
    }))?;
    m.validate()?;
    assert_eq!(
        m.enabled_capabilities(),
        vec!["files", "future_cap", "threads"]
    );
    Ok(())
}

#[test]
fn credentials_not_in_debug() -> R {
    let i: Integration = serde_json::from_value(json!({
        "id": Uuid::nil(), "org_id": Uuid::nil(), "tenant_ref": "t", "tenant_label": "T",
        "credentials": {"token": "hunter2"}, "status": "active"
    }))?;
    assert!(!format!("{i:?}").contains("hunter2"));
    Ok(())
}
