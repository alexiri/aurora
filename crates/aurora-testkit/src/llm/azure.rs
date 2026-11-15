//! In-process mock of an Azure OpenAI resource's v1 routes (`/openai/v1/{models,chat/completions,responses}`): it
//! accepts the resource key in `api-key` or an Entra token (`Authorization: Bearer <JWT>` issued by the Entra mock),
//! answers `401` otherwise, records every request (method, path, headers, body) and streams SSE for `"stream": true`.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::response::Response;

pub use super::vertex::Seen;

/// The byte-exact SSE body of a streamed completion.
pub const SSE: &str =
    "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n: keep-alive\n\ndata: [DONE]\n\n";

struct Shared {
    key: String,
    seen: Mutex<Vec<Seen>>,
}

#[derive(Clone)]
pub struct MockAzure {
    st: Arc<Shared>,
    base: String,
}

fn authentic(st: &Shared, req: &Request) -> bool {
    let h = req.headers();
    if h.get("api-key").and_then(|v| v.to_str().ok()) == Some(st.key.as_str()) {
        return true;
    }
    // An Entra token: a JWT (the Entra mock's tokens), never the resource key.
    h.get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| t.starts_with(super::entra::TOKEN_PREFIX) && t.matches('.').count() == 2)
}

async fn handle(State(st): State<Arc<Shared>>, req: Request) -> Response {
    let ok = authentic(&st, &req);
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 1 << 20)
        .await
        .unwrap_or_default();
    let path = parts.uri.path().to_owned();
    let body = String::from_utf8_lossy(&bytes).into_owned();
    st.seen.lock().unwrap().push(Seen {
        method: parts.method.to_string(),
        path: path.clone(),
        headers: parts
            .headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_owned()))
            .collect(),
        body: body.clone(),
    });
    let json = |status: StatusCode, b: &'static str| {
        Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(b))
            .unwrap()
    };
    if !ok {
        return json(
            StatusCode::UNAUTHORIZED,
            "{\"error\":{\"code\":\"401\",\"message\":\"Access denied due to invalid subscription key.\"}}",
        );
    }
    if path == "/openai/v1/models" {
        return json(StatusCode::OK, "{\"data\":[{\"id\":\"gpt-4o-base\"}]}");
    }
    if body.contains("\"stream\":true") || body.contains("\"stream\": true") {
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(SSE))
            .unwrap();
    }
    json(StatusCode::OK, "{\"ok\":true}")
}

impl MockAzure {
    /// A resource whose key is `key`.
    pub async fn start(key: &str) -> Self {
        let st = Arc::new(Shared {
            key: key.to_owned(),
            seen: Mutex::new(vec![]),
        });
        let app = Router::new().fallback(handle).with_state(st.clone());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move {
            let _ = axum::serve(l, app).await;
        });
        Self { st, base }
    }

    /// `http://127.0.0.1:PORT`: the endpoint's `base_url` (a loopback `http` base, as the config layer allows).
    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn requests(&self) -> Vec<Seen> {
        self.st.seen.lock().unwrap().clone()
    }
}
