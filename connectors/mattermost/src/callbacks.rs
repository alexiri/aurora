//! Public callbacks (docs/20 §Public callbacks): `POST /mm/actions` (interactive
//! messages), `POST /mm/commands` (slash command), plus `GET /mm/ready` and,
//! in the test profile only, `POST /mm/test/redeliver`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use aurora_connector_sdk::EventAck;
use aurora_protocol::connector::{ActionInvoked, CommandInvoked, EventKind};
use axum::extract::{DefaultBodyLimit, Form, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::connector::Shared;
use crate::events::space;
use crate::integ::Integ;
use crate::render::verify;

/// Callbacks answer within Mattermost's patience even when the core is slow.
const CORE_ACK_WAIT: Duration = Duration::from_secs(5);
const BODY_LIMIT: usize = 64 * 1024;

pub fn router(shared: Arc<Shared>) -> Router {
    let mut r = Router::new()
        .route("/mm/actions", post(actions))
        .route("/mm/commands", post(commands))
        .route("/mm/ready", get(ready));
    if shared.settings.test_control {
        tracing::warn!("test control mounted at /mm/test/*");
        r = r
            .route("/mm/test/redeliver", post(redeliver))
            .route("/mm/test/lose-results", post(lose_results))
            .route("/mm/test/bots", get(test_bots));
    }
    r.layer(DefaultBodyLimit::max(BODY_LIMIT))
        .with_state(shared)
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ActionRequest {
    user_id: String,
    channel_id: String,
    post_id: String,
    trigger_id: String,
    selected_option: String,
    context: HashMap<String, Value>,
}

fn forbidden(msg: &str) -> Response {
    (StatusCode::FORBIDDEN, Json(json!({"error": msg}))).into_response()
}

fn ctx_str<'a>(m: &'a HashMap<String, Value>, k: &str) -> &'a str {
    m.get(k).and_then(Value::as_str).unwrap_or("")
}

/// Mattermost does not sign callbacks: the connector's HMAC over the action
/// token is the only proof the button came from a card this connector rendered.
async fn actions(State(shared): State<Arc<Shared>>, Json(req): Json<ActionRequest>) -> Response {
    let token = ctx_str(&req.context, "token");
    let sig = ctx_str(&req.context, "sig");
    if token.is_empty() || !verify(&shared.settings.hmac_key, token, sig) {
        tracing::warn!("interactive callback with a bad signature rejected");
        return forbidden("bad signature");
    }
    let integ = match Uuid::parse_str(ctx_str(&req.context, "iid"))
        .ok()
        .and_then(|id| shared.integration(id))
    {
        Some(i) => i,
        None => return forbidden("unknown integration"),
    };
    if req.user_id.is_empty() {
        return forbidden("no clicker");
    }
    // Mattermost reports the chosen option inside the action context (older and
    // newer servers also at the top level).
    let selected = if req.selected_option.is_empty() {
        ctx_str(&req.context, "selected_option").to_owned()
    } else {
        req.selected_option.clone()
    };
    let payload = ActionInvoked {
        token: token.to_owned(),
        clicker_ref: req.user_id.clone(),
        value: (!selected.is_empty()).then_some(selected),
        space_ref: (!req.channel_id.is_empty()).then(|| req.channel_id.clone()),
        message_ref: (!req.post_id.is_empty()).then(|| req.post_id.clone()),
    };
    let id = format!(
        "action:{}:{}:{}",
        req.post_id,
        req.user_id,
        if req.trigger_id.is_empty() {
            Uuid::now_v7().to_string()
        } else {
            req.trigger_id.clone()
        }
    );
    let ack = tokio::time::timeout(
        CORE_ACK_WAIT,
        integ.emit(id, EventKind::ActionInvoked, &payload),
    )
    .await;
    match ack {
        Ok(Some(a)) if !matches!(a, EventAck::Rejected(_)) => Json(json!({})).into_response(),
        Ok(_) => (StatusCode::BAD_GATEWAY, Json(json!({}))).into_response(),
        // The event is queued and retried by the SDK; the click is not lost.
        Err(_) => Json(json!({})).into_response(),
    }
}

