//! P4-LLM-4 (C-805, C-806, C-807, SEC-64, SEC-65, SEC-97): the gateway signs Bedrock inference. A child module of
//! `governance` (declared there with `#[path]`) so it shares that harness: real proxy, real policy pipeline, verifying
//! AWS mocks (`aurora_testkit::llm::{bedrock, sts}`), and a scratch database holding the organization's external id.
use super::*;
use aurora_testkit::llm::bedrock::BedrockSeen;

const INVOKE: &str = "/model/us.anthropic.claude-3-haiku-20240307-v1%3A0/invoke";
const STREAM: &str =
    "/model/us.anthropic.claude-3-haiku-20240307-v1%3A0/invoke-with-response-stream";
/// What an agent might send: its own SigV4 and API-key material, all of which must be removed.
const AGENT_AWS: &str = "Authorization: AWS4-HMAC-SHA256 Credential=AKIAAGENT0000000/20260101/us-east-1/bedrock/aws4_request, SignedHeaders=host, Signature=deadbeef\r\n\
x-amz-security-token: agent-session-token\r\nx-amz-date: 20260101T000000Z\r\nx-amz-content-sha256: agent-hash\r\nx-amz-agent-extra: agent-extra\r\nx-api-key: agent-key\r\n";
const BODY: &[u8] = br#"{"anthropic_version":"bedrock-2023-05-31","messages":[]}"#;

async fn env_with(mode: BedrockMode, ext: &'static str) -> Env {
    start(Opts {
        bedrock: Some(mode),
        org_external_id: ext,
        doc: json!({"llm": {"posture": "open", "endpoints": {"bedrock": {"allowed": true}}}}),
        secrets: vec![("unused", true, None)],
        ..Opts::default()
    })
    .await
}

fn rig(env: &Env) -> &BedrockRig {
    env.bedrock.as_ref().unwrap()
}

fn no_agent_material(seen: &BedrockSeen) {
    for (k, v) in &seen.req.headers {
        assert!(
            !v.contains("AKIAAGENT")
                && !v.contains("agent-session-token")
                && !v.contains("agent-hash")
                && !v.contains("agent-extra")
                && !v.contains("agent-key")
                && !v.contains("deadbeef")
                && k != "x-amz-agent-extra"
                && k != "x-api-key",
            "{k}: {v}"
        );
    }
}

/// (acceptance 2) Each credential mode reaches the runtime mock with a signature the mock verifies from the right
/// secret; with assume-role the **session** key signs and the source key id never appears; the agent's own
/// `Authorization` / `X-Amz-*` / `x-api-key` never arrive; no secret is in the request log.
#[tokio::test]
async fn llm_bedrock_each_mode_reaches_the_mock_with_a_valid_signature() {
    for mode in [
        BedrockMode::AssumeRole,
        BedrockMode::Sigv4,
        BedrockMode::ApiKey,
    ] {
        let env = env_with(mode, "ext-org-1").await;
        let r = request(&env, env.llm, "POST", INVOKE, AGENT_AWS, BODY).await;
        assert_eq!(r.status, 200, "{mode:?}: {}", r.body);
        let seen = rig(&env).mock.requests();
        assert_eq!(seen.len(), 1, "{mode:?}");
        let one = &seen[0];
        no_agent_material(one);
        assert_eq!(one.req.body.as_bytes(), BODY, "the body is unchanged");
        match mode {
            BedrockMode::ApiKey => {
                assert!(one.bearer_ok, "{one:?}");
                assert!(
                    one.req
                        .headers
                        .iter()
                        .all(|(k, _)| !k.starts_with("x-amz-")),
                    "an API key is not signed: {one:?}"
                );
            }
            BedrockMode::Sigv4 => {
                let v = one.verified.as_ref().unwrap();
                assert_eq!(v.access_key, BEDROCK_STATIC_KEY);
                assert_eq!(
                    (v.service.as_str(), v.region.as_str()),
                    ("bedrock", "us-east-1")
                );
                assert!(v.session_token.is_none());
                assert_eq!(rig(&env).sts.calls(), 0, "a static key calls no STS");
            }
            BedrockMode::AssumeRole => {
                let v = one.verified.as_ref().unwrap();
                assert!(v.access_key.starts_with("ASIAMOCKSESSION"), "{v:?}");
                assert_eq!(v.session_token.as_deref(), Some("session-token-1"));
                assert_eq!(
                    (v.service.as_str(), v.region.as_str()),
                    ("bedrock", "us-east-1")
                );
                let calls = rig(&env).sts.assume_calls();
                assert_eq!(calls.len(), 1);
                assert_eq!(
                    calls[0].signed_by, BEDROCK_SOURCE_KEY,
                    "the source key signs STS only"
                );
                assert_eq!(calls[0].role_arn, BEDROCK_ROLE);
                assert_eq!(calls[0].role_session_name, format!("aurora-{}", env.org));
                // The source key is nowhere in what the runtime saw.
                let dump = format!("{one:?}");
                assert!(
                    !dump.contains(BEDROCK_SOURCE_KEY) && !dump.contains(BEDROCK_SOURCE_SECRET),
                    "{dump}"
                );
                // The session is reused: a second request does not call STS again.
                let r = request(&env, env.llm, "POST", INVOKE, "", BODY).await;
                assert_eq!(r.status, 200);
                assert_eq!(rig(&env).sts.assume_calls().len(), 1, "single mint");
            }
        }
        env.log.flush().await;
        let rows = env.captured.0.lock().unwrap().clone();
        let dump = format!("{rows:?}{}", r.body);
        for secret in [
            BEDROCK_SOURCE_SECRET,
            BEDROCK_STATIC_SECRET,
            BEDROCK_API_KEY,
            "session-secret-1",
            "session-token-1",
        ] {
            assert!(!dump.contains(secret), "{mode:?}: {secret} in {dump}");
        }
    }
}

