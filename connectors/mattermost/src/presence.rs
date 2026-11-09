//! Dedicated presences (docs/20 §Operations): `presence.provision`, `.deprovision`,
//! `.rename`, `.set_icon`. Every call needs the integration's system-admin token
//! (the manifest only offers `presence_dedicated` when it is present and works).

use std::sync::Arc;
use std::time::Duration;

use aurora_protocol::connector::*;
use reqwest::Method;
use serde_json::{Value, json};

use crate::api::{R, is_mm_id};
use crate::integ::Integ;

/// Mattermost usernames are 3–22 characters.
const USERNAME_MIN: usize = 3;
const USERNAME_MAX: usize = 22;
/// Avatars come from the Aurora API (re-encoded PNG, 256×256): far below this.
const AVATAR_MAX_BYTES: usize = 5 * 1024 * 1024;
const PRESENCE_WAIT_STEPS: u32 = 50;

fn err(code: ErrorCode, msg: impl Into<String>) -> OpError {
    OpError::new(code, msg)
}

/// A Mattermost username from an agent handle: lowercase `[a-z0-9._-]`, starts with a
/// letter, 3–22 characters.
pub fn bot_username(handle: &str) -> String {
    let mut s: String = handle
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect();
    s = s.trim_matches(['.', '-', '_']).to_owned();
    if !s.chars().next().is_some_and(|c| c.is_ascii_lowercase()) {
        s.insert(0, 'a');
    }
    while s.len() < USERNAME_MIN {
        s.push('x');
    }
    s.truncate(USERNAME_MAX);
    s
}

/// Usernames to try in order: `<handle>`, `<handle>-agent`, then numbered variants.
pub fn candidates(handle: &str) -> Vec<String> {
    let base = bot_username(handle);
    let with = |suffix: &str| {
        let keep = USERNAME_MAX.saturating_sub(suffix.len());
        let mut b = base.clone();
        b.truncate(keep);
        format!("{}{suffix}", b.trim_end_matches(['.', '-', '_']))
    };
    let mut out = vec![base.clone(), with("-agent")];
    out.extend((2..=5).map(|n| with(&format!("-agent{n}"))));
    out.dedup();
    out
}

fn admin(integ: &Integ) -> R<String> {
    integ.snapshot().admin_token.ok_or_else(|| {
        err(
            ErrorCode::Forbidden,
            "dedicated presences need a system-admin token on the integration",
        )
    })
}

fn marker(op: &Operation) -> String {
    format!("aurora-op:{}", op.op_id)
}

