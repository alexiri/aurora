#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-REFINE-8 (D-417, D-407, C-1080..C-1082, SEC-135 amended): one attention feed per level, the level
//! rule (a request counted once), reasons on asks, messages on declines, history, and the rail counts.
//! A child module of `app_setup` (its world: a workspace owner, a member, an organization admin, the
//! instance admin `root`, and the `lin` app that needs an OAuth client).
use super::*;
use aurora_api::domains::app_setup_requests::service::reevaluate;

impl W {
    async fn feed(&self, who: &str, uri: &str) -> Value {
        let (s, v) = self.call(who, Method::GET, uri, None).await;
        assert_eq!(s, 200, "{uri}: {v}");
        v
    }

    fn ws_feed(&self, state: &str) -> String {
        format!("/v1/workspaces/{}/attention?state={state}", self.ws)
    }

    fn org_feed(&self, state: &str) -> String {
        format!("/v1/orgs/{}/attention?state={state}", self.org)
    }

    fn counts_uri(&self) -> String {
        format!(
            "/v1/attention/counts?org={}&workspace={}",
            self.org, self.ws
        )
    }

    async fn ask_req(&self, who: &str, body: Value) -> (u16, Value) {
        self.call(
            who,
            Method::POST,
            &format!("/v1/workspaces/{}/requests", self.ws),
            Some(body),
        )
        .await
    }

    async fn ask_app(&self, who: &str, app: &str, body: Option<Value>) -> (u16, Value) {
        self.call(
            who,
            Method::POST,
            &format!("/v1/workspaces/{}/apps/{app}/setup-request", self.ws),
            body,
        )
        .await
    }

    async fn org_list(&self, who: &str, status: &str) -> (u16, Value) {
        self.call(
            who,
            Method::GET,
            &format!("/v1/orgs/{}/app-setup-requests?status={status}", self.org),
            None,
        )
        .await
    }

    async fn my_list(&self, who: &str, status: &str) -> Value {
        self.feed(
            who,
            &format!("/v1/workspaces/{}/requests?status={status}", self.ws),
        )
        .await
    }

    /// A connector of type `mattermost` active on the instance (no organization connected it yet).
    async fn instance_connector(&self) {
        sqlx::query("INSERT INTO connectors (id, token_hash, display_name, manifest, status) VALUES ('mattermost', $1, 'Mattermost', '{}', 'active')")
            .bind(vec![9u8; 32])
            .execute(self.h.pool())
            .await
            .unwrap();
    }

    async fn org_connects(&self) {
        sqlx::query("INSERT INTO channel_integrations (id, org_id, connector_id, tenant_ref, tenant_label, config, credentials_ref, status) VALUES ($1,$2,'mattermost','T1','T1','{}','x','active')")
            .bind(Uuid::now_v7())
            .bind(self.org)
            .execute(self.h.pool())
            .await
            .unwrap();
    }
}

fn kinds(v: &Value) -> Vec<&str> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["kind"].as_str().unwrap())
        .collect()
}

fn of_kind<'a>(v: &'a Value, kind: &str) -> Vec<&'a Value> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["kind"] == kind)
        .collect()
}

