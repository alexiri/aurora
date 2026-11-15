//! In-process mock of Vertex AI prediction routes: records every request (method, path, headers, body) and answers
//! `rawPredict` / `generateContent` with JSON, `streamRawPredict` / `streamGenerateContent` with a fixed SSE body, the
//! OpenAI-compatible `chat/completions` with JSON.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::response::Response;

/// The byte-exact SSE body of a `streamRawPredict` answer.
pub const SSE: &str = "event: message_start\ndata: {\"type\":\"message_start\"}\n\n: keep-alive\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    pub path: String,
    /// Lowercase header names.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Seen {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Clone)]
pub struct MockVertex {
    seen: Arc<Mutex<Vec<Seen>>>,
    base: String,
}

async fn handle(State(seen): State<Arc<Mutex<Vec<Seen>>>>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 1 << 20)
        .await
        .unwrap_or_default();
    let path = parts.uri.path().to_owned();
    seen.lock().unwrap().push(Seen {
        method: parts.method.to_string(),
        path: path.clone(),
        headers: parts
            .headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_owned()))
            .collect(),
        body: String::from_utf8_lossy(&bytes).into_owned(),
    });
    let stream = path.ends_with(":streamRawPredict") || path.ends_with(":streamGenerateContent");
    let mut b = Response::builder().status(StatusCode::OK);
    if stream {
        b = b.header(header::CONTENT_TYPE, "text/event-stream");
        b.body(Body::from(SSE)).unwrap()
    } else {
        b = b.header(header::CONTENT_TYPE, "application/json");
        b.body(Body::from("{\"ok\":true}")).unwrap()
    }
}

impl MockVertex {
    pub async fn start() -> Self {
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let app = Router::new().fallback(handle).with_state(seen.clone());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move {
            let _ = axum::serve(l, app).await;
        });
        Self { seen, base }
    }

    /// `http://127.0.0.1:PORT`: the target of the endpoint host's `llm.dev_upstream_overrides` entry.
    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}
