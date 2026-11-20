//! (P5-REFINE-6, C-1067, D-419) Send-only apps have their own levels. The Outgoing webhook (the
//! shipped manifest: one write tool `send`, no read tool) offers `ask` and `act`, never `read`:
//! the connect journey and the grant `PUT` refuse `read` with `422 level_not_offered` before it
//! is expanded, `ask` saves "ask for every send" and both read back as themselves.
//!
//! A child module of `connect` (it reuses that file's world and helpers).
use super::*;

fn manifest() -> String {
    include_str!("../../../../plugins/builtin/webhook/plugin.yaml")
        .replace("publisher: aurora", "publisher: acme")
}

fn presets() -> Vec<(String, Vec<u8>)> {
    macro_rules! p {
        ($f:literal) => {
            (
                concat!("presets/", $f).to_owned(),
                include_bytes!(concat!("../../../../plugins/builtin/webhook/presets/", $f))
                    .to_vec(),
            )
        };
    }
    vec![
        p!("slack.json"),
        p!("mattermost.json"),
        p!("google-chat.json"),
        p!("discord.json"),
        p!("teams-workflow.json"),
        p!("ntfy-json.json"),
        p!("home-assistant.json"),
        p!("generic-json.json"),
        p!("ifttt.json"),
    ]
}

struct S {
    w: W,
    agent: Uuid,
}

async fn lane() -> S {
    let w = world_with_files(None, &manifest(), false, &presets()).await;
    let agent = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES ($1,$2,$3,'dani','Dani','byo','medium','')")
        .bind(agent).bind(w.org).bind(w.ws).execute(w.h.pool()).await.unwrap();
    S { w, agent }
}

fn webhook_config() -> Value {
    json!({
        "version": 1, "method": "POST", "content_type": "application/json",
        "template": "{\"text\": {{text}}}",
        "slots": [{"name": "text", "type": "string", "required": true, "description": "The message"}],
        "headers": [], "description": "Posts to ops", "signing": false,
        "rate": {"per_minute": null, "per_day": null}, "preset": null
    })
}

impl S {
    async fn connect(&self, level: Option<&str>) -> (u16, Value) {
        let mut b = self.w.connect_body("link");
        b["fields"] = json!({"name": "Ops alerts", "url": "https://hooks.example/hooks/x"});
        b["webhook"] = webhook_config();
        if let Some(l) = level {
            b["grant"] = json!({"agent_ids": [self.agent], "level": l});
        }
        let (s, _, v) = self
            .w
            .call(
                &self.w.owner,
                Method::POST,
                "/v1/apps/webhook/connect",
                Some(b),
            )
            .await;
        (s, v)
    }

    async fn call(&self, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        let (s, _, v) = self.w.call(&self.w.owner, m, uri, body).await;
        (s, v)
    }

    async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
            .fetch_one(self.w.h.pool())
            .await
            .unwrap()
    }

    /// The webhook account of the workspace's accounts list.
    async fn account(&self) -> Value {
        let (s, v) = self
            .call(
                Method::GET,
                &format!("/v1/workspaces/{}/accounts", self.w.ws),
                None,
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["app"]["app_id"] == "webhook")
            .cloned()
            .unwrap_or_else(|| panic!("no webhook account: {v}"))
    }

    async fn grant_row(&self) -> (String, Vec<String>, Vec<String>) {
        sqlx::query_as(
            "SELECT g.mode, g.allow_tools, g.ask_tools FROM grants g \
             JOIN app_connections c ON c.id = g.connection_id WHERE g.agent_id = $1",
        )
        .bind(self.agent)
        .fetch_one(self.w.h.pool())
        .await
        .unwrap()
    }
}

#[tokio::test]
async fn the_outgoing_webhook_offers_ask_and_act_and_refuses_read() {
    let l = lane().await;
    // The catalogue says it only sends.
    let (s, v) = l.call(Method::GET, "/v1/apps/webhook", None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["send_only"], json!(true), "{v}");

    // `read` is refused before anything is written: no account, no grant.
    let (s, v) = l.connect(Some("read")).await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["code"], "level_not_offered", "{v}");
    assert_eq!(v["error"]["details"]["app_id"], "webhook");
    assert_eq!(v["error"]["details"]["offered"], json!(["ask", "act"]));
    assert_eq!(l.count("app_connections").await, 0);
    assert_eq!(l.count("grants").await, 0);

    // `ask` saves "ask for every send" and reads back as `ask`.
    let (s, v) = l.connect(Some("ask")).await;
    assert_eq!(s, 200, "{v}");
    let conn = v["connection"]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        l.grant_row().await,
        ("custom".to_owned(), vec![], vec!["send".to_owned()])
    );
    let a = l.account().await;
    assert_eq!(a["levels"], json!(["ask", "act"]), "{a}");
    assert_eq!(a["app"]["send_only"], json!(true), "{a}");
    assert_eq!(a["agents"][0]["level"], "ask", "{a}");

    // The grant PUT: `read` is refused the same way and changes nothing; `act` reads back `act`.
    let uri = format!("/v1/agents/{}/grants/connections/{conn}", l.agent);
    let (s, v) = l
        .call(Method::PUT, &uri, Some(json!({"level": "read"})))
        .await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["code"], "level_not_offered", "{v}");
    assert_eq!(v["error"]["details"]["offered"], json!(["ask", "act"]));
    assert_eq!(l.grant_row().await.2, vec!["send".to_owned()]);
    let (s, v) = l
        .call(Method::PUT, &uri, Some(json!({"level": "act"})))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(l.grant_row().await.0, "full");
    assert_eq!(l.account().await["agents"][0]["level"], "act");
    let (s, v) = l
        .call(Method::PUT, &uri, Some(json!({"level": "ask"})))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(l.account().await["agents"][0]["level"], "ask");
}

#[tokio::test]
async fn act_through_the_journey_reads_back_act() {
    let l = lane().await;
    let (s, v) = l.connect(Some("act")).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(l.grant_row().await.0, "full");
    let a = l.account().await;
    assert_eq!(a["agents"][0]["level"], "act", "{a}");
}

#[tokio::test]
async fn an_organization_account_of_a_send_only_app_takes_no_read_limit() {
    let l = lane().await;
    let mut b = json!({"scope": "organization", "org_id": l.w.org, "method_id": "link",
        "limit": "read"});
    b["fields"] = json!({"name": "Ops alerts", "url": "https://hooks.example/hooks/x"});
    b["webhook"] = webhook_config();
    let (s, _, v) = l
        .w
        .call(
            &l.w.owner,
            Method::POST,
            "/v1/apps/webhook/connect",
            Some(b),
        )
        .await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["code"], "level_not_offered", "{v}");
    assert_eq!(l.count("app_connections").await, 0);
}
