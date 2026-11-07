#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! BE-067: agent avatars (SEC-30): raw upload, re-encode to 256×256 PNG, public key rotation,
//! nosniff, decompression-bomb refusal, and the permission edges.
mod common;

use axum::body::Body;
use common::*;
use http::{HeaderMap, Method, Request};
use http_body_util::BodyExt;
use image::{DynamicImage, ImageFormat, RgbImage};
use serde_json::Value;
use std::io::Cursor;
use tower::ServiceExt;
use uuid::Uuid;

struct World {
    h: Harness,
    agent: Uuid,
    member: String,
    outsider: String,
}

async fn world() -> World {
    let h = Harness::new().await;
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let member = h.user("m@x.io", false).await;
    let outsider = h.user("o@x.io", false).await;
    h.member(org, member, "member", "active").await;
    h.member(org, outsider, "member", "active").await;
    h.bind_ws(ws, member, "member").await;
    let agent = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness) VALUES ($1,$2,$3,'a1','A1','hosted','fake')")
        .bind(agent).bind(org).bind(ws).execute(h.pool()).await.unwrap();
    let (_, m) = h.good_session(member).await;
    let (_, o) = h.good_session(outsider).await;
    World {
        h,
        agent,
        member: cookie(&m),
        outsider: cookie(&o),
    }
}

