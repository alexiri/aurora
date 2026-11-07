//! Round-trip, open-vocabulary and bound tests for W5-W7 (`docs/16`).

use aurora_protocol::common::{Validate, ValidationError};
use aurora_protocol::runner::*;
use aurora_protocol::supervisor::*;
use aurora_protocol::tools::*;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

trait ParseCode {
    fn parse_code(&self) -> Code;
}
impl ParseCode for str {
    fn parse_code(&self) -> Code {
        Code::new(self).unwrap_or_else(|_| unreachable!())
    }
}

type R = Result<(), Box<dyn std::error::Error>>;

const U1: &str = "018f0000-0000-7000-8000-000000000001";
const U2: &str = "018f0000-0000-7000-8000-000000000002";
const U3: &str = "018f0000-0000-7000-8000-000000000003";
const U4: &str = "018f0000-0000-7000-8000-000000000004";

/// Parses `v` as `T` and checks it re-serializes to the same JSON.
fn rt<T: DeserializeOwned + Serialize>(v: Value) -> Result<T, Box<dyn std::error::Error>> {
    let t: T = serde_json::from_value(v.clone())?;
    assert_eq!(serde_json::to_value(&t)?, v);
    Ok(t)
}

#[test]
fn register_roundtrip() -> R {
    rt::<RegisterRequest>(json!({
        "version": "0.1.0", "backend": "docker", "advertised_host": "runner-1.internal",
        "capabilities": {
            "backend": "docker", "home_durability": "resident",
            "resources": {"cpu": {"min": 1, "max": 8}, "memory_mib": {"min": 512, "max": 16384},
                "pids": {"min": 64, "max": 4096}, "home_gib": {"min": 1, "max": 100}},
            "max_sandboxes": 4, "harnesses": ["claude-code", "fake"], "max_attachment_bytes": 10485760
        }
    }))?;
    rt::<RegisterResponse>(json!({
        "runner_id": U1, "replica_id": U2, "protocol": 1,
        "config": {"poll_wait_secs": 25, "reconcile_interval_secs": 60,
                   "orphan_grace_secs": 3600, "idle_stop_secs": 1800}
    }))?;
    rt::<WorkRequest>(json!({"wait_secs": 25, "capacity": {"running": 1, "max": 4}}))?;
    Ok(())
}

fn env(kind: &str, extra: Value) -> Value {
    let mut v = json!({"item_id": U1, "kind": kind, "sandbox_id": U2, "boot_id": U3});
    if let (Some(o), Value::Object(e)) = (v.as_object_mut(), extra) {
        o.extend(e);
    }
    v
}

fn deliver_body() -> Value {
    json!({
        "conversation_id": U1, "turn_id": U2,
        "session": {"resume_ref": "abc", "epoch": 3},
        "model": {"provider": "anthropic", "endpoint_id": "e1", "base_url": "https://x",
                  "dialect": "anthropic", "model": "m", "effort": "high", "client_env": [["A", "b"]]},
        "context_credential": "auc_secret",
        "input": {"text": "hi", "speaker_label": "Alice", "door": "chat",
                  "recall": {"index": "idx", "snippets": ["s"]}, "notices": ["n"]},
        "attachments": [{"attachment_id": U3, "filename": "a.txt", "size": 5}],
        "gate": {"command": "true", "timeout_secs": 10}
    })
}

#[test]
fn work_items_roundtrip() -> R {
    let items = vec![
        env(
            "sandbox.start",
            json!({"spec": {
            "image": "img", "env": [["K", "V"]], "boot_secret": "c2VjcmV0", "home_ref": "h",
            "home_gib": 10, "resources": {"cpu_millis": 1000, "memory_mib": 2048, "pids": 512},
            "labels": {"a": "b"}}}),
        ),
        env("sandbox.stop", json!({"reason": "idle", "grace_secs": 10})),
        env(
            "home.sync",
            json!({"generation": 4, "part_index": 0, "part_count": 2,
            "part": {"kind": "skills", "entries": [{"path": "a", "content": "b"}]}}),
        ),
        env("turn.deliver", deliver_body()),
        env(
            "turn.message",
            json!({"turn_id": U1, "followup_id": U2, "text": "t", "speaker_label": "A"}),
        ),
        env("turn.send_now", json!({"turn_id": U1})),
        env("turn.abort", json!({"turn_id": U1})),
        env("turn.notice", json!({"turn_id": U1, "text": "ceiling"})),
        env("session.release", json!({"conversation_id": U1})),
    ];
    for it in items {
        let w: WorkItem = rt(it)?;
        assert!(!matches!(w.body, WorkItemBody::Other(_)));
        w.validate()?;
    }
    Ok(())
}