/// Acceptance 1, 2 and 5: each request appears at exactly one level, moves when the facts change, and
/// leaves both when it resolves; the counts equal each feed's `count`.
#[tokio::test]
async fn a_request_is_counted_at_exactly_one_level() {
    let w = world().await;
    // Carol (the member) asks for a chat platform: no connector on the instance, so only an instance
    // admin can resolve it. Wendy asks for `lin` (an organization-level sign-in app).
    let (s, chat) = w
        .ask_req(&w.member, json!({"subject": "chat_platform"}))
        .await;
    assert_eq!(s, 200, "{chat}");
    assert_eq!(chat["level"], "administration");
    let (s, lin) = w.ask_app(&w.ws_owner, "lin", None).await;
    assert_eq!(s, 200, "{lin}");
    assert_eq!(lin["level"], "organization");

    let org = w.feed(&w.org_admin, &w.org_feed("open")).await;
    assert_eq!(kinds(&org), ["app_request"], "{org}");
    assert_eq!(org["count"], 1);
    let app = &org["items"][0];
    assert_eq!(app["title"], "Wendy Owner wants Linear Like");
    assert_eq!(
        app["detail"],
        "Needs a sign-in app for Linear Like that only an organization admin can register."
    );
    assert_eq!(app["action"]["label"], "Set up");
    assert_eq!(
        (app["can_decline"].as_bool(), app["own"].as_bool()),
        (Some(true), Some(false))
    );
    assert_eq!(app["id"], lin["id"]);
    assert_eq!(app["topic"], "apps");

    let admin = w.feed(&w.root, "/v1/admin/attention").await;
    let chats = of_kind(&admin, "chat_platform_request");
    assert_eq!(chats.len(), 1, "{admin}");
    assert!(of_kind(&admin, "app_request").is_empty(), "{admin}");
    assert_eq!(chats[0]["title"], "Mia Member wants a chat platform");
    assert_eq!(
        chats[0]["detail"],
        "Your organization has no chat platform connected."
    );
    assert_eq!(chats[0]["action"]["label"], "Connect a chat platform");
    assert_eq!(chats[0]["level"], "administration");
    let admin_count = admin["count"].as_i64().unwrap();

    // The counts are the feeds' counts; a scope the caller cannot act on is null.
    let (s, c) = w
        .call(&w.org_admin, Method::GET, &w.counts_uri(), None)
        .await;
    assert_eq!(s, 200, "{c}");
    assert_eq!(c["organization"]["count"], 1);
    assert_eq!(c["organization"]["topics"], json!({"apps": 1}));
    assert_eq!(c["administration"], Value::Null);
    let (_, c) = w
        .call(&w.root, Method::GET, "/v1/attention/counts", None)
        .await;
    assert_eq!(c["administration"]["count"].as_i64(), Some(admin_count));
    assert_eq!(c["administration"]["topics"]["chat-platforms"], 1);
    assert_eq!(
        (&c["organization"], &c["workspace"]),
        (&Value::Null, &Value::Null)
    );
    let (_, c) = w.call(&w.member, Method::GET, &w.counts_uri(), None).await;
    assert_eq!(c["administration"], Value::Null);
    assert_eq!(c["organization"], Value::Null);
    let ws = w.feed(&w.member, &w.ws_feed("open")).await;
    assert_eq!(c["workspace"]["count"], ws["count"]);

    // A connector appears on the instance: the request is now the organization's, still once.
    w.instance_connector().await;
    let org = w.feed(&w.org_admin, &w.org_feed("open")).await;
    assert_eq!(org["count"], 2, "{org}");
    assert_eq!(of_kind(&org, "chat_platform_request").len(), 1);
    let admin = w.feed(&w.root, "/v1/admin/attention").await;
    assert!(
        of_kind(&admin, "chat_platform_request").is_empty(),
        "{admin}"
    );

    // The organization connects one: the request resolves and leaves both feeds.
    w.org_connects().await;
    reevaluate(&w.h.state, Some(w.org)).await;
    let org = w.feed(&w.org_admin, &w.org_feed("open")).await;
    assert_eq!(kinds(&org), ["app_request"], "{org}");
    let done = w.feed(&w.org_admin, &w.org_feed("done")).await;
    let resolved = of_kind(&done, "chat_platform_request");
    assert_eq!(resolved.len(), 1, "{done}");
    assert_eq!(resolved[0]["state"], "done");
    assert_eq!(resolved[0]["closed_by_role"], "system");
    assert_eq!(
        resolved[0]["title"], "Mia Member wants a chat platform",
        "the asker who was told"
    );
    assert_eq!(done["count"], 0, "done items are never counted");
}