/// (acceptance 2, D-261(b), SEC-97) `ExternalId` is the **requesting organization's own** external id, read by the
/// gateway from the database: two organizations send different ones; an organization with none never has its role
/// assumed (502, no STS call, nothing forwarded).
#[tokio::test]
async fn llm_bedrock_external_id_is_the_organizations_own() {
    let a = env_with(BedrockMode::AssumeRole, "ext-A").await;
    let b = env_with(BedrockMode::AssumeRole, "ext-B").await;
    for e in [&a, &b] {
        let r = request(e, e.llm, "POST", INVOKE, "", BODY).await;
        assert_eq!(r.status, 200, "{}", r.body);
    }
    assert_eq!(rig(&a).sts.assume_calls()[0].external_id, "ext-A");
    assert_eq!(rig(&b).sts.assume_calls()[0].external_id, "ext-B");

    let none = env_with(BedrockMode::AssumeRole, "").await;
    let r = request(&none, none.llm, "POST", INVOKE, "", BODY).await;
    assert_eq!(r.status, 502, "{}", r.body);
    assert!(
        has(&r, "credential_unavailable") && r.body.contains("llm_mint"),
        "{}",
        r.body
    );
    assert_eq!(rig(&none).sts.calls(), 0, "fails closed with no call");
    assert!(rig(&none).mock.requests().is_empty());
}

/// (C-807) STS refuses the role (trust policy): 502 `llm_mint` `mint_rejected`, request-log `llm_auth`, nothing is
/// forwarded to the runtime, and a burst is held to one STS call.
#[tokio::test]
async fn llm_bedrock_assume_role_denied_forwards_nothing() {
    let env = env_with(BedrockMode::AssumeRole, "ext-org-1").await;
    rig(&env).sts.deny_assume(true);
    for _ in 0..4 {
        let r = request(&env, env.llm, "POST", INVOKE, "", BODY).await;
        assert_eq!(r.status, 502, "{}", r.body);
        assert!(
            r.body.contains("llm_mint") && r.body.contains("mint_rejected"),
            "{}",
            r.body
        );
    }
    assert_eq!(
        rig(&env).sts.assume_calls().len(),
        1,
        "a failed mint is held"
    );
    assert!(rig(&env).mock.requests().is_empty());
    env.log.flush().await;
    let rows = env.captured.0.lock().unwrap().clone();
    assert!(
        rows.iter()
            .any(|row| row.error_code.as_deref() == Some("llm_auth") && row.status == 502),
        "{rows:?}"
    );
}