#[test]
fn reports_roundtrip() -> R {
    let base = |kind: &str, extra: Value| {
        let mut v = json!({"report_id": U1, "kind": kind, "sandbox_id": U2, "boot_id": U3});
        if let (Some(o), Value::Object(e)) = (v.as_object_mut(), extra) {
            o.extend(e);
        }
        v
    };
    let reports = vec![
        base(
            "sandbox.status",
            json!({"state": "failed", "container_ref": "c", "reason_code": "oom",
            "message": "m", "observed": {"cpu_millis": 5, "memory_mib": 6}}),
        ),
        base(
            "supervisor.ready",
            json!({"version": "1", "harness": {"id": "fake", "capabilities": ["steer"]}}),
        ),
        base("home.synced", json!({"generation": 2})),
        base("turn.started", json!({"turn_id": U4})),
        base(
            "turn.events",
            json!({"turn_id": U4, "events": [{"n": 1, "event": {"kind": "text_delta", "text": "x"}}]}),
        ),
        base("turn.progress", json!({"turn_id": U4})),
        base(
            "turn.result",
            json!({"turn_id": U4, "status": "failed", "error_code": "e", "error_message": "m",
            "usage": {"input_tokens": 1}}),
        ),
        base(
            "steer.result",
            json!({"followup_id": U4, "outcome": "missed"}),
        ),
        base(
            "send_now.result",
            json!({"turn_id": U4, "outcome": "moved"}),
        ),
        base(
            "session",
            json!({"conversation_id": U4, "session_ref": "r", "epoch": 1}),
        ),
        base(
            "process.state",
            json!({"processes": [{"process_id": "p", "state": "running", "busy": true}]}),
        ),
        base("watch.fired", json!({"watch_id": "w", "detail": "d"})),
        base(
            "gate.result",
            json!({"turn_id": U4, "outcome": "wake", "output": "o"}),
        ),
        base("ports.state", json!({"ports": [8080, 3000]})),
    ];
    for r in &reports {
        let rep: Report = rt(r.clone())?;
        assert!(!matches!(rep.body, ReportBody::Other(_)), "{r}");
        rep.validate()?;
    }
    let req: ReportsRequest = rt(json!({ "reports": reports }))?;
    req.validate()?;
    rt::<ReportsResponse>(json!({"results": [
        {"report_id": U1, "status": "applied"}, {"report_id": U2, "status": "fenced", "detail": "d"}]}))?;
    Ok(())
}

#[test]
fn other_calls_roundtrip() -> R {
    rt::<ChannelClaimRequest>(json!({"bootstrap_token": "aup_x"}))?;
    rt::<ChannelClaimResponse>(json!({"sandbox_id": U1, "boot_id": U2}))?;
    rt::<ChannelRef>(json!({"sandbox_id": U1, "boot_id": U2}))?;
    rt::<OkResponse>(json!({"ok": true}))?;
    rt::<MemoryWriteRequest>(
        json!({"sandbox_id": U1, "boot_id": U2, "key": "k", "title": "t", "content": "c"}),
    )?;
    rt::<MemoryWriteResponse>(json!({"revision": 3}))?;
    rt::<AttachmentUploadResponse>(json!({"attachment_id": U1}))?;
    rt::<SandboxesResponse>(json!({"sandboxes": [
        {"sandbox_id": U1, "agent_id": U2, "boot_id": U3, "desired": "running", "home_ref": "h"}]}))?;
    rt::<SandboxesCheckRequest>(json!({"sandbox_ids": [U1]}))?;
    rt::<SandboxesCheckResponse>(json!({"known": [U1]}))?;
    rt::<StraysRequest>(
        json!({"objects": [{"container_ref": "c", "created_at": "2026-01-01T00:00:00Z"}]}),
    )?;
    Ok(())
}