/// SEC-135 amended: a reason is cleaned, bounded, replaced on a re-ask without a new notice or audit row,
/// readable by the organization's admins for every asker and by an asker for their own only.
#[tokio::test]
async fn a_reason_is_sanitized_bounded_replaced_and_private() {
    let w = world().await;
    let carl = {
        let c = named(&w.h, "carl@x.io", "Carl Cooper", false).await;
        w.h.member(w.org, c, "member", "active").await;
        w.h.bind_ws(w.ws, c, "owner").await;
        let (_, t) = w.h.good_session(c).await;
        cookie(&t)
    };
    let long = "x".repeat(301);
    let (s, v) = w
        .ask_app(&w.ws_owner, "lin", Some(json!({"reason": long})))
        .await;
    assert_eq!(
        (s, v["error"]["details"]["fields"][0]["path"].as_str()),
        (400, Some("/reason")),
        "{v}"
    );
    let (s, v) = w
        .ask_app(
            &w.ws_owner,
            "lin",
            Some(json!({"reason": "  \u{202e}So Daneel\u{200b} can\u{0007} file issues\n\n\n\nthanks "})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["askers"][0]["reason"],
        "So Daneel can file issues\n\nthanks"
    );
    w.h.wait_audit_rows("app_setup_request.ask", None, 1).await;
    let meta: Value = sqlx::query_scalar(
        "SELECT metadata FROM audit_log WHERE action = 'app_setup_request.ask' LIMIT 1",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(meta["has_reason"], true);
    assert!(
        !meta.to_string().contains("Daneel"),
        "the text is never audited"
    );
    // The admins' notice carries the reason in its detail only.
    let (title, detail): (String, String) = sqlx::query_as(
        "SELECT title, detail FROM app_setup_request_notices WHERE kind = 'asked' LIMIT 1",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert!(!title.contains("Daneel"), "{title}");
    assert!(
        detail.contains("They wrote: So Daneel can file issues thanks"),
        "{detail}"
    );

    // Carl asks too; Wendy re-asks with a new reason: replaced, no new notice, no new audit row.
    let (_, c) = w
        .ask_req(
            &carl,
            json!({"subject": "app", "app_id": "lin", "reason": "Carl's"}),
        )
        .await;
    assert_eq!(
        c["askers"].as_array().unwrap().len(),
        1,
        "an asker sees only their own ask"
    );
    assert_eq!(c["askers"][0]["reason"], "Carl's");
    w.h.wait_audit_rows("app_setup_request.ask", None, 2).await;
    let notices: i64 = sqlx::query_scalar("SELECT count(*) FROM app_setup_request_notices")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    let (s, v) = w
        .ask_app(&w.ws_owner, "lin", Some(json!({"reason": "Newer"})))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["askers"][0]["reason"], "Newer");
    // A re-ask without a reason keeps it.
    let (_, v) = w.ask_app(&w.ws_owner, "lin", None).await;
    assert_eq!(v["askers"][0]["reason"], "Newer");
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(w.h.audit_rows("app_setup_request.ask", None).await, 2);
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM app_setup_request_notices")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(after, notices);

    // The organization's admin reads every asker's reason; the feed shows the first asker's.
    let (_, list) = w.org_list(&w.org_admin, "open").await;
    let reasons: Vec<&str> = list["items"][0]["askers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["reason"].as_str().unwrap())
        .collect();
    assert_eq!(reasons, ["Newer", "Carl's"]);
    let org = w.feed(&w.org_admin, &w.org_feed("open")).await;
    assert_eq!(org["items"][0]["reason"], "Newer");
    assert_eq!(
        org["items"][0]["title"],
        "Wendy Owner and Carl Cooper want Linear Like"
    );
    // Carl's own list has his ask only.
    let mine = w.my_list(&carl, "open").await;
    assert_eq!(mine["items"][0]["askers"].as_array().unwrap().len(), 1);
    assert_eq!(mine["items"][0]["askers"][0]["reason"], "Carl's");
    // A plain member reads nothing of the organization's.
    assert_eq!(w.org_list(&w.member, "open").await.0, 403);
    let (s, _) = w
        .call(&w.member, Method::GET, &w.org_feed("open"), None)
        .await;
    assert_eq!(s, 403);
}

/// Acceptance 3 and 6: a decline carries a message and names the role, never the decliner; each level
/// declines only its own requests.
#[tokio::test]
async fn a_decline_carries_a_message_and_names_the_role() {
    let w = world().await;
    let (_, key) = w.ask_req(&w.member, json!({"subject": "ai_key"})).await;
    let (_, chat) = w
        .ask_req(&w.member, json!({"subject": "chat_platform"}))
        .await;
    let (key_id, chat_id) = (key["id"].as_str().unwrap(), chat["id"].as_str().unwrap());
    let org_decline = |id: &str| format!("/v1/orgs/{}/app-setup-requests/{id}/decline", w.org);
    let admin_decline = |id: &str| format!("/v1/admin/requests/{id}/decline");

    // Each level declines only its own: 404 across levels.
    let (s, _) = w
        .call(&w.org_admin, Method::POST, &org_decline(chat_id), None)
        .await;
    assert_eq!(s, 404);
    let (s, _) = w
        .call(&w.root, Method::POST, &admin_decline(key_id), None)
        .await;
    assert_eq!(s, 404);
    let (s, _) = w
        .call(&w.org_admin, Method::POST, &admin_decline(chat_id), None)
        .await;
    assert_eq!(s, 403, "Administration's route is for instance admins");
    let (s, v) = w
        .call(
            &w.org_admin,
            Method::POST,
            &org_decline(key_id),
            Some(json!({"message": "y".repeat(301)})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["details"]["fields"][0]["path"].as_str()),
        (400, Some("/message"))
    );

    let (s, d) = w
        .call(
            &w.org_admin,
            Method::POST,
            &org_decline(key_id),
            Some(json!({"message": "Not this quarter. See https://evil.example/x"})),
        )
        .await;
    assert_eq!(s, 200, "{d}");
    assert_eq!(
        (d["status"].as_str(), d["closed_by_role"].as_str()),
        (Some("declined"), Some("org_admin"))
    );
    assert_eq!(d["closed_by"]["display_name"], "Adam Admin");
    assert_eq!(d["message"], "Not this quarter. See https://evil.example/x");
    let (title, detail): (String, String) = sqlx::query_as(
        "SELECT title, detail FROM app_setup_request_notices WHERE kind = 'declined'",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(title, "An admin isn't setting up an AI key now");
    assert!(detail.contains("They wrote: Not this quarter."), "{detail}");
    assert!(
        !detail.contains("https://"),
        "links are defused for mail and push: {detail}"
    );
    assert!(!format!("{title}{detail}").contains("Adam"));
    w.h.wait_audit_rows("app_setup_request.decline", None, 1)
        .await;
    let meta: Value = sqlx::query_scalar(
        "SELECT metadata FROM audit_log WHERE action = 'app_setup_request.decline' LIMIT 1",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(
        (
            meta["has_message"].as_bool(),
            meta["closed_by_role"].as_str()
        ),
        (Some(true), Some("org_admin"))
    );

    // The instance admin declines the chat-platform request (no body: still valid).
    let (s, d) = w
        .call(&w.root, Method::POST, &admin_decline(chat_id), None)
        .await;
    assert_eq!(s, 200, "{d}");
    assert_eq!(d["closed_by_role"], "instance_admin");

    // The asker's history: the message and the role, never a name.
    let hist = w.my_list(&w.member, "history").await;
    assert_eq!(hist["items"].as_array().unwrap().len(), 2, "{hist}");
    assert!(!hist.to_string().contains("Adam") && !hist.to_string().contains("Root"));
    let k = hist["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["subject"] == "ai_key")
        .unwrap();
    assert_eq!(
        (
            k["status"].as_str(),
            k["closed_by_role"].as_str(),
            &k["closed_by"]
        ),
        (Some("declined"), Some("org_admin"), &Value::Null)
    );
    assert_eq!(k["message"], "Not this quarter. See https://evil.example/x");
    // The asker's workspace feed shows both as done, own, never counted, without a name.
    let done = w.feed(&w.member, &w.ws_feed("done")).await;
    let mine = of_kind(&done, "my_request");
    assert_eq!(mine.len(), 2, "{done}");
    assert!(
        mine.iter()
            .all(|i| i["own"] == true && i["state"] == "declined" && i["closed_by"].is_null())
    );
    assert!(
        mine.iter()
            .any(|i| i["title"] == "An admin isn't setting up an AI key now")
    );

    // The organization's history: both levels; the decliner's name only for its own owners' decline.
    let (_, h) = w.org_list(&w.org_admin, "history").await;
    let items = h["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{h}");
    let by = |subj: &str| items.iter().find(|i| i["subject"] == subj).unwrap()["closed_by"].clone();
    assert_eq!(by("ai_key")["display_name"], "Adam Admin");
    assert_eq!(
        by("chat_platform"),
        Value::Null,
        "an instance admin is not named to the organization"
    );
    // Administration's history and done feed: the chat-platform request, with the decliner.
    let al = w.feed(&w.root, "/v1/admin/requests?status=history").await;
    assert_eq!(al["items"].as_array().unwrap().len(), 1, "{al}");
    assert_eq!(al["items"][0]["closed_by"]["display_name"], "Root Admin");
    let ad = w.feed(&w.root, "/v1/admin/attention?state=done").await;
    assert_eq!(of_kind(&ad, "chat_platform_request").len(), 1, "{ad}");
    assert_eq!(ad["items"][0]["state"], "declined");
}

/// Acceptance 4: history shows for 30 days, then not; withdrawn requests are never listed.
#[tokio::test]
async fn history_is_kept_thirty_days_and_withdrawals_are_not_listed() {
    let w = world().await;
    let (_, key) = w.ask_req(&w.member, json!({"subject": "ai_key"})).await;
    let key_id = key["id"].as_str().unwrap().to_owned();
    w.call(
        &w.org_admin,
        Method::POST,
        &format!("/v1/orgs/{}/app-setup-requests/{key_id}/decline", w.org),
        None,
    )
    .await;
    // A withdrawn one.
    let (_, lin) = w.ask_app(&w.ws_owner, "lin", None).await;
    let (s, _) = w
        .call(
            &w.ws_owner,
            Method::DELETE,
            &format!(
                "/v1/workspaces/{}/requests/{}",
                w.ws,
                lin["id"].as_str().unwrap()
            ),
            None,
        )
        .await;
    assert_eq!(s, 204);
    let in_history = |v: &Value| v["items"].as_array().unwrap().len();
    assert_eq!(in_history(&w.org_list(&w.org_admin, "history").await.1), 1);
    assert_eq!(
        in_history(&w.feed(&w.org_admin, &w.org_feed("done")).await),
        1
    );
    assert_eq!(in_history(&w.my_list(&w.member, "history").await), 1);
    assert_eq!(
        in_history(&w.my_list(&w.ws_owner, "history").await),
        0,
        "withdrawn"
    );
    // 31 days later it is gone from every view (the purge is at 90).
    sqlx::query("UPDATE app_setup_requests SET closed_at = now() - interval '31 days'")
        .execute(w.h.pool())
        .await
        .unwrap();
    assert_eq!(in_history(&w.org_list(&w.org_admin, "history").await.1), 0);
    assert_eq!(
        in_history(&w.feed(&w.org_admin, &w.org_feed("done")).await),
        0
    );
    assert_eq!(in_history(&w.my_list(&w.member, "history").await), 0);
    assert_eq!(
        of_kind(&w.feed(&w.member, &w.ws_feed("done")).await, "my_request").len(),
        0
    );
    // A resolved one: the asker's history from their `resolved` notice.
    w.ask_req(&w.member, json!({"subject": "ai_key"})).await;
    sqlx::query("INSERT INTO secrets (id, org_id, name, kind, llm_provider, llm_auth_mode, llm_endpoint_id, host_pattern, injection, value_source, value_ref) VALUES ($1,$2,'k','llm','anthropic','api_key','anthropic','api.example.com','{}','managed','x')")
        .bind(Uuid::now_v7())
        .bind(w.org)
        .execute(w.h.pool())
        .await
        .unwrap();
    reevaluate(&w.h.state, Some(w.org)).await;
    let h = w.my_list(&w.member, "history").await;
    assert_eq!(
        (
            h["items"][0]["status"].as_str(),
            h["items"][0]["closed_by_role"].as_str()
        ),
        (Some("resolved"), Some("system")),
        "{h}"
    );
    let done = w.feed(&w.member, &w.ws_feed("done")).await;
    assert_eq!(
        of_kind(&done, "my_request")[0]["title"],
        "An AI key is ready"
    );
    // Bad states are 400.
    let (s, _) = w
        .call(&w.member, Method::GET, &w.ws_feed("closed"), None)
        .await;
    assert_eq!(s, 400);
    let (s, _) = w
        .call(
            &w.member,
            Method::GET,
            &format!("/v1/workspaces/{}/requests?status=nope", w.ws),
            None,
        )
        .await;
    assert_eq!(s, 400);
}

/// The workspace feed for a member and an owner: the member's own waiting request is shown and never
/// counted; a broken workspace account is the owner's to fix and "Ask a workspace owner" for the member.
#[tokio::test]
async fn the_workspace_feed_for_a_member_and_an_owner() {
    let w = world().await;
    w.ask_req(&w.member, json!({"subject": "ai_key"})).await;
    let ws = w.feed(&w.member, &w.ws_feed("open")).await;
    let mine = of_kind(&ws, "my_request");
    assert_eq!(mine.len(), 1, "{ws}");
    assert_eq!(
        (
            mine[0]["title"].as_str(),
            mine[0]["detail"].as_str(),
            mine[0]["own"].as_bool()
        ),
        (
            Some("You asked for an AI key"),
            Some("Waiting for an organization admin."),
            Some(true)
        )
    );
    assert_eq!(ws["count"], 0, "an own request is never counted");
    // The owner does not see the member's request as their own.
    let ows = w.feed(&w.ws_owner, &w.ws_feed("open")).await;
    assert!(of_kind(&ows, "my_request").is_empty());

    // A workspace token whose last call was refused.
    let secret = Uuid::now_v7();
    sqlx::query("INSERT INTO secrets (id, org_id, workspace_id, name, kind, host_pattern, injection, value_source, value_ref, last_error_at, last_error) VALUES ($1,$2,$3,'Weather token','generic','api.weather.example','{}','managed','x', now(), 'rejected')")
        .bind(secret)
        .bind(w.org)
        .bind(w.ws)
        .execute(w.h.pool())
        .await
        .unwrap();
    let ows = w.feed(&w.ws_owner, &w.ws_feed("open")).await;
    let b = of_kind(&ows, "broken_account");
    assert_eq!(b.len(), 1, "{ows}");
    assert_eq!(b[0]["title"], "Weather token stopped working");
    assert_eq!(b[0]["action"]["label"], "Fix");
    assert_eq!(b[0]["id"], format!("broken_account:{secret}"));
    assert_eq!(ows["count"], 1);
    let ws = w.feed(&w.member, &w.ws_feed("open")).await;
    let b = of_kind(&ws, "broken_account");
    assert_eq!(b[0]["action"], Value::Null);
    assert!(
        b[0]["detail"]
            .as_str()
            .unwrap()
            .ends_with("Ask a workspace owner."),
        "{ws}"
    );
    assert_eq!(ws["count"], 1);
    // The counts equal the feeds' counts, for each viewer.
    for who in [&w.member, &w.ws_owner, &w.org_admin] {
        let feed = w.feed(who, &w.ws_feed("open")).await;
        let (_, c) = w.call(who, Method::GET, &w.counts_uri(), None).await;
        assert_eq!(c["workspace"]["count"], feed["count"], "{c} {feed}");
        assert_eq!(c["workspace"]["topics"]["accounts"], 1);
    }
    // A call that worked since: nothing to do.
    sqlx::query("UPDATE secrets SET last_ok_at = now() + interval '1 second' WHERE id = $1")
        .bind(secret)
        .execute(w.h.pool())
        .await
        .unwrap();
    let ows = w.feed(&w.ws_owner, &w.ws_feed("open")).await;
    assert!(of_kind(&ows, "broken_account").is_empty());
    // Outsiders: 404 for the workspace, null counts.
    let stranger = {
        let u = named(&w.h, "out@x.io", "Out Sider", false).await;
        let (_, t) = w.h.good_session(u).await;
        cookie(&t)
    };
    let (s, _) = w
        .call(&stranger, Method::GET, &w.ws_feed("open"), None)
        .await;
    assert_eq!(s, 404);
    let (s, c) = w.call(&stranger, Method::GET, &w.counts_uri(), None).await;
    assert_eq!(s, 200);
    assert_eq!(
        c,
        json!({"administration": null, "organization": null, "workspace": null})
    );
}

// ---- the organization's library updates and broken accounts ------------------------------------

/// A library of `org` (`None` = the instance's).
async fn library(w: &W, org: Option<Uuid>, label: &str, status: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO skill_libraries (id, org_id, label, url, repo, status) VALUES ($1,$2,$3,$4,'git.example/x/y',$5)",
    )
    .bind(id)
    .bind(org)
    .bind(label)
    .bind(format!("https://git.example/x/{id}"))
    .bind(status)
    .execute(w.h.pool())
    .await
    .unwrap();
    id
}

/// A skill of `org` added from `lib` (`ws` = a workspace skill, else the organization's), with one
/// proposed update in `update_status`. Returns the update's id.
async fn library_update(
    w: &W,
    org: Uuid,
    ws: Option<Uuid>,
    lib: Uuid,
    name: &str,
    update_status: &str,
) -> Uuid {
    let skill = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO skills (id, org_id, workspace_id, name, description, body, enabled, created_by_kind,
            library_id, library_label, library_repo, library_path, library_commit_sha, library_blobs,
            library_content_sha256, library_pinned_at)
         VALUES ($1,$2,$3,$4,'d','b',true,'user',$5,'l','x/y',$4,repeat('a',40),'{}'::jsonb,'\\x00',now())",
    )
    .bind(skill)
    .bind(org)
    .bind(ws)
    .bind(name)
    .bind(lib)
    .execute(w.h.pool())
    .await
    .unwrap();
    let update = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO skill_library_updates (id, skill_id, library_id, kind, status, commit_sha, content_sha256, bytes)
         VALUES ($1,$2,$3,'removed',$4,'x','\\x00',1)",
    )
    .bind(update)
    .bind(skill)
    .bind(lib)
    .bind(update_status)
    .execute(w.h.pool())
    .await
    .unwrap();
    update
}

/// A token secret of `org` (`ws` = a workspace's) whose last call was refused (`broken`) or worked.
async fn token_secret(w: &W, org: Uuid, ws: Option<Uuid>, name: &str, broken: bool) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO secrets (id, org_id, workspace_id, name, kind, host_pattern, injection, value_source, value_ref,
            last_ok_at, last_error_at, last_error)
         VALUES ($1,$2,$3,$4,'generic','api.weather.example','{}','managed','x',
            CASE WHEN $5 THEN NULL ELSE now() END, CASE WHEN $5 THEN now() END, CASE WHEN $5 THEN 'rejected' END)",
    )
    .bind(id)
    .bind(org)
    .bind(ws)
    .bind(name)
    .bind(broken)
    .execute(w.h.pool())
    .await
    .unwrap();
    id
}

async fn connection(w: &W, org: Uuid, ws: Option<Uuid>, label: &str, status: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref)
         VALUES ($1,$2,$3,'ghlike','multi','1.0.0',$4,$5,'x')",
    )
    .bind(id)
    .bind(org)
    .bind(ws)
    .bind(label)
    .bind(status)
    .execute(w.h.pool())
    .await
    .unwrap();
    id
}