/// (C-805, 06 step 6a, acceptance 2) A body over `LLM_SIGN_BODY_MAX` is 413 `llm_request_too_large` **before** any
/// secret is opened (the store is made unreachable: opening it first would answer a store error); exactly the limit
/// is signed and forwarded.
#[tokio::test]
async fn llm_bedrock_oversized_body_is_413_before_the_secret_is_opened() {
    // In CI this test intermittently got no answer at all to a request below (60 s, no bytes, no
    // close): the request helper did not flush, so the end of the body could stay in the client's
    // TLS session (FIX-177; see the next test). The gateway runs on this test's thread: its debug
    // log goes to the test's captured output, which a failure prints, together with how the read
    // ended, whether the body's end had been queued, and the request-log rows.
    let _log = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_test_writer()
            .with_ansi(false)
            .with_env_filter("aurora_gateway=debug,aurora_secrets=debug,info")
            .finish(),
    );
    let limit = aurora_core::limits::LLM_SIGN_BODY_MAX;
    let env = env_with(BedrockMode::Sigv4, "ext-org-1").await;
    let diag = |r: &Reply, at: std::time::Instant| {
        let rows = env.captured.0.lock().unwrap().clone();
        format!(
            "{} after {:?}, read ended: {}, body end queued in TLS: {}; request log: {rows:?}",
            r.body,
            at.elapsed(),
            r.ended,
            r.queued
        )
    };
    env.mem.set_unavailable(true);
    let big = vec![b' '; limit + 1];
    let at = std::time::Instant::now();
    let r = request(&env, env.llm, "POST", INVOKE, "", &big).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), env.log.flush()).await;
    assert_eq!(r.status, 413, "{}", diag(&r, at));
    assert!(has(&r, "llm_request_too_large"), "{}", r.body);
    assert!(rig(&env).mock.requests().is_empty());
    // Control: a small body does open the secret (and finds the store down).
    let r = request(&env, env.llm, "POST", INVOKE, "", BODY).await;
    assert_ne!(r.status, 413);
    assert!(!has(&r, "llm_request_too_large"));
    env.mem.set_unavailable(false);
    let exact = vec![b' '; limit];
    let at = std::time::Instant::now();
    let r = request(&env, env.llm, "POST", INVOKE, "", &exact).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), env.log.flush()).await;
    assert_eq!(r.status, 200, "{}", diag(&r, at));
    let seen = rig(&env).mock.requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].req.body.len(), limit);
    assert!(seen[0].verified.is_ok(), "{:?}", seen[0].verified);
}

/// (SEC-65, C-800, acceptance 2) Only the runtime inference routes are served: guardrail, async-invoke, agent,
/// control-plane list routes, unknown operations and an OpenAI-surface route without the declared dialect are refused
/// `llm_path_not_permitted` before policy and before the secret is opened; no STS call is made.
#[tokio::test]
async fn llm_bedrock_is_confined_to_the_runtime_inference_routes() {
    let env = env_with(BedrockMode::AssumeRole, "ext-org-1").await;
    env.mem.set_unavailable(true);
    for (method, path) in [
        ("POST", "/guardrail/g-1/version/1/apply"),
        ("POST", "/async-invoke"),
        ("GET", "/async-invoke"),
        ("POST", "/agents/a-1/agentAliases/x/sessions/s/text"),
        ("GET", "/foundation-models"),
        ("GET", "/inference-profiles"),
        ("POST", "/model/x/unknown-op"),
        ("GET", "/model/x/invoke"),
        ("POST", "/openai/v1/chat/completions"),
    ] {
        let r = request(&env, env.llm, method, path, "", b"{}").await;
        assert_eq!(r.status, 403, "{method} {path}: {}", r.body);
        assert!(
            has(&r, "llm_path_not_permitted"),
            "{method} {path}: {}",
            r.body
        );
    }
    assert!(rig(&env).mock.requests().is_empty());
    assert_eq!(rig(&env).sts.calls(), 0);
    // A dot segment is refused even earlier, as ambiguous.
    let r = request(&env, env.llm, "POST", "/model/%2e%2e/invoke", "", b"{}").await;
    assert_eq!(r.status, 400, "{}", r.body);
    assert!(has(&r, "ambiguous_path"), "{}", r.body);
}