#[test]
fn supervisor_handshake_and_frames() -> R {
    let h: Hello = rt(json!({"hello": {"protocol": 1, "bootstrap_token": "aup_x"}}))?;
    h.validate()?;
    let h2: Hello = rt(json!({"hello": {"protocol": 1, "session":
        {"sandbox_id": U1, "boot_id": U2, "issued_at": 1700000000, "mac": "m"}}}))?;
    h2.validate()?;
    let both = Hello {
        hello: HelloBody {
            bootstrap_token: h.hello.bootstrap_token,
            ..h2.hello
        },
    };
    assert!(both.validate().is_err());
    rt::<Welcome>(json!({"welcome": {"session":
        {"sandbox_id": U1, "boot_id": U2, "issued_at": 1, "mac": "m"}, "resume_from": 7}}))?;

    let f: Frame<RunnerToSupervisor> =
        rt(json!({"seq": 1, "ack": 0, "kind": "turn.deliver", "body": deliver_body()}))?;
    f.validate()?;
    rt::<Frame<RunnerToSupervisor>>(json!({"seq": 2, "ack": 1, "kind": "ack", "body": {}}))?;
    rt::<Frame<RunnerToSupervisor>>(json!({"seq": 2, "ack": 1, "kind": "tool.result", "body":
        {"call_id": "c", "ok": true, "content": [{"type": "text", "text": "x"}]}}))?;
    rt::<Frame<RunnerToSupervisor>>(
        json!({"seq": 2, "ack": 1, "kind": "memory.write.result", "body":
        {"key": "k", "ok": true, "revision": 2}}),
    )?;
    rt::<Frame<RunnerToSupervisor>>(json!({"seq": 2, "ack": 1, "kind": "file.result", "body":
        {"upload_id": "u", "ok": true, "attachment_id": U1}}))?;
    rt::<Frame<RunnerToSupervisor>>(
        json!({"seq": 3, "ack": 1, "kind": "shutdown", "body": {"grace_secs": 5}}),
    )?;

    let s2r = [
        (
            "ready",
            json!({"version": "1", "harness": {"id": "fake", "capabilities": []}}),
        ),
        (
            "turn.event",
            json!({"turn_id": U1, "n": 1, "event": {"kind": "done"}}),
        ),
        (
            "tool.call",
            json!({"call_id": "c", "conversation_id": U1, "turn_id": U2, "tool": "t", "args": {"a": 1}}),
        ),
        ("memory.write", json!({"key": "k", "content": "c"})),
        (
            "file.part",
            json!({"upload_id": "u", "index": 0, "data_b64": "QUJD"}),
        ),
        (
            "file.commit",
            json!({"upload_id": "u", "conversation_id": U1, "turn_id": U2, "filename": "f",
            "caption": "c", "sha256": "a".repeat(64), "size": 3}),
        ),
        ("turn.result", json!({"turn_id": U1, "status": "done"})),
        ("ports.state", json!({"ports": [80]})),
    ];
    for (i, (kind, body)) in s2r.into_iter().enumerate() {
        let f: Frame<SupervisorToRunner> =
            rt(json!({"seq": i, "ack": 0, "kind": kind, "body": body}))?;
        assert!(!matches!(f.msg, SupervisorToRunner::Other(_)), "{kind}");
        f.validate()?;
    }
    Ok(())
}

#[test]
fn tools_roundtrip() -> R {
    rt::<ToolCall>(
        json!({"sandbox_id": U1, "boot_id": U2, "call_id": "c", "conversation_id": U3,
        "tool": "memory_search", "args": {"q": "x"}}),
    )?;
    let ok: ToolResult = rt(json!({"call_id": "c", "ok": true,
        "content": [{"type": "text", "text": "hi"}, {"type": "json", "value": {"a": [1]}}]}))?;
    ok.validate()?;
    let bad: ToolResult =
        rt(json!({"call_id": "c", "ok": false, "error": {"code": "denied", "message": "no"}}))?;
    bad.validate()?;
    let inconsistent = ToolResult { ok: true, ..bad };
    assert!(inconsistent.validate().is_err());
    Ok(())
}

#[test]
fn unknown_kinds_degrade_to_other() -> R {
    let w: WorkItem = serde_json::from_value(env("future.thing", json!({"x": 1})))?;
    assert!(matches!(w.body, WorkItemBody::Other(ref u) if u.kind.as_str() == "future.thing"));
    // Round trip preserves the unknown body.
    assert_eq!(
        serde_json::to_value(&w)?,
        env("future.thing", json!({"x": 1}))
    );

    let r: Report = serde_json::from_value(
        json!({"report_id": U1, "kind": "z", "sandbox_id": U2, "boot_id": U3}),
    )?;
    assert!(matches!(r.body, ReportBody::Other(_)));

    let f: Frame<SupervisorToRunner> =
        serde_json::from_value(json!({"seq": 1, "ack": 0, "kind": "nope", "body": {"a": 1}}))?;
    assert!(matches!(f.msg, SupervisorToRunner::Other(_)));
    let f: Frame<RunnerToSupervisor> =
        serde_json::from_value(json!({"seq": 1, "ack": 0, "kind": "nope"}))?;
    assert!(matches!(f.msg, RunnerToSupervisor::Other(_)));

    let c: ToolContent = serde_json::from_value(json!({"type": "image", "data": "x"}))?;
    assert!(matches!(c, ToolContent::Other(_)));

    let s: ReportResult = serde_json::from_value(json!({"report_id": U1, "status": "weird"}))?;
    assert!(matches!(s.status, ReportStatus::Other(_)));
    let st: SandboxStop = serde_json::from_value(json!({"reason": "new_reason", "grace_secs": 1}))?;
    assert_eq!(st.reason.as_str(), "new_reason");
    Ok(())
}

