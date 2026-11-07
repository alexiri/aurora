//! Transform stage: `git_commit_trailer` (docs/09 §5).

use super::Unavailable;
use super::template::{Dest, Vars, render};
use crate::Body;
use aurora_registry::manifest::CommitTrailer;
use http::{HeaderMap, HeaderValue, Method, header};

/// Bodies larger than this are forwarded untouched.
pub const MAX_BODY: usize = 1024 * 1024;

fn is_trailer_line(l: &str) -> bool {
    l.split_once(": ").is_some_and(|(k, _)| {
        !k.is_empty() && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })
}

/// Append `trailer` to a commit message unless already present as a line.
pub fn append_trailer(message: &str, trailer: &str) -> Option<String> {
    if message.lines().any(|l| l.trim() == trailer) {
        return None;
    }
    let msg = message.trim_end();
    let last_para = msg.rsplit("\n\n").next().unwrap_or("");
    let in_trailer_block =
        msg.contains("\n\n") && !last_para.is_empty() && last_para.lines().all(is_trailer_line);
    let sep = if in_trailer_block { "\n" } else { "\n\n" };
    Some(format!("{msg}{sep}{trailer}"))
}

pub async fn git_commit_trailer(
    t: &CommitTrailer,
    v: &Vars<'_>,
    method: &Method,
    headers: &mut HeaderMap,
    body: Body,
) -> Result<Body, Unavailable> {
    let json_ct = headers
        .get(header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|c| c.to_ascii_lowercase().contains("json"));
    if method != Method::POST || !json_ct {
        return Ok(body);
    }
    let trailer = render(&t.trailer, Dest::Header, v)?.text;
    if trailer.is_empty() {
        return Ok(body);
    }
    let (prefix, truncated, replay) = crate::decide::buffer_prefix(body, MAX_BODY).await;
    if truncated {
        return Ok(replay);
    }
    let Ok(mut doc) = serde_json::from_slice::<serde_json::Value>(&prefix) else {
        return Ok(replay);
    };
    let Some(msg) = doc.get("message").and_then(|m| m.as_str()) else {
        return Ok(replay);
    };
    let Some(new_msg) = append_trailer(msg, &trailer) else {
        return Ok(replay);
    };
    doc["message"] = serde_json::Value::String(new_msg);
    let out = serde_json::to_vec(&doc).map_err(|_| Unavailable)?;
    headers.remove(header::TRANSFER_ENCODING);
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(out.len()));
    Ok(crate::full_body(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_once() {
        let t = "Co-Authored-By: Agent";
        let m = append_trailer("fix thing", t).unwrap();
        assert_eq!(m, "fix thing\n\nCo-Authored-By: Agent");
        assert!(append_trailer(&m, t).is_none());
        let m2 = append_trailer("fix\n\nbody\n\nSigned-off-by: X", t).unwrap();
        assert_eq!(m2, "fix\n\nbody\n\nSigned-off-by: X\nCo-Authored-By: Agent");
    }
}