async fn org_names(w: &W) -> String {
    sqlx::query_scalar("UPDATE organizations SET name = 'Acme' WHERE id = $1 RETURNING slug")
        .bind(w.org)
        .fetch_one(w.h.pool())
        .await
        .unwrap()
}

impl W {
    /// The organization feed's count is the rail's, and the topics add up to it.
    async fn org_counts_match(&self, who: &str, topics: Value) {
        let feed = self.feed(who, &self.org_feed("open")).await;
        let (_, c) = self.call(who, Method::GET, &self.counts_uri(), None).await;
        assert_eq!(c["organization"]["count"], feed["count"], "{c} {feed}");
        assert_eq!(c["organization"]["topics"], topics, "{c}");
    }

    /// A viewer who may not act for the organization: no feed, no count.
    async fn sees_no_org_items(&self, who: &str) {
        let (s, v) = self
            .call(who, Method::GET, &self.org_feed("open"), None)
            .await;
        assert_eq!(s, 403, "{v}");
        assert!(v.get("items").is_none(), "{v}");
        let (_, c) = self.call(who, Method::GET, &self.counts_uri(), None).await;
        assert_eq!(c["organization"], Value::Null, "{c}");
    }
}

/// An organization owner or admin sees one `library_update` item per library with updates to
/// organization skills, and it is counted. A member (a workspace owner too) sees neither. Workspace
/// skills' updates, decided updates and paused libraries (nothing can be approved there) add nothing.
#[tokio::test]
async fn a_library_update_waits_for_who_may_decide_it() {
    let w = world().await;
    let slug = org_names(&w).await;
    let lib = library(&w, Some(w.org), "Anthropic skills", "active").await;
    let first = library_update(&w, w.org, None, lib, "pdf", "pending").await;
    // None of these is the organization's to decide now.
    library_update(&w, w.org, None, lib, "decided", "rejected").await;
    let theirs = library(&w, Some(w.org), "Team skills", "active").await;
    library_update(&w, w.org, Some(w.ws), theirs, "ws-only", "pending").await;
    let paused = library(&w, Some(w.org), "Paused skills", "paused").await;
    library_update(&w, w.org, None, paused, "held", "pending").await;

    let org = w.feed(&w.org_admin, &w.org_feed("open")).await;
    assert_eq!(kinds(&org), ["library_update"], "{org}");
    assert_eq!(org["count"], 1);
    let i = &org["items"][0];
    assert_eq!(i["id"], format!("library_update:{lib}"));
    assert_eq!(i["subject_id"], lib.to_string());
    assert_eq!(i["level"], "organization");
    assert_eq!(i["topic"], "skills");
    assert_eq!(i["title"], "Anthropic skills has an update to review");
    assert_eq!(i["detail"], "Shared with every workspace in Acme.");
    assert_eq!(i["href"], format!("/o/{slug}/skills"));
    assert_eq!(
        i["action"],
        json!({"label": "Review", "href": format!("/o/{slug}/skills")})
    );
    assert_eq!(
        (
            i["decision"].as_bool(),
            i["own"].as_bool(),
            i["can_decline"].as_bool(),
            i["state"].as_str()
        ),
        (Some(true), Some(false), Some(false), Some("open"))
    );
    w.org_counts_match(&w.org_admin, json!({"skills": 1})).await;
    w.sees_no_org_items(&w.member).await;
    w.sees_no_org_items(&w.ws_owner).await;
    // Neither of the other levels' feeds lists it.
    for uri in [w.ws_feed("open"), "/v1/admin/attention".to_owned()] {
        let who = if uri.contains("admin") {
            &w.root
        } else {
            &w.org_admin
        };
        let v = w.feed(who, &uri).await;
        assert!(of_kind(&v, "library_update").is_empty(), "{v}");
    }

    // A second skill with an update, in an instance library too: one item per library, with how many.
    library_update(&w, w.org, None, lib, "docx", "pending").await;
    let shared = library(&w, None, "House skills", "error").await;
    library_update(&w, w.org, None, shared, "house", "pending").await;
    let org = w.feed(&w.org_admin, &w.org_feed("open")).await;
    let titles: Vec<&str> = org["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        titles,
        [
            "Anthropic skills has 2 updates to review",
            "House skills has an update to review"
        ],
        "{org}"
    );
    w.org_counts_match(&w.org_admin, json!({"skills": 2})).await;
    // `done` lists no library updates; a decided one leaves the open list.
    let done = w.feed(&w.org_admin, &w.org_feed("done")).await;
    assert!(of_kind(&done, "library_update").is_empty(), "{done}");
    sqlx::query("UPDATE skill_library_updates SET status = 'applied', decided_at = now() WHERE id = $1 OR library_id = $2")
        .bind(first)
        .bind(lib)
        .execute(w.h.pool())
        .await
        .unwrap();
    let org = w.feed(&w.org_admin, &w.org_feed("open")).await;
    assert_eq!(org["count"], 1, "{org}");
}