#[test]
fn missing_or_malformed_tag_is_an_error() {
    assert!(
        serde_json::from_value::<WorkItem>(json!({"item_id": U1, "sandbox_id": U2, "boot_id": U3}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<Frame<RunnerToSupervisor>>(json!({"seq": 1, "ack": 0, "kind": 5}))
            .is_err()
    );
    // Known kind with a malformed body is an error, not Other.
    assert!(serde_json::from_value::<WorkItem>(env("turn.abort", json!({"turn_id": 3}))).is_err());
    // Over-long unknown kind rejected.
    assert!(serde_json::from_value::<WorkItem>(env(&"k".repeat(65), json!({}))).is_err());
}

#[test]
fn oversize_rejected() -> R {
    let w: WorkRequest =
        serde_json::from_value(json!({"wait_secs": 26, "capacity": {"running": 0, "max": 1}}))?;
    assert!(w.validate().is_err());

    let mut d = deliver_body();
    d["input"]["text"] = Value::String("x".repeat(100_001));
    let t: TurnDeliver = serde_json::from_value(d)?;
    assert!(matches!(
        t.validate(),
        Err(ValidationError::TooLarge {
            field: "input.text",
            ..
        })
    ));

    let mut d = deliver_body();
    d["attachments"] = Value::Array(
        (0..11)
            .map(|_| json!({"attachment_id": U1, "filename": "f", "size": 1}))
            .collect(),
    );
    assert!(
        serde_json::from_value::<TurnDeliver>(d)?
            .validate()
            .is_err()
    );

    let hs = HomeSync {
        generation: 1,
        part_index: 0,
        part_count: 1,
        part: HomeSyncPart {
            kind: HomeSyncKind::Skills,
            entries: vec![json!("x".repeat(200 * 1024 + 1))],
        },
    };
    assert!(hs.validate().is_err());
    let bad_idx = HomeSync {
        part_index: 1,
        part: hs.part.clone(),
        ..hs
    };
    assert!(bad_idx.validate().is_err());

    let report = |i: usize| -> Report {
        Report {
            report_id: uuid::Uuid::from_u128(i as u128),
            sandbox_id: uuid::Uuid::nil(),
            boot_id: uuid::Uuid::nil(),
            body: ReportBody::HomeSynced(HomeSynced { generation: 1 }),
        }
    };
    let too_many = ReportsRequest {
        reports: (0..201).map(report).collect(),
    };
    assert!(too_many.validate().is_err());
    assert!(
        ReportsRequest {
            reports: (0..200).map(report).collect()
        }
        .validate()
        .is_ok()
    );

    let big = Report {
        body: ReportBody::GateResult(GateResult {
            turn_id: uuid::Uuid::nil(),
            outcome: GateOutcome::Wake,
            output: Some("x".repeat(aurora_protocol::runner::MAX_GATE_OUTPUT + 1)),
        }),
        ..report(1)
    };
    assert!(big.validate().is_err());

    let wide = ReportsRequest {
        reports: (0..200)
            .map(|i| Report {
                body: ReportBody::TurnEvents(TurnEvents {
                    turn_id: uuid::Uuid::nil(),
                    events: vec![NumberedEvent {
                        n: 1,
                        event: HarnessEventWire {
                            kind: "text".parse_code(),
                            data: serde_json::Map::from_iter([(
                                "text".to_owned(),
                                json!("d".repeat(6000)),
                            )]),
                        },
                    }],
                }),
                ..report(i)
            })
            .collect(),
    };
    assert!(wide.validate().is_err(), "1 MiB batch bound");

    let mem = MemoryWrite {
        key: "k".into(),
        title: None,
        description: None,
        content: "c".repeat(100_001),
    };
    assert!(mem.validate().is_err());

    let frame = Frame {
        seq: 1,
        ack: 0,
        msg: SupervisorToRunner::MemoryWrite(MemoryWrite {
            key: "k".into(),
            title: None,
            description: None,
            content: "c".repeat(100_000),
        }),
    };
    assert!(frame.validate().is_ok());
    // A full 192 KiB decoded file part does not fit in WS_FRAME_MAX once base64-encoded.
    let part = Frame {
        seq: 1,
        ack: 0,
        msg: SupervisorToRunner::FilePart(FilePart {
            upload_id: "u".into(),
            index: 0,
            data_b64: "A".repeat(192 * 1024 / 3 * 4),
        }),
    };
    assert!(part.validate().is_err());
    let over = FilePart {
        upload_id: "u".into(),
        index: 0,
        data_b64: "A".repeat(192 * 1024 / 3 * 4 + 4),
    };
    assert!(over.validate().is_err());

    let commit = FileCommit {
        upload_id: "u".into(),
        conversation_id: uuid::Uuid::nil(),
        turn_id: uuid::Uuid::nil(),
        filename: "f".into(),
        caption: None,
        sha256: "zz".into(),
        size: 1,
    };
    assert!(commit.validate().is_err());

    let huge = ToolCallBody {
        call_id: "c".into(),
        conversation_id: uuid::Uuid::nil(),
        turn_id: None,
        tool: "t".into(),
        args: json!({"a": "x".repeat(300_000)}),
    };
    assert!(huge.validate().is_err());
    Ok(())
}

#[test]
fn secrets_not_in_debug() -> R {
    let t: TurnDeliver = serde_json::from_value(deliver_body())?;
    assert!(!format!("{t:?}").contains("auc_secret"));
    Ok(())
}

#[test]
fn file_in_roundtrip_and_validation() -> R {
    use aurora_protocol::supervisor::{FileIn, MAX_FILE_PART_DECODED};
    let ok = json!({"turn_id": U1, "attachment_id": U2, "filename": "a b.txt", "index": 0,
        "last": true, "size": 3, "sha256": "a".repeat(64), "data_b64": "QUJD"});
    let f: Frame<RunnerToSupervisor> =
        rt(json!({"seq": 1, "ack": 0, "kind": "file.in", "body": ok}))?;
    assert!(matches!(f.msg, RunnerToSupervisor::FileIn(_)));
    f.validate()?;

    let base = FileIn {
        turn_id: uuid::Uuid::nil(),
        attachment_id: uuid::Uuid::nil(),
        filename: "f".into(),
        index: 0,
        last: true,
        size: 3,
        sha256: "a".repeat(64),
        data_b64: "QUJD".into(),
    };
    assert!(base.validate().is_ok());
    // Empty file: one empty last chunk.
    let empty = FileIn {
        size: 0,
        data_b64: String::new(),
        ..base.clone()
    };
    assert!(empty.validate().is_ok());
    // A full-size middle chunk fits WS_FRAME_MAX.
    let full = Frame {
        seq: 1,
        ack: 0,
        msg: RunnerToSupervisor::FileIn(FileIn {
            index: 0,
            last: false,
            size: 10 * 1024 * 1024,
            data_b64: "A".repeat(MAX_FILE_PART_DECODED / 3 * 4),
            ..base.clone()
        }),
    };
    assert!(full.validate().is_ok());
    let bad = [
        FileIn {
            data_b64: "A".repeat((MAX_FILE_PART_DECODED / 3 + 1) * 4),
            size: 10 * 1024 * 1024,
            last: false,
            ..base.clone()
        },
        FileIn {
            filename: String::new(),
            ..base.clone()
        },
        FileIn {
            filename: "x".repeat(256),
            ..base.clone()
        },
        FileIn {
            sha256: "zz".into(),
            ..base.clone()
        },
        FileIn {
            size: 11 * 1024 * 1024,
            ..base.clone()
        },
        // Single chunk shorter than the declared size.
        FileIn {
            size: 9,
            ..base.clone()
        },
        // Not last, yet already complete.
        FileIn {
            last: false,
            ..base.clone()
        },
        // Empty chunk of a non-empty file.
        FileIn {
            data_b64: String::new(),
            ..base.clone()
        },
        // Index beyond the size.
        FileIn {
            index: 3,
            ..base.clone()
        },
        // Empty file declared not last.
        FileIn {
            size: 0,
            last: false,
            data_b64: String::new(),
            ..base.clone()
        },
        FileIn {
            data_b64: "QUJ".into(),
            ..base.clone()
        },
    ];
    for (i, b) in bad.iter().enumerate() {
        assert!(b.validate().is_err(), "case {i}");
    }
    Ok(())
}
