//! The "is typing…" indicator (`native_status`, FIX-158 Part C, D-319).
//!
//! Mattermost shows the bot as typing for about five seconds after each
//! `POST /api/v4/users/{user_id}/typing`. `status.set` starts one refresher per
//! (sender, channel, thread) that repeats the call every [`REFRESH`]; `status.clear` stops it.
//! A refresher also ends on its own after [`MAX`] (a lost clear never types forever), and on a
//! 403/404 (the sender is not in the channel): no op failure, no retry storm.
//! A connector restart drops the refreshers; the next turn sets the indicator again.
//! Typing calls are not posts and do not count toward the write limits.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use aurora_protocol::connector::ErrorCode;
use serde_json::json;
use tokio::task::JoinHandle;

use crate::api::Mm;

/// Clients drop the indicator after about 5 s.
pub const REFRESH: Duration = Duration::from_secs(aurora_core::limits::TYPING_REFRESH_SECS);
/// A refresher that never received its `status.clear` stops after this long.
pub const MAX: Duration = aurora_core::limits::TURN_CEILING_WARNING;

pub struct Typing {
    refresh: Duration,
    max: Duration,
    running: Mutex<HashMap<String, JoinHandle<()>>>,
}

impl Default for Typing {
    fn default() -> Self {
        Self::with_timing(REFRESH, MAX)
    }
}

impl Typing {
    pub fn with_timing(refresh: Duration, max: Duration) -> Self {
        Self {
            refresh,
            max,
            running: Mutex::new(HashMap::new()),
        }
    }

    fn key(user: &str, channel: &str, parent: &str) -> String {
        format!("{user}\u{1f}{channel}\u{1f}{parent}")
    }

    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<String, JoinHandle<()>>> {
        self.running.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Starts (or keeps) the refresher of `(user, channel, parent)`. Two sets are one refresher.
    pub fn start(&self, mm: Mm, token: String, user: String, channel: String, parent: String) {
        let key = Self::key(&user, &channel, &parent);
        let mut running = self.map();
        running.retain(|_, h| !h.is_finished());
        if running.contains_key(&key) {
            return;
        }
        let (refresh, max) = (self.refresh, self.max);
        let task = tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + max;
            let mut body = json!({"channel_id": channel});
            if !parent.is_empty() {
                body["parent_id"] = json!(parent);
            }
            let path = format!("/users/{user}/typing");
            loop {
                match mm.post(&token, &path, &body).await {
                    Ok(_) => {}
                    Err(e) if matches!(e.code, ErrorCode::Forbidden | ErrorCode::NotFound) => {
                        tracing::debug!(channel = %channel, "typing refused; stopping");
                        return;
                    }
                    Err(e) => {
                        tracing::debug!(code = ?e.code, "typing call failed; will retry");
                    }
                }
                if tokio::time::Instant::now() + refresh >= deadline {
                    return;
                }
                tokio::time::sleep(refresh).await;
            }
        });
        running.insert(key, task);
    }

    /// Stops the refresher of `(user, channel, parent)`, if any.
    pub fn stop(&self, user: &str, channel: &str, parent: &str) {
        if let Some(h) = self.map().remove(&Self::key(user, channel, parent)) {
            h.abort();
        }
    }

    /// Live refreshers (tests).
    pub fn active(&self) -> usize {
        let mut running = self.map();
        running.retain(|_, h| !h.is_finished());
        running.len()
    }
}

impl Drop for Typing {
    fn drop(&mut self) {
        for (_, h) in self.map().drain() {
            h.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const FAST: Duration = Duration::from_millis(40);

    async fn server(status: u16) -> (MockServer, Mm) {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v4/users/U1/typing"))
            .and(header("authorization", "Bearer tok"))
            .and(body_json(json!({"channel_id": "C1", "parent_id": "P1"})))
            .respond_with(ResponseTemplate::new(status).set_body_json(json!({"status": "OK"})))
            .mount(&s)
            .await;
        let mm = Mm::with_policy(&s.uri(), &crate::api::NetPolicy::allow_all()).unwrap();
        (s, mm)
    }

    async fn calls(s: &MockServer) -> usize {
        s.received_requests().await.unwrap_or_default().len()
    }

    async fn until(mut f: impl FnMut() -> bool) {
        let end = tokio::time::Instant::now() + Duration::from_secs(10);
        while !f() {
            assert!(tokio::time::Instant::now() < end, "condition not reached");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn start(t: &Typing, mm: &Mm) {
        t.start(
            mm.clone(),
            "tok".into(),
            "U1".into(),
            "C1".into(),
            "P1".into(),
        );
    }

    #[tokio::test]
    async fn refreshes_with_the_senders_token_until_cleared() {
        let (s, mm) = server(200).await;
        let t = Typing::with_timing(FAST, Duration::from_secs(60));
        start(&t, &mm);
        let mut seen = 0;
        loop {
            seen = calls(&s).await.max(seen);
            if seen >= 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        t.stop("U1", "C1", "P1");
        assert_eq!(t.active(), 0);
        tokio::time::sleep(FAST * 4).await;
        let after = calls(&s).await;
        tokio::time::sleep(FAST * 4).await;
        assert_eq!(calls(&s).await, after, "no call after the clear");
    }

    #[tokio::test]
    async fn two_sets_are_one_refresher() {
        let (_s, mm) = server(200).await;
        let t = Typing::with_timing(FAST, Duration::from_secs(60));
        start(&t, &mm);
        start(&t, &mm);
        assert_eq!(t.active(), 1);
        t.stop("U1", "C1", "P1");
    }

    #[tokio::test]
    async fn stops_by_itself_at_the_maximum() {
        let (s, mm) = server(200).await;
        let t = Typing::with_timing(FAST, FAST * 5);
        start(&t, &mm);
        until(|| t.active() == 0).await;
        assert!(calls(&s).await >= 1);
    }

    #[tokio::test]
    async fn a_403_stops_quietly() {
        let (s, mm) = server(403).await;
        let t = Typing::with_timing(FAST, Duration::from_secs(60));
        start(&t, &mm);
        until(|| t.active() == 0).await;
        assert_eq!(calls(&s).await, 1, "one call, no retry storm");
    }
}