/// The integration of `team_id` whose slash-command token is `token`. Several integrations may
/// serve one team; the token is what says which of them the command was registered for, so a team
/// is never resolved to "the first integration in it".
fn find_by_team_and_token(shared: &Shared, team_id: &str, token: &str) -> Option<Arc<Integ>> {
    if token.is_empty() {
        return None;
    }
    shared.all().into_iter().find(|i| {
        i.team_id == team_id
            && i.snapshot()
                .command_token
                .as_deref()
                .is_some_and(|t| ct_eq(t, token))
    })
}

fn ct_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

async fn commands(
    State(shared): State<Arc<Shared>>,
    Form(f): Form<HashMap<String, String>>,
) -> Response {
    let get = |k: &str| f.get(k).map(String::as_str).unwrap_or("");
    let Some(integ) = find_by_team_and_token(&shared, get("team_id"), get("token")) else {
        // Unknown team, unknown token and wrong token look the same from outside.
        return forbidden("bad token");
    };
    let text = get("text").trim();
    let (name, args) = match text.split_once(char::is_whitespace) {
        Some((n, a)) => (n.to_ascii_lowercase(), a.trim().to_owned()),
        None if text.is_empty() => ("help".to_owned(), String::new()),
        None => (text.to_ascii_lowercase(), String::new()),
    };
    let channel = match integ.channel(get("channel_id")).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e.message, "slash command in an unknown channel");
            return forbidden("unknown channel");
        }
    };
    let payload = CommandInvoked {
        name,
        args,
        space: space(&channel),
        thread_ref: None,
        user_ref: get("user_id").to_owned(),
        response_ref: None,
    };
    let id = format!("command:{}", get("trigger_id"));
    let id = if id == "command:" {
        format!("command:{}", Uuid::now_v7())
    } else {
        id
    };
    let _ = tokio::time::timeout(
        CORE_ACK_WAIT,
        integ.emit(id, EventKind::CommandInvoked, &payload),
    )
    .await;
    Json(json!({"response_type": "ephemeral", "text": "Working…"})).into_response()
}