/// Deciding a library update needs a signed-in session, so an API key (even an organization
/// admin's) is not told of one; with skill libraries switched off nobody can decide, so nobody is.
#[tokio::test]
async fn a_library_update_is_not_shown_to_who_cannot_decide_at_all() {
    let w = world().await;
    let lib = library(&w, Some(w.org), "Anthropic skills", "active").await;
    library_update(&w, w.org, None, lib, "pdf", "pending").await;
    token_secret(&w, w.org, None, "Weather token", true).await;
    let adam: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = 'adam@x.io'")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    let (_, key) = w.h.api_key(adam, w.org, None, false).await;
    let auth = bearer(&key);
    for uri in [w.org_feed("open"), w.counts_uri()] {
        let (s, _, v) =
            w.h.send(Method::GET, &uri, &[("authorization", auth.as_str())], None)
                .await;
        assert_eq!(s, 200, "{uri}: {v}");
        let count = if uri.contains("counts") {
            &v["organization"]["count"]
        } else {
            assert_eq!(kinds(&v), ["org_account_broken"], "{v}");
            &v["count"]
        };
        assert_eq!(count, 1, "{uri}: {v}");
    }

    let off = world_with(|c| c.skills_library.enabled = false).await;
    let lib = library(&off, Some(off.org), "Anthropic skills", "active").await;
    library_update(&off, off.org, None, lib, "pdf", "pending").await;
    token_secret(&off, off.org, None, "Weather token", true).await;
    let v = off.feed(&off.org_admin, &off.org_feed("open")).await;
    assert_eq!(kinds(&v), ["org_account_broken"], "{v}");
    off.org_counts_match(&off.org_admin, json!({"apps": 1}))
        .await;
}