/// The user id of a presence's bot: from the config the core handed us, else (a
/// self-contained deprovision) from the op's own payload.
async fn bot_of(integ: &Arc<Integ>, op: &Operation, c: &PresenceChangeOp, wait: bool) -> R<String> {
    let steps = if wait { PRESENCE_WAIT_STEPS } else { 1 };
    for i in 0..steps {
        if let Some(p) = integ.snapshot().presences.get(&c.presence_id) {
            return Ok(p.user_id.clone());
        }
        if i + 1 < steps {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    op.payload
        .get("platform_user_ref")
        .and_then(Value::as_str)
        .filter(|r| is_mm_id(r))
        .map(str::to_owned)
        .ok_or_else(|| err(ErrorCode::NotFound, "unknown presence"))
}

/// `presence.provision`: a bot account of its own for the agent.
pub async fn provision(integ: &Arc<Integ>, op: &Operation, p: PresenceProvisionOp) -> R<Value> {
    let admin = admin(integ)?;
    let description = format!("Aurora agent {} [{}]", p.agent.handle, marker(op));
    let display = p.agent.name.trim();
    let display = if display.is_empty() {
        p.agent.handle.as_str()
    } else {
        display
    };
    let mut bot: Option<(String, String)> = None;
    for name in candidates(&p.agent.handle) {
        match integ
            .mm
            .get(&admin, &format!("/users/username/{name}"))
            .await
        {
            Ok(u) => {
                // A retried provision finds its own half-made bot by the op marker.
                let id = u.get("id").and_then(Value::as_str).unwrap_or_default();
                if is_mm_id(id)
                    && integ
                        .mm
                        .get(&admin, &format!("/bots/{id}"))
                        .await
                        .ok()
                        .and_then(|b| {
                            b.get("description")
                                .and_then(Value::as_str)
                                .map(|d| d.contains(&marker(op)))
                        })
                        .unwrap_or(false)
                {
                    bot = Some((id.to_owned(), name));
                    break;
                }
            }
            Err(e) if e.code == ErrorCode::NotFound => {
                let made = integ
                    .mm
                    .post(
                        &admin,
                        "/bots",
                        &json!({"username": name, "display_name": display, "description": description}),
                    )
                    .await;
                match made {
                    Ok(v) => {
                        let id = v
                            .get("user_id")
                            .and_then(Value::as_str)
                            .filter(|i| is_mm_id(i))
                            .ok_or_else(|| {
                                err(
                                    ErrorCode::Unavailable,
                                    "bot creation answered without an id",
                                )
                            })?;
                        bot = Some((id.to_owned(), name));
                        break;
                    }
                    // Lost a race for the name: try the next candidate.
                    Err(e) if e.code == ErrorCode::Invalid => {}
                    Err(e) => return Err(e),
                }
            }
            Err(e) => return Err(e),
        }
    }
    let (bot_id, username) =
        bot.ok_or_else(|| err(ErrorCode::Invalid, "no free bot username for this handle"))?;

    let token = finish_provision(integ, &admin, &bot_id, p.agent.avatar_url.as_deref()).await;
    match token {
        Ok(t) => Ok(json!(PresenceProvisionResult {
            platform_user_ref: bot_id,
            handle: username,
            credentials: json!({"token": t.0, "token_id": t.1}),
        })),
        Err(e) => {
            // Do not leave an orphan bot behind; a retry makes a new one.
            let _ = integ
                .mm
                .send(
                    &admin,
                    Method::POST,
                    &format!("/bots/{bot_id}/disable"),
                    &[],
                    None,
                )
                .await;
            Err(e)
        }
    }
}

/// Team membership, access token and (best effort) avatar. Returns `(token, token_id)`.
async fn finish_provision(
    integ: &Arc<Integ>,
    admin: &str,
    bot_id: &str,
    avatar_url: Option<&str>,
) -> R<(String, String)> {
    integ
        .mm
        .post(
            admin,
            &format!("/teams/{}/members", integ.team_id),
            &json!({"team_id": integ.team_id, "user_id": bot_id}),
        )
        .await?;
    let t = integ
        .mm
        .post(
            admin,
            &format!("/users/{bot_id}/tokens"),
            &json!({"description": "aurora"}),
        )
        .await?;
    let token = t.get("token").and_then(Value::as_str);
    let id = t.get("id").and_then(Value::as_str);
    let (Some(token), Some(id)) = (token, id) else {
        return Err(err(
            ErrorCode::Unavailable,
            "token creation answered without a token",
        ));
    };
    if let Some(url) = avatar_url
        && let Err(e) = apply_icon(integ, admin, bot_id, Some(url)).await
    {
        tracing::info!(error = %e.message, "bot avatar not applied");
    }
    Ok((token.to_owned(), id.to_owned()))
}

/// `presence.rename`: display name (and handle, when it changed).
pub async fn rename(integ: &Arc<Integ>, op: &Operation, c: PresenceChangeOp) -> R<Value> {
    let admin = admin(integ)?;
    let bot = bot_of(integ, op, &c, true).await?;
    let mut patch = serde_json::Map::new();
    if let Some(n) = c.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        patch.insert("display_name".into(), json!(n));
    }
    if let Some(h) = c.handle.as_deref().filter(|h| !h.trim().is_empty()) {
        patch.insert("username".into(), json!(bot_username(h)));
    }
    if patch.is_empty() {
        return Err(err(ErrorCode::Invalid, "rename needs a name or a handle"));
    }
    integ
        .mm
        .json::<Value>(
            &admin,
            Method::PUT,
            &format!("/bots/{bot}"),
            &[],
            Some(&Value::Object(patch)),
        )
        .await?;
    Ok(json!({}))
}

/// `presence.set_icon`: `avatar_url` sets the picture, absent resets it.
pub async fn set_icon(integ: &Arc<Integ>, op: &Operation, c: PresenceChangeOp) -> R<Value> {
    let admin = admin(integ)?;
    let bot = bot_of(integ, op, &c, true).await?;
    apply_icon(integ, &admin, &bot, c.avatar_url.as_deref()).await?;
    Ok(json!({}))
}

/// `presence.deprovision` (RFE-024, self-contained): revoke every token, then disable the
/// bot, acting from the op's payload (`platform_user_ref`, `config.server_url`) and its
/// `credentials` (the admin token) alone. The integration, presence, agent and org rows may
/// already be deleted and no replica may own the integration, so nothing is looked up. A bot
/// that is already gone is `not_found` (the core records it as dropped).
pub async fn deprovision(
    open: &[Arc<Integ>],
    op: &Operation,
    c: PresenceDeprovisionOp,
) -> R<Value> {
    let bot = c.platform_user_ref.as_str();
    if !is_mm_id(bot) {
        return Err(err(ErrorCode::Invalid, "bad platform_user_ref"));
    }
    let server = c
        .config
        .get("server_url")
        .and_then(Value::as_str)
        .ok_or_else(|| err(ErrorCode::Invalid, "config.server_url missing"))?;
    let mm = crate::api::Mm::new(server).map_err(|e| err(ErrorCode::Invalid, e))?;
    let admin = op
        .credentials
        .as_ref()
        .and_then(|v| v.get("admin_token"))
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            err(
                ErrorCode::Forbidden,
                "the operation carries no system-admin token",
            )
        })?;
    // Our own disable must not come back as a `presence.removed` event.
    for integ in open {
        integ.retire_bot(bot);
    }
    let tokens = mm.get(admin, &format!("/users/{bot}/tokens")).await?;
    for t in tokens.as_array().into_iter().flatten() {
        if let Some(id) = t.get("id").and_then(Value::as_str) {
            mm.post(admin, "/users/tokens/revoke", &json!({"token_id": id}))
                .await?;
        }
    }
    mm.send(
        admin,
        Method::POST,
        &format!("/bots/{bot}/disable"),
        &[],
        None,
    )
    .await?;
    Ok(json!({}))
}

