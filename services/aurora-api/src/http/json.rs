//! `Json`, `Query` and `Path` extractors whose rejections are 400 `invalid_request`
//! with `details.fields = [{path, message}]`.

use aurora_core::{ApiError, FieldError};
use axum::body::Bytes;
use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::response::{IntoResponse, Response};
use http::{StatusCode, header, request::Parts};
use serde::{Serialize, de::DeserializeOwned};
use serde_path_to_error::Segment;

pub struct Json<T>(pub T);

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

fn is_json(req: &Request) -> bool {
    req.headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            let m = v
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            m == "application/json" || (m.starts_with("application/") && m.ends_with("+json"))
        })
        .unwrap_or(false)
}

fn pointer(path: &serde_path_to_error::Path) -> String {
    let mut out = String::new();
    for seg in path.iter() {
        match seg {
            Segment::Seq { index } => out.push_str(&format!("/{index}")),
            Segment::Map { key } => {
                out.push('/');
                out.push_str(&key.replace('~', "~0").replace('/', "~1"));
            }
            Segment::Enum { variant } => out.push_str(&format!("/{variant}")),
            Segment::Unknown => {}
        }
    }
    out
}

impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for Json<T> {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, ApiError> {
        if !is_json(&req) {
            return Err(ApiError::invalid_request(
                "Content-Type must be application/json.",
                vec![FieldError::new(
                    "",
                    "expected Content-Type: application/json",
                )],
            ));
        }
        let bytes = Bytes::from_request(req, state).await.map_err(|e| {
            if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
                ApiError::too_large("Request body too large.")
            } else {
                ApiError::invalid_request("Could not read request body.", vec![])
            }
        })?;
        let mut de = serde_json::Deserializer::from_slice(&bytes);
        serde_path_to_error::deserialize(&mut de)
            .map(Json)
            .map_err(|e| {
                ApiError::invalid_request(
                    "Invalid request body.",
                    vec![FieldError::new(pointer(e.path()), e.inner().to_string())],
                )
            })
    }
}

pub struct Query<T>(pub T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequestParts<S> for Query<T> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiError> {
        axum::extract::Query::<T>::from_request_parts(parts, state)
            .await
            .map(|q| Query(q.0))
            .map_err(|e| {
                ApiError::invalid_request(
                    "Invalid query parameters.",
                    vec![FieldError::new("", e.body_text())],
                )
            })
    }
}

pub struct Path<T>(pub T);

impl<T: DeserializeOwned + Send, S: Send + Sync> FromRequestParts<S> for Path<T> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiError> {
        axum::extract::Path::<T>::from_request_parts(parts, state)
            .await
            .map(|p| Path(p.0))
            .map_err(|e| {
                ApiError::invalid_request(
                    "Invalid path parameter.",
                    vec![FieldError::new("", e.body_text())],
                )
            })
    }
}
