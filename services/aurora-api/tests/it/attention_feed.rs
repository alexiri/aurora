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