/// (D-260(d), acceptance 2) The AWS event stream passes byte for byte and is streamed: the first frame reaches the
/// client while the upstream still holds the rest back.
#[tokio::test]
async fn llm_bedrock_event_stream_is_unaltered_and_streamed() {
    let env = env_with(BedrockMode::Sigv4, "ext-org-1").await;
    let gate = rig(&env).mock.gate_stream();
    let frames = aurora_testkit::llm::bedrock::stream_frames();
    let mut s = crate::common::connect(env.proxy).await;
    let head = format!(
        "POST http://{t}{STREAM} HTTP/1.1\r\nHost: {t}\r\n{}Content-Length: {}\r\nConnection: close\r\n\r\n",
        auth("agent"),
        BODY.len(),
        t = env.llm
    );
    s.write_all(head.as_bytes()).await.unwrap();
    crate::common::write_flushed(&mut s, BODY).await;
    let mut got = Vec::new();
    let mut buf = vec![0u8; 4096];
    let body_start = |got: &[u8]| got.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4);
    // Read until the first frame is in; the second is gated, so this can only finish if nothing buffers the answer.
    let first = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(i) = body_start(&got)
                && got.len() >= i + frames[0].len()
            {
                return got[i..].to_vec();
            }
            let n = s.read(&mut buf).await.unwrap();
            assert!(n > 0, "closed before the first frame");
            got.extend_from_slice(&buf[..n]);
        }
    })
    .await
    .expect("the first frame must arrive while the stream is still open");
    assert_eq!(first, frames[0], "first frame, unaltered");
    gate.notify_one();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut got)).await;
    let i = body_start(&got).unwrap();
    assert!(
        String::from_utf8_lossy(&got[..i]).starts_with("HTTP/1.1 200"),
        "{}",
        String::from_utf8_lossy(&got[..i])
    );
    assert_eq!(
        &got[i..],
        aurora_testkit::llm::bedrock::stream_body().as_slice()
    );
    assert!(rig(&env).mock.requests()[0].verified.is_ok());
}

/// (acceptance 3, D-121(d)) The carve-out covers the exact Bedrock host only: under egress `credentialed_only` (the
/// Default Rule `block`) an uncredentialed AWS request (S3's stand-in) is still blocked while Bedrock works.
#[tokio::test]
async fn llm_bedrock_carve_out_does_not_open_other_aws_hosts() {
    let env = start(Opts {
        bedrock: Some(BedrockMode::Sigv4),
        doc: json!({"llm": {"posture": "open", "endpoints": {"bedrock": {"allowed": true}}},
                    "egress": {"posture": "credentialed_only"}}),
        secrets: vec![("unused", true, None)],
        ..Opts::default()
    })
    .await;
    let r = request(&env, env.plain, "GET", "/bucket/key", "", b"").await;
    assert!(
        r.status == 403 && has(&r, "blocked_by_egress_policy"),
        "{} {}",
        r.status,
        r.body
    );
    let r = request(&env, env.llm, "POST", INVOKE, "", BODY).await;
    assert_eq!(r.status, 200, "{}", r.body);
}

/// (P5-MODELS-2, C-880) The model of a Bedrock request is in its path; recorded with no allow-list.
#[tokio::test]
async fn request_log_records_the_llm_model_of_a_bedrock_path() {
    let env = env_with(BedrockMode::Sigv4, "ext-org-1").await;
    let r = request(&env, env.llm, "POST", INVOKE, "", BODY).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let m = next_llm_model(&env, 0).await.1.unwrap_or_default();
    assert!(
        m.starts_with("us.anthropic.claude-3-haiku-20240307-v1"),
        "{m}"
    );
}

/// (FIX-177) A body the gateway must read whole (SigV4 signing) is answered when it is sent on a client socket
/// with a tight send buffer, as on a CI runner. There the body's last write finds the socket full and the end
/// of the body stays queued in the client's TLS session until a flush; the request helper used to read
/// without flushing, so the gateway waited for the rest of the `Content-Length` while the client waited for
/// the answer (the CI hang of the 413 test above). The test keeps sending until it has seen that case.
#[tokio::test]
async fn llm_bedrock_large_body_from_a_full_client_socket_is_answered() {
    let env = env_with(BedrockMode::Sigv4, "ext-org-1").await;
    let body = vec![b' '; 256 * 1024];
    let mut queued = 0;
    for attempt in 0..40 {
        let s = crate::common::connect_sndbuf(env.proxy, 4096).await;
        let r = request_on(s, env.llm, "POST", INVOKE, "", &body).await;
        assert_eq!(
            r.status, 200,
            "attempt {attempt} (end of the body queued in TLS: {}): {}, read ended: {}",
            r.queued, r.body, r.ended
        );
        if r.queued {
            queued += 1;
            if queued == 3 {
                break;
            }
        }
    }
    assert!(
        queued > 0,
        "no attempt found the client socket full: nothing was exercised"
    );
    assert!(
        rig(&env)
            .mock
            .requests()
            .iter()
            .all(|s| s.req.body.len() == body.len())
    );
}
