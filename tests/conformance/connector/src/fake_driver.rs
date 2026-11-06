//! [`PlatformDriver`] for `connectors/fake`, over its `/fake/*` control API.

use std::time::Duration;

use anyhow::{Context, bail};
use async_trait::async_trait;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::driver::*;

pub struct FakeDriver {
    base: String,
    http: reqwest::Client,
}

impl FakeDriver {
    /// `base` = origin of the connector's control listener (`http://127.0.0.1:PORT`).
    pub fn new(base: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_owned(),
            http: reqwest::Client::new(),
        }
    }

    async fn post_json(&self, path: &str, body: Value) -> Result<Value> {
        let r = self
            .http
            .post(format!("{}{path}", self.base))
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {path}"))?;
        let status = r.status();
        let v: Value = r.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            bail!("POST {path} -> {status}: {v}");
        }
        Ok(v)
    }

    async fn get_json(&self, path: &str) -> Result<Value> {
        let r = self
            .http
            .get(format!("{}{path}", self.base))
            .send()
            .await
            .with_context(|| format!("GET {path}"))?;
        let status = r.status();
        let v: Value = r.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            bail!("GET {path} -> {status}: {v}");
        }
        Ok(v)
    }
}

fn ack(v: &Value) -> Result<Ack> {
    match v["ack"].as_str() {
        Some("accepted") => Ok(Ack::Accepted),
        Some("duplicate") => Ok(Ack::Duplicate),
        Some("rejected") => Ok(Ack::Rejected(v["code"].as_str().unwrap_or("").to_owned())),
        _ => bail!("no ack in {v}"),
    }
}