async fn raw(
    h: &Harness,
    m: Method,
    uri: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> (u16, HeaderMap, Vec<u8>) {
    let mut b = Request::builder().method(m).uri(uri);
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let res = h
        .app
        .clone()
        .oneshot(b.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let (p, body) = res.into_parts();
    (
        p.status.as_u16(),
        p.headers,
        body.collect().await.unwrap().to_bytes().to_vec(),
    )
}

impl World {
    fn uri(&self) -> String {
        format!("/v1/agents/{}/avatar", self.agent)
    }
    async fn put_as(&self, cookie: &str, ct: &str, body: Vec<u8>) -> (u16, Value) {
        let (s, _, b) = raw(
            &self.h,
            Method::PUT,
            &self.uri(),
            &[("cookie", cookie), ("origin", ORIGIN), ("content-type", ct)],
            body,
        )
        .await;
        (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
    }
    async fn put(&self, body: Vec<u8>) -> (u16, Value) {
        self.put_as(&self.member, "application/octet-stream", body)
            .await
    }
    async fn public(&self, url_path: &str) -> (u16, HeaderMap, Vec<u8>) {
        raw(&self.h, Method::GET, url_path, &[], vec![]).await
    }
}

fn image(w: u32, h: u32) -> DynamicImage {
    let mut img = RgbImage::new(w, h);
    for (x, y, p) in img.enumerate_pixels_mut() {
        *p = image::Rgb([(x % 256) as u8, (y % 256) as u8, 128]);
    }
    DynamicImage::ImageRgb8(img)
}

fn encode(img: &DynamicImage, f: ImageFormat) -> Vec<u8> {
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), f).unwrap();
    out
}

fn assert_clean_png(bytes: &[u8]) {
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    let img = image::load_from_memory_with_format(bytes, ImageFormat::Png).unwrap();
    assert_eq!((img.width(), img.height()), (256, 256));
    for chunk in [&b"tEXt"[..], b"zTXt", b"iTXt", b"eXIf", b"iCCP"] {
        assert!(
            !bytes.windows(4).any(|w| w == chunk),
            "metadata chunk {chunk:?} survived"
        );
    }
}

#[tokio::test]
async fn png_jpeg_webp_are_reencoded_to_256_png() {
    let w = world().await;
    for (f, name) in [
        (ImageFormat::Png, "png"),
        (ImageFormat::Jpeg, "jpeg"),
        (ImageFormat::WebP, "webp"),
    ] {
        // Non-square on purpose: it must be cover-cropped, not stretched or rejected.
        let (s, v) = w.put(encode(&image(640, 320), f)).await;
        assert_eq!(s, 200, "{name}: {v}");
        let url = v["avatar_url"].as_str().unwrap().to_owned();
        assert!(url.starts_with("/v1/public/avatars/"), "{url}");
        let (s, hd, body) = w.public(&url).await;
        assert_eq!(s, 200, "{name}");
        assert_eq!(hd["content-type"], "image/png");
        assert_clean_png(&body);
        // The authenticated route serves the same bytes.
        let (s, hd, authed) = raw(
            &w.h,
            Method::GET,
            &w.uri(),
            &[("cookie", &w.member)],
            vec![],
        )
        .await;
        assert_eq!(
            (s, hd["content-type"].to_str().unwrap()),
            (200, "image/png")
        );
        assert_eq!(authed, body);
    }
}

#[tokio::test]
async fn polyglot_and_metadata_laden_uploads_are_served_clean() {
    let w = world().await;
    // PNG with a tEXt chunk and a script payload appended after IEND.
    let mut png = encode(&image(300, 300), ImageFormat::Png);
    let iend = png.len() - 12;
    png.splice(iend..iend, {
        let mut c = Vec::new();
        let data = b"Comment\0SECRETMARKER1";
        c.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = b"tEXt".to_vec();
        body.extend_from_slice(data);
        let crc = crc32(&body);
        c.extend_from_slice(&body);
        c.extend_from_slice(&crc.to_be_bytes());
        c
    });
    png.extend_from_slice(b"<script>alert('TRAILERMARKER')</script>");
    // JPEG with an APP1/Exif segment and trailing bytes after EOI.
    let mut jpg = encode(&image(300, 300), ImageFormat::Jpeg);
    let exif = b"Exif\0\0SECRETGPSMARKER2";
    let mut seg = vec![0xFF, 0xE1];
    seg.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
    seg.extend_from_slice(exif);
    jpg.splice(2..2, seg);
    jpg.extend_from_slice(b"TRAILERMARKER3");

    for (name, input) in [("png", png), ("jpeg", jpg)] {
        let (s, v) = w.put(input.clone()).await;
        assert_eq!(s, 200, "{name}: {v}");
        let url = v["avatar_url"].as_str().unwrap();
        let (s, hd, out) = w.public(url).await;
        assert_eq!(s, 200);
        assert_eq!(hd["content-type"], "image/png");
        assert_clean_png(&out);
        for marker in [
            "SECRETMARKER1",
            "SECRETGPSMARKER2",
            "TRAILERMARKER",
            "<script",
        ] {
            assert!(
                !out.windows(marker.len()).any(|x| x == marker.as_bytes()),
                "{name}: {marker} leaked"
            );
        }
        assert_ne!(out, input);
        // IEND is the last chunk: nothing trails the fresh encode.
        assert_eq!(&out[out.len() - 8..out.len() - 4], b"IEND");
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for b in data {
        c ^= u32::from(*b);
        for _ in 0..8 {
            c = if c & 1 == 1 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

#[tokio::test]
async fn type_is_sniffed_and_bad_inputs_are_refused() {
    let w = world().await;
    // A real PNG sent with a lying Content-Type is accepted: the bytes decide.
    let png = encode(&image(64, 64), ImageFormat::Png);
    let (s, _) = w.put_as(&w.member, "text/html", png).await;
    assert_eq!(s, 200);
    // SVG, plain text, GIF-looking bytes and an empty body are all 422 avatar_invalid.
    for (name, body) in [
        (
            "svg",
            b"<svg xmlns='http://www.w3.org/2000/svg'><script>1</script></svg>".to_vec(),
        ),
        ("text", b"just text".to_vec()),
        ("gif", b"GIF89a\x01\x00\x01\x00\x00\x00\x00;".to_vec()),
        ("empty", Vec::new()),
        (
            "truncated png",
            encode(&image(64, 64), ImageFormat::Png)[..40].to_vec(),
        ),
    ] {
        let (s, v) = w.put_as(&w.member, "image/png", body).await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (422, Some("avatar_invalid")),
            "{name}: {v}"
        );
    }
}

#[tokio::test]
async fn oversize_is_413_and_dimension_bomb_is_refused() {
    let w = world().await;
    let (s, _) = w.put(vec![0u8; aurora_core::limits::AVATAR_MAX + 1]).await;
    assert_eq!(s, 413);
    // 5000×5000 grayscale compresses far below the byte cap but exceeds the 4096 pixel limit.
    let bomb = encode(
        &DynamicImage::ImageLuma8(image::GrayImage::new(5000, 5000)),
        ImageFormat::Png,
    );
    assert!(
        bomb.len() < aurora_core::limits::AVATAR_MAX,
        "{}",
        bomb.len()
    );
    let (s, v) = w.put(bomb).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("avatar_invalid")),
        "{v}"
    );
    // Nothing was stored for either refusal.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM blobs")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn public_key_rotates_and_old_key_is_404() {
    let w = world().await;
    let png = encode(&image(100, 100), ImageFormat::Png);
    let (_, a) = w.put(png.clone()).await;
    let first = a["avatar_url"].as_str().unwrap().to_owned();
    let (_, b) = w.put(png).await;
    let second = b["avatar_url"].as_str().unwrap().to_owned();
    assert_ne!(first, second);
    let key = second.rsplit('/').next().unwrap();
    assert!(key.len() >= 43, "key carries >= 256 bits of entropy: {key}");

    let (s, hd, body) = w.public(&second).await;
    assert_eq!(s, 200);
    assert_eq!(hd["x-content-type-options"], "nosniff");
    assert_eq!(hd["content-type"], "image/png");
    assert!(hd["cache-control"].to_str().unwrap().contains("max-age"));
    assert_clean_png(&body);

    let (s_old, _, old_body) = w.public(&first).await;
    assert_eq!(s_old, 404);
    // No oracle: an unknown well-formed key and a malformed key answer identically.
    let unknown = format!("/v1/public/avatars/{}", "A".repeat(43));
    let (s_unk, _, unk_body) = w.public(&unknown).await;
    let (s_bad, _, bad_body) = w.public("/v1/public/avatars/short").await;
    assert_eq!((s_unk, s_bad), (404, 404));
    assert_eq!(old_body, unk_body);
    assert_eq!(unk_body, bad_body);
    // The previous blob was deleted: exactly one blob remains.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM blobs")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn delete_removes_avatar_and_blob() {
    let w = world().await;
    let (_, a) = w.put(encode(&image(80, 80), ImageFormat::Png)).await;
    let url = a["avatar_url"].as_str().unwrap().to_owned();
    let (s, _, _) = raw(
        &w.h,
        Method::DELETE,
        &w.uri(),
        &[("cookie", &w.member), ("origin", ORIGIN)],
        vec![],
    )
    .await;
    assert_eq!(s, 204);
    assert_eq!(w.public(&url).await.0, 404);
    let (s, _, _) = raw(
        &w.h,
        Method::GET,
        &w.uri(),
        &[("cookie", &w.member)],
        vec![],
    )
    .await;
    assert_eq!(s, 404);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM blobs")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
    let (s, v) =
        w.h.get(&format!("/v1/agents/{}", w.agent), &[("cookie", &w.member)])
            .await;
    assert_eq!((s.as_u16(), &v["avatar_url"]), (200, &Value::Null));
}

#[tokio::test]
async fn avatar_routes_require_agent_access() {
    let w = world().await;
    let png = encode(&image(64, 64), ImageFormat::Png);
    // No credentials.
    let (s, _, _) = raw(
        &w.h,
        Method::PUT,
        &w.uri(),
        &[("origin", ORIGIN)],
        png.clone(),
    )
    .await;
    assert_eq!(s, 401);
    let (s, _, _) = raw(&w.h, Method::GET, &w.uri(), &[], vec![]).await;
    assert_eq!(s, 401);
    // An org member without a binding to the workspace cannot see the agent: 404.
    let (s, _) = w.put_as(&w.outsider, "image/png", png).await;
    assert_eq!(s, 404);
    let (s, _, _) = raw(
        &w.h,
        Method::GET,
        &w.uri(),
        &[("cookie", &w.outsider)],
        vec![],
    )
    .await;
    assert_eq!(s, 404);
    let (s, _, _) = raw(
        &w.h,
        Method::DELETE,
        &w.uri(),
        &[("cookie", &w.outsider), ("origin", ORIGIN)],
        vec![],
    )
    .await;
    assert_eq!(s, 404);
}
