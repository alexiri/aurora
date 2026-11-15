//! In-process mock of Bedrock: the runtime (`/model/{id}/invoke`, `invoke-with-response-stream`, `converse*`, the OpenAI
//! surface under `/openai/v1`) and the control plane (`/foundation-models`, `/inference-profiles`) on one loopback port.
//! Every request is verified the way AWS does: SigV4 from known secrets (and their session tokens), or a Bedrock API key
//! as `Authorization: Bearer`; the streaming routes answer a real event-stream frame sequence; a refusal carries
//! `x-amzn-ErrorType`. It records what each request carried (verified access key, headers, body) so a test can prove which
//! credential arrived and which never did.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use super::aws::{self, Refusal, Verified};
pub use super::vertex::Seen;
use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::response::Response;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// One request as the mock saw it: the request and the verification outcome.
#[derive(Debug, Clone)]
pub struct BedrockSeen {
    pub req: Seen,
    pub verified: Result<Verified, Refusal>,
    /// The Bedrock API key presented as a bearer token, when it was the valid one.
    pub bearer_ok: bool,
}

#[derive(Default)]
struct Shared {
    /// access key id → (secret, required session token).
    keys: Mutex<BTreeMap<String, (String, Option<String>)>>,
    api_key: Mutex<Option<String>>,
    seen: Mutex<Vec<BedrockSeen>>,
    /// Status the control-plane list routes answer (200 or e.g. 403).
    list_status: Mutex<u16>,
}

#[derive(Clone)]
pub struct MockBedrock {
    st: Arc<Shared>,
    base: String,
}

/// The event-stream frames of one streamed answer (a `chunk` event twice, then the stream ends).
pub fn stream_body() -> Vec<u8> {
    let mut out = Vec::new();
    for t in ["hel", "lo"] {
        let payload = format!("{{\"bytes\":\"{}\"}}", base64_of(t));
        out.extend(aws::event_frame(
            &[
                (":event-type", "chunk"),
                (":content-type", "application/json"),
                (":message-type", "event"),
            ],
            payload.as_bytes(),
        ));
    }
    out
}

fn base64_of(s: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(s)
}

fn refused(status: StatusCode, error_type: &str) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-amzn-ErrorType", error_type)
        .body(Body::from(format!("{{\"message\":\"{error_type}\"}}")))
        .unwrap()
}

async fn handle(State(st): State<Arc<Shared>>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 16 << 20)
        .await
        .unwrap_or_default();
    let path = parts.uri.path().to_owned();
    let headers: Vec<(String, String)> = parts
        .headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_owned()))
        .collect();
    let method = parts.method.to_string();
    let verified = {
        let keys = st.keys.lock().unwrap();
        aws::verify(&method, &path, parts.uri.query(), &headers, &bytes, |ak| {
            keys.get(ak).cloned()
        })
    };
    let bearer_ok = headers
        .iter()
        .find(|(k, _)| k == "authorization")
        .and_then(|(_, v)| v.strip_prefix("Bearer "))
        .is_some_and(|t| st.api_key.lock().unwrap().as_deref() == Some(t));
    st.seen.lock().unwrap().push(BedrockSeen {
        req: Seen {
            method,
            path: path.clone(),
            headers,
            body: String::from_utf8_lossy(&bytes).into_owned(),
        },
        verified: verified.clone(),
        bearer_ok,
    });
    if !bearer_ok {
        match &verified {
            Ok(_) => {}
            Err(Refusal::UnknownKey(_) | Refusal::BadToken) => {
                return refused(StatusCode::FORBIDDEN, "UnrecognizedClientException");
            }
            Err(Refusal::BadSignature) => {
                return refused(StatusCode::FORBIDDEN, "InvalidSignatureException");
            }
            Err(Refusal::Unsigned) => {
                return refused(StatusCode::FORBIDDEN, "MissingAuthenticationToken");
            }
        }
    }
    let json = |v: String| {
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v))
            .unwrap()
    };
    match path.as_str() {
        "/foundation-models" | "/inference-profiles" => {
            let status = *st.list_status.lock().unwrap();
            if status != 200 {
                return refused(
                    StatusCode::from_u16(status).unwrap(),
                    "AccessDeniedException",
                );
            }
            if path == "/foundation-models" {
                json(r#"{"modelSummaries":[{"modelId":"anthropic.claude-3-haiku-20240307-v1:0","modelName":"Claude 3 Haiku"}]}"#.into())
            } else {
                json(r#"{"inferenceProfileSummaries":[{"inferenceProfileId":"us.anthropic.claude-3-haiku-20240307-v1:0","inferenceProfileName":"US Claude 3 Haiku"}]}"#.into())
            }
        }
        p if p.ends_with("/invoke-with-response-stream") || p.ends_with("/converse-stream") => {
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/vnd.amazon.eventstream")
                .body(Body::from(stream_body()))
                .unwrap()
        }
        _ => json(r#"{"type":"message","content":[{"type":"text","text":"ok"}]}"#.into()),
    }
}

impl MockBedrock {
    /// A mock that knows no credentials yet; add them with [`Self::accept_key`] / [`Self::accept_api_key`].
    pub async fn start() -> Self {
        let st = Arc::new(Shared {
            list_status: Mutex::new(200),
            ..Shared::default()
        });
        let app = Router::new().fallback(handle).with_state(st.clone());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move {
            let _ = axum::serve(l, app).await;
        });
        Self { st, base }
    }

    /// Accept requests signed by `access_key` (with the session token it must carry, for temporary keys).
    pub fn accept_key(&self, access_key: &str, secret: &str, session_token: Option<&str>) {
        self.st.keys.lock().unwrap().insert(
            access_key.to_owned(),
            (secret.to_owned(), session_token.map(str::to_owned)),
        );
    }

    /// Accept `Authorization: Bearer <key>`.
    pub fn accept_api_key(&self, key: &str) {
        *self.st.api_key.lock().unwrap() = Some(key.to_owned());
    }

    /// The status of the control-plane list routes (`403` makes the API fall back to the static models).
    pub fn set_list_status(&self, status: u16) {
        *self.st.list_status.lock().unwrap() = status;
    }

    /// `http://127.0.0.1:PORT`: the runtime base and, via `llm.dev_upstream_overrides`, the control plane.
    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn requests(&self) -> Vec<BedrockSeen> {
        self.st.seen.lock().unwrap().clone()
    }
}