async fn ready(State(shared): State<Arc<Shared>>) -> Response {
    let integrations = shared.all();
    let n = integrations.iter().filter(|i| i.is_ready()).count();
    let code = if n > 0 {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (code, Json(json!({"ready": n}))).into_response()
}

/// Test profile: re-runs catch-up now (a redelivery of recent platform events).
async fn redeliver(State(shared): State<Arc<Shared>>) -> Response {
    for i in shared.all() {
        i.catch_up().await;
    }
    Json(json!({"ok": true})).into_response()
}

#[derive(Deserialize)]
struct LoseQuery {
    n: u32,
}

/// Test profile: drop the results of the next `n` `message.post` ops.
async fn lose_results(
    State(shared): State<Arc<Shared>>,
    axum::extract::Query(q): axum::extract::Query<LoseQuery>,
) -> Response {
    shared
        .lose_results
        .store(q.n, std::sync::atomic::Ordering::SeqCst);
    Json(json!({"ok": true})).into_response()
}

/// Test profile: the dedicated bots the open integrations know (user ids), so a suite can
/// wait until a presence it handed the core has been picked up.
async fn test_bots(State(shared): State<Arc<Shared>>) -> Json<Value> {
    let ids: Vec<String> = shared
        .integrations
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .flat_map(|i| {
            i.snapshot()
                .presences
                .into_values()
                .map(|p| p.user_id)
                .collect::<Vec<_>>()
        })
        .collect();
    Json(json!({"bots": ids}))
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::api::NetPolicy;
    use crate::integ::Settings;
    use crate::render::sign;
    use aurora_connector_sdk::{CoreClient, IntegrationHandle, RetryPolicy, spawn_event_poster};
    use aurora_protocol::connector::{EventsRequest, EventsResponse, Integration, SharedBot};
    use axum::body::Body;
    use axum::http::Request;
    use std::collections::BTreeMap;
    use std::sync::atomic::AtomicU32;
    use std::sync::{Mutex, RwLock};
    use tower::ServiceExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, Request as WmRequest, Respond, ResponseTemplate};

    #[test]
    fn constant_time_compare() {
        assert!(ct_eq("abc", "abc"));
        assert!(!ct_eq("abc", "abd"));
        assert!(!ct_eq("abc", "abcd"));
        assert!(!ct_eq("", "x"));
    }

    /// The fake core accepts every event it is sent.
    struct AcceptAll;
    impl Respond for AcceptAll {
        fn respond(&self, req: &WmRequest) -> ResponseTemplate {
            let ev: EventsRequest = serde_json::from_slice(&req.body).unwrap();
            ResponseTemplate::new(200).set_body_json(EventsResponse {
                accepted: ev.events.iter().map(|e| e.event_id.clone()).collect(),
                duplicates: vec![],
                rejected: vec![],
            })
        }
    }

    const KEY: &[u8] = b"test-hmac-key";
    const CHANNEL: &str = "chan0000000000000000000001";

    struct World {
        router: Router,
        shared: Arc<Shared>,
        core: MockServer,
        a: Uuid,
        b: Uuid,
    }

    fn integration(id: Uuid, mm: &MockServer, team: &str, command_token: &str) -> Integration {
        Integration {
            id,
            org_id: Uuid::nil(),
            tenant_ref: format!("mm.test/{team}"),
            tenant_label: "T".into(),
            config: json!({"server_url": mm.uri()}),
            credentials: json!({"bot_token": "bot", "command_token": command_token}),
            credentials_version: 1,
            shared_bot: Some(SharedBot {
                platform_user_ref: "bot00000000000000000000001".into(),
                handle: "aurora".into(),
                display_name: "Aurora".into(),
            }),
            status: "active".into(),
        }
    }

    /// Two integrations in team `team1` (tokens `tok-a`, `tok-b`) and one in `team2`.
    async fn world() -> World {
        let core = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/connector/events"))
            .respond_with(AcceptAll)
            .mount(&core)
            .await;
        let mm = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v4/channels/{CHANNEL}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": CHANNEL, "team_id": "team1", "type": "O", "name": "general",
                "display_name": "General"
            })))
            .mount(&mm)
            .await;
        let client =
            Arc::new(CoreClient::new(&core.uri(), "aun_test", RetryPolicy::default()).unwrap());
        let (_tx, rx) = tokio::sync::watch::channel(false);
        let sink = spawn_event_poster(client, rx);
        // Keep the shutdown sender alive for the whole test process.
        std::mem::forget(_tx);
        let settings = Settings {
            public_url: Some("https://connector.test".into()),
            hmac_key: KEY.to_vec(),
            catchup_window: Duration::from_secs(60),
            api_url: None,
            token_check: Duration::from_secs(3600),
            capabilities: None,
            test_control: false,
            net: NetPolicy::allow_all(),
        };
        let declared = ["commands".to_owned()].into_iter().collect();
        let mut map = BTreeMap::new();
        let (a, b, c) = (Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3));
        for (id, team, tok) in [
            (a, "team1", "tok-a"),
            (b, "team1", "tok-b"),
            (c, "team2", "tok-c"),
        ] {
            let i = Integ::from_config(
                &integration(id, &mm, team, tok),
                &[],
                &declared,
                sink.clone(),
                IntegrationHandle::new(id),
                settings.clone(),
            )
            .unwrap();
            map.insert(id, Arc::new(i));
        }
        let shared = Arc::new(Shared {
            settings,
            integrations: RwLock::new(map),
            core: Mutex::new(None),
            lose_results: AtomicU32::new(0),
        });
        // Leak the mock Mattermost: the integrations hold its address for the whole test.
        std::mem::forget(mm);
        World {
            router: router(shared.clone()),
            shared,
            core,
            a,
            b,
        }
    }

    async fn post_json(w: &World, uri: &str, body: Value) -> StatusCode {
        let req = Request::post(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        w.router.clone().oneshot(req).await.unwrap().status()
    }

    async fn post_form(w: &World, form: &[(&str, &str)]) -> StatusCode {
        let body = form
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        let req = Request::post("/mm/commands")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(body))
            .unwrap();
        w.router.clone().oneshot(req).await.unwrap().status()
    }

    /// The events the core was handed: `(integration id, kind)`.
    async fn emitted(w: &World) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for r in w.core.received_requests().await.unwrap() {
            if let Ok(req) = serde_json::from_slice::<Value>(&r.body) {
                for e in req["events"].as_array().cloned().unwrap_or_default() {
                    out.push((
                        e["integration_id"].as_str().unwrap_or_default().to_owned(),
                        e["kind"].as_str().unwrap_or_default().to_owned(),
                    ));
                }
            }
        }
        out
    }

    fn action(w: &World, iid: Uuid, token: &str, sig: &str, user: &str) -> Value {
        let _ = w;
        json!({
            "user_id": user, "channel_id": CHANNEL, "post_id": "post0000000000000000000001",
            "trigger_id": "t1",
            "context": {"token": token, "sig": sig, "iid": iid.to_string()},
        })
    }

    /// Action callbacks: a bad signature, an unknown integration or no clicker is refused and
    /// nothing reaches the core.
    #[tokio::test]
    async fn action_callbacks_are_refused_without_a_valid_signed_context() {
        let w = world().await;
        let ok_sig = sign(KEY, "aat_x");
        for (body, why) in [
            (action(&w, w.a, "aat_x", "AAAA", "u1"), "bad signature"),
            (action(&w, w.a, "aat_x", "", "u1"), "missing signature"),
            (action(&w, w.a, "", "", "u1"), "missing token"),
            (
                action(&w, w.a, "aat_x", &sign(b"other-key", "aat_x"), "u1"),
                "wrong key",
            ),
            (
                action(&w, w.a, "aat_other", &ok_sig, "u1"),
                "signature for another token",
            ),
            (
                action(&w, Uuid::from_u128(99), "aat_x", &ok_sig, "u1"),
                "unknown integration",
            ),
            (action(&w, w.a, "aat_x", &ok_sig, ""), "no clicker"),
            (
                json!({"context": {"token": "aat_x", "sig": ok_sig}}),
                "no integration",
            ),
        ] {
            assert_eq!(
                post_json(&w, "/mm/actions", body).await,
                StatusCode::FORBIDDEN,
                "{why}"
            );
        }
        assert!(emitted(&w).await.is_empty(), "nothing reaches the core");
        // The same request with a valid signature is accepted and reaches the core once.
        let st = post_json(&w, "/mm/actions", action(&w, w.a, "aat_x", &ok_sig, "u1")).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(
            emitted(&w).await,
            vec![(w.a.to_string(), "action.invoked".to_owned())]
        );
    }

    /// Slash commands: wrong or missing token, unknown team, and another team's token are
    /// refused; the integration whose token matches gets the command, even when several serve the
    /// team.
    #[tokio::test]
    async fn slash_commands_route_by_token_and_refuse_everything_else() {
        let w = world().await;
        let cmd = |team: &'static str, token: &'static str| {
            vec![
                ("team_id", team),
                ("token", token),
                ("channel_id", CHANNEL),
                ("user_id", "user0000000000000000000001"),
                ("text", "help"),
                ("trigger_id", "trig"),
            ]
        };
        for (team, token, why) in [
            ("team1", "wrong", "wrong token"),
            ("team1", "", "missing token"),
            ("nope", "tok-a", "unknown team"),
            ("team2", "tok-a", "a token of another team's integration"),
            ("team1", "tok-c", "the other team's token"),
        ] {
            assert_eq!(
                post_form(&w, &cmd(team, token)).await,
                StatusCode::FORBIDDEN,
                "{why}"
            );
        }
        assert!(emitted(&w).await.is_empty());
        // Two integrations serve team1: each one's token reaches that integration only.
        assert_eq!(post_form(&w, &cmd("team1", "tok-b")).await, StatusCode::OK);
        assert_eq!(
            emitted(&w).await,
            vec![(w.b.to_string(), "command.invoked".to_owned())]
        );
        let mut again = cmd("team1", "tok-a");
        again[5] = ("trigger_id", "trig2");
        assert_eq!(post_form(&w, &again).await, StatusCode::OK);
        let seen = emitted(&w).await;
        assert!(
            seen.contains(&(w.a.to_string(), "command.invoked".to_owned())),
            "{seen:?}"
        );
        let _ = &w.shared;
    }

    #[test]
    fn two_integrations_in_a_team_get_different_triggers() {
        use crate::connector::command_trigger;
        let (a, b, c) = (Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3));
        let team = [a, b, c];
        assert_eq!(command_trigger(a, &team), "aurora");
        let tb = command_trigger(b, &team);
        let tc = command_trigger(c, &team);
        assert!(tb.starts_with("aurora-") && tc.starts_with("aurora-"));
        assert_ne!(tb, tc);
        // Alone in the team: the plain trigger.
        assert_eq!(command_trigger(c, &[c]), "aurora");
    }
}