/// An organization account that is failing or must sign in again is the owners' and admins' to fix:
/// listed and counted for them, never for a member. A workspace's account, a working one and a
/// switched-off one are not the organization's items.
#[tokio::test]
async fn a_broken_organization_account_waits_for_who_may_fix_it() {
    let w = world().await;
    let slug = org_names(&w).await;
    let secret = token_secret(&w, w.org, None, "Weather token", true).await;
    let conn = connection(&w, w.org, None, "GitHub", "reauth_required").await;
    token_secret(&w, w.org, Some(w.ws), "Team token", true).await;
    token_secret(&w, w.org, None, "Healthy token", false).await;
    connection(&w, w.org, Some(w.ws), "Team GitHub", "reauth_required").await;
    connection(&w, w.org, None, "Off GitHub", "disabled").await;
    connection(&w, w.org, None, "Good GitHub", "active").await;

    let org = w.feed(&w.org_admin, &w.org_feed("open")).await;
    let items = of_kind(&org, "org_account_broken");
    assert_eq!(
        (items.len(), kinds(&org).len(), org["count"].as_i64()),
        (2, 2, Some(2)),
        "{org}"
    );
    let by = |subject: Uuid| {
        items
            .iter()
            .find(|i| i["subject_id"] == subject.to_string())
            .unwrap_or_else(|| panic!("{subject}: {org}"))
    };
    let s = by(secret);
    assert_eq!(s["id"], format!("org_account_broken:{secret}"));
    assert_eq!(
        (s["level"].as_str(), s["topic"].as_str()),
        (Some("organization"), Some("apps"))
    );
    assert_eq!(
        s["title"],
        "The Weather token account for Acme stopped working"
    );
    assert_eq!(
        s["detail"],
        "It no longer accepts the saved credentials. Workspaces that use it stop until it is fixed."
    );
    assert_eq!(
        s["action"],
        json!({"label": "Fix", "href": format!("/o/{slug}/apps?tab=accounts")})
    );
    assert_eq!(
        (s["decision"].as_bool(), s["can_decline"].as_bool()),
        (Some(false), Some(false))
    );
    let c = by(conn);
    assert_eq!(
        c["title"],
        "The GitHub account for Acme needs someone to sign in again"
    );
    assert_eq!(
        c["detail"],
        "The sign-in expired. Workspaces that use it stop until it is renewed."
    );
    assert_eq!(
        c["action"],
        json!({"label": "Sign in again",
            "href": format!("/o/{slug}/apps?tab=accounts&reconnect={conn}")})
    );
    w.org_counts_match(&w.org_admin, json!({"apps": 2})).await;
    w.sees_no_org_items(&w.member).await;
    w.sees_no_org_items(&w.ws_owner).await;

    // A call that worked since: nothing to do.
    sqlx::query("UPDATE secrets SET last_ok_at = now() + interval '1 second' WHERE id = $1")
        .bind(secret)
        .execute(w.h.pool())
        .await
        .unwrap();
    let org = w.feed(&w.org_admin, &w.org_feed("open")).await;
    assert_eq!(org["count"], 1, "{org}");
}