#[async_trait]
impl PlatformDriver for FakeDriver {
    async fn wait_ready(&self, timeout: Duration) -> Result<()> {
        let end = tokio::time::Instant::now() + timeout;
        loop {
            if let Ok(v) = self.get_json("/fake/status").await
                && v["ready"] == json!(true)
            {
                return Ok(());
            }
            if tokio::time::Instant::now() >= end {
                bail!("connector did not open an integration within {timeout:?}");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn capabilities(&self) -> Result<Vec<String>> {
        let v = self.get_json("/fake/status").await?;
        Ok(serde_json::from_value(v["capabilities"].clone())?)
    }

    async fn add_user(&self, u: UserSpec) -> Result<()> {
        self.post_json(
            "/fake/users",
            json!({"ref": u.reference, "display_name": u.display_name, "email": u.email,
                   "email_verified": u.email_verified, "is_bot": u.is_bot}),
        )
        .await
        .map(|_| ())
    }

    async fn add_space(&self, s: SpaceSpec) -> Result<()> {
        self.post_json(
            "/fake/spaces",
            json!({"ref": s.reference, "kind": s.kind, "label": s.label}),
        )
        .await
        .map(|_| ())
    }

    async fn host_file(&self, f: FileSpec) -> Result<()> {
        self.post_json(
            "/fake/files",
            json!({"ref": f.reference, "name": f.name, "content_type": f.content_type,
                   "content": f.content}),
        )
        .await
        .map(|_| ())
    }

    async fn post_message(&self, m: MessageSpec) -> Result<Delivered> {
        let files: Vec<Value> = m
            .files
            .iter()
            .map(|(r, name, size, ct)| {
                json!({"ref": r, "name": name, "size": size, "content_type": ct})
            })
            .collect();
        let v = self
            .post_json(
                "/fake/messages",
                json!({"space_ref": m.space, "thread_ref": m.thread_ref, "author_ref": m.author,
                       "text": m.text, "files": files, "event_id": m.event_id,
                       "message_ref": m.message_ref}),
            )
            .await?;
        Ok(Delivered {
            ack: ack(&v)?,
            message_ref: v["message_ref"].as_str().unwrap_or_default().to_owned(),
        })
    }

    async fn click(&self, c: Click) -> Result<Ack> {
        let v = self
            .post_json(
                "/fake/actions",
                json!({"token": c.token, "clicker_ref": c.clicker, "value": c.value,
                       "space_ref": c.space_ref, "message_ref": c.message_ref}),
            )
            .await?;
        ack(&v)
    }

    async fn bot_added(&self, space_ref: &str, bot_ref: &str, actor_ref: &str) -> Result<Ack> {
        let v = self
            .post_json(
                "/fake/bot/added",
                json!({"space_ref": space_ref, "bot_ref": bot_ref, "actor_ref": actor_ref}),
            )
            .await?;
        ack(&v)
    }

    async fn presence_removed(
        &self,
        bot_ref: &str,
        presence_id: Option<Uuid>,
        reason: &str,
    ) -> Result<Ack> {
        let v = self
            .post_json(
                "/fake/presence-removed",
                json!({"bot_ref": bot_ref, "presence_id": presence_id, "reason": reason}),
            )
            .await?;
        ack(&v)
    }

    async fn inject(&self, f: Failure) -> Result<()> {
        let body = match f {
            Failure::DeadCredentials => json!({"dead_credentials": true}),
            Failure::LoseResults(n) => json!({"lose_results": n}),
        };
        self.post_json("/fake/failures", body).await.map(|_| ())
    }

    async fn request_link(&self, user_ref: &str) -> Result<String> {
        let v = self
            .post_json("/fake/link-start", json!({"platform_user_ref": user_ref}))
            .await?;
        Ok(v["url"].as_str().context("no url")?.to_owned())
    }

    async fn posts(&self, space_ref: Option<&str>) -> Result<Vec<Post>> {
        let path = match space_ref {
            Some(s) => format!("/fake/posted?space_ref={s}"),
            None => "/fake/posted".to_owned(),
        };
        let v = self.get_json(&path).await?;
        Ok(serde_json::from_value(v["messages"].clone())?)
    }

    async fn attachment(&self, message_ref: &str, name: &str) -> Result<Option<Vec<u8>>> {
        let r = self
            .http
            .get(format!(
                "{}/fake/attachments/{message_ref}/{name}",
                self.base
            ))
            .send()
            .await?;
        if r.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(r.error_for_status()?.bytes().await?.to_vec()))
    }

    fn buttons(&self, post: &Post) -> Vec<Button> {
        blocks(post, "buttons")
            .flat_map(|b| b["items"].as_array().cloned().unwrap_or_default())
            .map(|i| Button {
                label: i["label"].as_str().unwrap_or_default().to_owned(),
                token: i["token"].as_str().unwrap_or_default().to_owned(),
            })
            .collect()
    }

    fn select(&self, post: &Post) -> Option<Select> {
        blocks(post, "select").next().map(|b| Select {
            token: b["token"].as_str().unwrap_or_default().to_owned(),
            options: b["options"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|o| {
                    (
                        o["label"].as_str().unwrap_or_default().to_owned(),
                        o["value"].as_str().unwrap_or_default().to_owned(),
                    )
                })
                .collect(),
        })
    }

    fn live_broadcasts(&self, post: &Post) -> Vec<String> {
        let mut texts = vec![post.text.clone()];
        if let Some(card) = &post.card {
            collect_strings(card, &mut texts);
        }
        let mut live = vec![];
        for t in texts {
            for w in ["@channel", "@all", "@here"] {
                if t.contains(w) {
                    live.push(w.to_owned());
                }
            }
        }
        live
    }
}

fn blocks<'a>(post: &'a Post, ty: &'a str) -> impl Iterator<Item = &'a Value> {
    post.card
        .iter()
        .flat_map(|c| c["blocks"].as_array().into_iter().flatten())
        .filter(move |b| b["type"] == json!(ty))
}

fn collect_strings(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| collect_strings(x, out)),
        Value::Object(o) => o.values().for_each(|x| collect_strings(x, out)),
        _ => {}
    }
}