// ------------------------------------------------------------------ avatars

async fn apply_icon(integ: &Integ, admin: &str, bot: &str, url: Option<&str>) -> R<()> {
    let Some(url) = url else {
        integ
            .mm
            .send(
                admin,
                Method::DELETE,
                &format!("/users/{bot}/image"),
                &[],
                None,
            )
            .await?;
        return Ok(());
    };
    let (bytes, content_type) = fetch_avatar(integ, url).await?;
    integ
        .mm
        .set_user_image(admin, bot, bytes, &content_type)
        .await
}

/// Resolves `avatar_url` against the Aurora API and refuses any other origin: the
/// connector never fetches arbitrary URLs.
fn resolve_avatar(api_url: &str, avatar_url: &str) -> Result<reqwest::Url, OpError> {
    let base = reqwest::Url::parse(api_url)
        .map_err(|_| err(ErrorCode::Unavailable, "the Aurora API URL is not known"))?;
    let url = base
        .join(avatar_url)
        .map_err(|_| err(ErrorCode::Invalid, "bad avatar_url"))?;
    let same_origin = url.scheme() == base.scheme()
        && url.host_str() == base.host_str()
        && url.port_or_known_default() == base.port_or_known_default();
    if !same_origin {
        return Err(err(
            ErrorCode::Invalid,
            "avatar_url must be served by the Aurora API",
        ));
    }
    Ok(url)
}

fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF8") {
        Some("image/gif")
    } else {
        None
    }
}

async fn fetch_avatar(integ: &Integ, avatar_url: &str) -> R<(Vec<u8>, String)> {
    let api = integ
        .settings
        .api_url
        .as_deref()
        .ok_or_else(|| err(ErrorCode::Unavailable, "the Aurora API URL is not known"))?;
    let url = resolve_avatar(api, avatar_url)?;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| err(ErrorCode::Unavailable, e.to_string()))?;
    let mut resp = http
        .get(url)
        .send()
        .await
        .map_err(|e| err(ErrorCode::Unavailable, format!("avatar fetch failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(err(
            ErrorCode::Invalid,
            format!("avatar fetch answered {}", resp.status().as_u16()),
        ));
    }
    let mut out: Vec<u8> = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| err(ErrorCode::Unavailable, format!("avatar fetch failed: {e}")))?
    {
        if out.len() + chunk.len() > AVATAR_MAX_BYTES {
            return Err(err(ErrorCode::TooLarge, "avatar is too large"));
        }
        out.extend_from_slice(&chunk);
    }
    let ct = sniff(&out).ok_or_else(|| err(ErrorCode::Invalid, "avatar is not an image"))?;
    Ok((out, ct.to_owned()))
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn usernames_follow_mattermost_rules() {
        assert_eq!(bot_username("Scout"), "scout");
        assert_eq!(bot_username("my agent!"), "my-agent");
        assert_eq!(bot_username("7zip"), "a7zip");
        assert_eq!(bot_username("x"), "xxx");
        assert_eq!(bot_username(&"a".repeat(40)).len(), 22);
    }

    #[test]
    fn collisions_fall_back_to_agent_suffix_within_the_limit() {
        let c = candidates("scout");
        assert_eq!(c[0], "scout");
        assert_eq!(c[1], "scout-agent");
        let long = candidates(&"b".repeat(30));
        assert!(long.iter().all(|n| n.len() <= USERNAME_MAX), "{long:?}");
        assert!(long[1].ends_with("-agent"));
        let mut all = long.clone();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), long.len());
    }

    #[test]
    fn avatars_only_come_from_the_aurora_api() {
        let ok = resolve_avatar("http://127.0.0.1:4100", "/v1/avatars/x.png").unwrap();
        assert_eq!(ok.as_str(), "http://127.0.0.1:4100/v1/avatars/x.png");
        assert!(resolve_avatar("http://127.0.0.1:4100", "http://127.0.0.1:4100/a.png").is_ok());
        assert!(resolve_avatar("http://127.0.0.1:4100", "http://evil.test/a.png").is_err());
        assert!(resolve_avatar("http://127.0.0.1:4100", "http://127.0.0.1:9999/a.png").is_err());
        assert!(resolve_avatar("https://api.test", "http://api.test/a.png").is_err());
        assert!(resolve_avatar("http://127.0.0.1:4100", "//evil.test/a.png").is_err());
    }

    #[test]
    fn image_sniffing() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\nrest"), Some("image/png"));
        assert_eq!(sniff(&[0xff, 0xd8, 0xff, 0xe0]), Some("image/jpeg"));
        assert_eq!(sniff(b"<html>"), None);
    }
}