/// Another organization's library updates and broken accounts never appear: not its own library's,
/// and not its skills' updates from a library the whole instance shares.
#[tokio::test]
async fn another_organizations_items_never_appear() {
    let w = world().await;
    let other = w.h.org().await;
    let other_ws = w.h.workspace(other).await;
    let theirs = library(&w, Some(other), "Their skills", "active").await;
    library_update(&w, other, None, theirs, "theirs", "pending").await;
    let shared = library(&w, None, "House skills", "active").await;
    library_update(&w, other, None, shared, "house", "pending").await;
    token_secret(&w, other, None, "Their token", true).await;
    connection(&w, other, None, "Their GitHub", "reauth_required").await;
    token_secret(&w, other, Some(other_ws), "Their team token", true).await;

    let org = w.feed(&w.org_admin, &w.org_feed("open")).await;
    assert_eq!(
        (kinds(&org).len(), org["count"].as_i64()),
        (0, Some(0)),
        "{org}"
    );
    w.org_counts_match(&w.org_admin, json!({})).await;
    // This organization's admin cannot ask about the other one: 404, and a null count.
    let (s, v) = w
        .call(
            &w.org_admin,
            Method::GET,
            &format!("/v1/orgs/{other}/attention?state=open"),
            None,
        )
        .await;
    assert_eq!(s, 404, "{v}");
    let (_, c) = w
        .call(
            &w.org_admin,
            Method::GET,
            &format!("/v1/attention/counts?org={other}"),
            None,
        )
        .await;
    assert_eq!(c["organization"], Value::Null, "{c}");

    // The shared library's item counts this organization's skills only.
    library_update(&w, w.org, None, shared, "house-here", "pending").await;
    let org = w.feed(&w.org_admin, &w.org_feed("open")).await;
    assert_eq!(kinds(&org), ["library_update"], "{org}");
    assert_eq!(
        org["items"][0]["title"],
        "House skills has an update to review"
    );
}
