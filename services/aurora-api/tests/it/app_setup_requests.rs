#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-POLISH-4 (D-386, C-1030, SEC-135): a person asks the organization's owners and admins to make an app
//! ready. A child module of `app_setup` (it reuses that file's world: a workspace owner, a member, an
//! organization admin and an instance admin, and the `lin` app that needs an OAuth client).
use super::*;
use aurora_api::domains::app_setup_requests::service::{notify_once, reevaluate};

impl W {
    fn ask_uri(&self, app: &str) -> String {
        format!("/v1/workspaces/{}/apps/{app}/setup-request", self.ws)
    }

    async fn ask(&self, who: &str, app: &str) -> (u16, Value) {
        self.call(who, Method::POST, &self.ask_uri(app), None).await
    }

    async fn requests(&self, who: &str, status: &str) -> (u16, Value) {
        self.call(
            who,
            Method::GET,
            &format!("/v1/orgs/{}/app-setup-requests?status={status}", self.org),
            None,
        )
        .await
    }

    /// A second person who can own the workspace.
    async fn second_owner(&self) -> String {
        let c = named(&self.h, "carl@x.io", "Carl Cooper", false).await;
        self.h.member(self.org, c, "member", "active").await;
        self.h.bind_ws(self.ws, c, "owner").await;
        let (_, t) = self.h.good_session(c).await;
        cookie(&t)
    }

    async fn notices(&self, kind: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM app_setup_request_notices WHERE kind = $1")
            .bind(kind)
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }
}

fn asker_names(r: &Value) -> Vec<&str> {
    r["askers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["display_name"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn a_second_asker_joins_one_request_and_a_re_ask_changes_nothing() {
    let w = world().await;
    let carl = w.second_owner().await;
    let (s, a) = w.ask(&w.ws_owner, "lin").await;
    assert_eq!(s, 200, "{a}");
    assert_eq!(
        (a["status"].as_str(), a["app_name"].as_str()),
        (Some("open"), Some("Linear Like"))
    );
    assert_eq!(a["needs"], json!(["oauth_client"]));
    assert_eq!(asker_names(&a), ["Wendy Owner"]);
    // Idempotent per person: the same request, the same single ask.
    let (s, again) = w.ask(&w.ws_owner, "lin").await;
    assert_eq!(s, 200);
    assert_eq!(again["id"], a["id"]);
    assert_eq!(asker_names(&again), ["Wendy Owner"]);
    // A second person's ask joins it; the asker sees only their own ask.
    let (_, b) = w.ask(&carl, "lin").await;
    assert_eq!(b["id"], a["id"]);
    assert_eq!(
        asker_names(&b),
        ["Carl Cooper"],
        "an asker never sees the others: {b}"
    );
    // The organization's admin sees both, and exactly one open request exists.
    let (s, list) = w.requests(&w.org_admin, "open").await;
    assert_eq!(s, 200, "{list}");
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        asker_names(&list["items"][0]),
        ["Wendy Owner", "Carl Cooper"]
    );
    // The catalogue says only whether *I* asked, and how many did.
    let (_, cat) = w.catalogue(&w.ws_owner).await;
    assert_eq!(
        item(&cat, "lin")["readiness"]["requested"],
        json!({"by_me": true, "count": 2})
    );
    let (_, cat) = w.catalogue(&w.member).await;
    assert_eq!(
        item(&cat, "lin")["readiness"]["requested"],
        json!({"by_me": false, "count": 2})
    );
}

#[tokio::test]
async fn the_last_asker_to_withdraw_closes_the_request() {
    let w = world().await;
    let carl = w.second_owner().await;
    w.ask(&w.ws_owner, "lin").await;
    w.ask(&carl, "lin").await;
    let (s, _) = w
        .call(&w.ws_owner, Method::DELETE, &w.ask_uri("lin"), None)
        .await;
    assert_eq!(s, 204);
    let (_, list) = w.requests(&w.org_admin, "open").await;
    assert_eq!(
        asker_names(&list["items"][0]),
        ["Carl Cooper"],
        "the request stays for Carl"
    );
    let (s, _) = w.call(&carl, Method::DELETE, &w.ask_uri("lin"), None).await;
    assert_eq!(s, 204);
    let (_, open) = w.requests(&w.org_admin, "open").await;
    assert_eq!(open["items"], json!([]));
    let (_, closed) = w.requests(&w.org_admin, "withdrawn").await;
    assert_eq!(closed["items"].as_array().unwrap().len(), 1, "{closed}");
    // A withdrawal of nothing is quiet.
    let (s, _) = w
        .call(&w.ws_owner, Method::DELETE, &w.ask_uri("lin"), None)
        .await;
    assert_eq!(s, 204);
}

/// SEC-135: only an active workspace owner may ask; a ready app is a 409; a person who cannot see the
/// organization's requests gets a 404, whoever they are.
#[tokio::test]
async fn who_may_ask_and_who_may_read_or_decline() {
    let w = world().await;
    // A member without `can_own` cannot ask (the connect plan's check); nobody anonymous either.
    let (s, v) = w.ask(&w.member, "lin").await;
    assert_eq!(s, 403, "{v}");
    // A ready app is not asked for.
    let (s, v) = w.ask(&w.ws_owner, "ghlike").await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("app_ready")),
        "{v}"
    );
    let (s, _) = w.ask(&w.ws_owner, "nope").await;
    assert_eq!(s, 404);
    let (_, a) = w.ask(&w.ws_owner, "lin").await;
    let id = a["id"].as_str().unwrap().to_owned();
    // The workspace owner and a plain member read nothing and decline nothing: 403, the organization admin
    // routes' answer to a member who is not an owner or admin.
    for who in [&w.ws_owner, &w.member] {
        let (s, _) = w.requests(who, "open").await;
        assert_eq!(s, 403);
        let (s, _) = w
            .call(
                who,
                Method::POST,
                &format!("/v1/orgs/{}/app-setup-requests/{id}/decline", w.org),
                None,
            )
            .await;
        assert_eq!(s, 403);
    }
    // Another organization's owner reads and declines nothing here either.
    let other = w.h.org().await;
    let o = named(&w.h, "other-owner@x.io", "Olga Other", false).await;
    w.h.member(other, o, "owner", "active").await;
    let (_, ot) = w.h.good_session(o).await;
    let oc = cookie(&ot);
    let (s, _) = w.requests(&oc, "open").await;
    assert_eq!(s, 404);
    let (s, _) = w
        .call(
            &oc,
            Method::POST,
            &format!("/v1/orgs/{}/app-setup-requests/{id}/decline", w.org),
            None,
        )
        .await;
    assert_eq!(s, 404);
    // …and asks from a workspace of another organization are refused.
    let (s, _) = w.call(&oc, Method::POST, &w.ask_uri("lin"), None).await;
    assert!(matches!(s, 403 | 404), "{s}");
    // The request is untouched by all of it.
    let (_, list) = w.requests(&w.org_admin, "open").await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn an_admin_declines_once_and_each_asker_is_told_once() {
    let w = world().await;
    let carl = w.second_owner().await;
    w.ask(&w.ws_owner, "lin").await;
    let (_, b) = w.ask(&carl, "lin").await;
    let id = b["id"].as_str().unwrap();
    let uri = format!("/v1/orgs/{}/app-setup-requests/{id}/decline", w.org);
    let (s, d) = w.call(&w.org_admin, Method::POST, &uri, None).await;
    assert_eq!(s, 200, "{d}");
    assert_eq!(d["status"], "declined");
    let (s, v) = w.call(&w.org_admin, Method::POST, &uri, None).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("already_closed"))
    );
    assert_eq!(w.notices("declined").await, 2, "one per asker");
    // The asked admin's mail and the digest of the second ask (one per hour), and the askers' notices (one each),
    // go out once.
    assert_eq!(notify_once(&w.h.state).await.unwrap(), 4);
    let sent = w.h.mail.sent();
    let to: std::collections::BTreeSet<_> = sent.iter().map(|m| m.to.as_str()).collect();
    assert_eq!(
        to,
        ["adam@x.io", "carl@x.io", "wo@x.io"].into_iter().collect()
    );
    let declined: Vec<_> = sent
        .iter()
        .filter(|m| m.subject.contains("isn't setting up"))
        .collect();
    assert_eq!(declined.len(), 2);
    assert_eq!(
        declined[0].subject,
        "Adam Admin isn't setting up Linear Like now"
    );
    assert_eq!(
        notify_once(&w.h.state).await.unwrap(),
        0,
        "the claims stop a second send"
    );
    // A declined request can be asked again: a new open one.
    let (s, again) = w.ask(&w.ws_owner, "lin").await;
    assert_eq!(s, 200);
    assert_ne!(again["id"], b["id"]);
}

/// Registering the missing OAuth client resolves the request, tells each asker once, and the catalogue
/// reports the app ready.
#[tokio::test]
async fn registering_the_missing_client_resolves_the_request_and_tells_each_asker_once() {
    let w = world().await;
    let carl = w.second_owner().await;
    w.ask(&w.ws_owner, "lin").await;
    w.ask(&carl, "lin").await;
    assert_eq!(
        w.notices("asked").await,
        1,
        "one notice per owner and admin of the organization"
    );
    w.client(&w.root, "/v1/admin/oauth-clients", "linear").await;
    let (_, open) = w.requests(&w.org_admin, "open").await;
    assert_eq!(open["items"], json!([]), "{open}");
    let (_, done) = w.requests(&w.org_admin, "resolved").await;
    assert_eq!(done["items"].as_array().unwrap().len(), 1, "{done}");
    assert_eq!(w.notices("resolved").await, 2);
    let (_, cat) = w.catalogue(&w.ws_owner).await;
    assert_eq!(item(&cat, "lin")["readiness"], json!({"state": "ready"}));
    // Mail: Adam's request mail and the hour's digest of the second ask, and one "is ready" mail for each
    // asker; no address of an asker leaks.
    assert_eq!(notify_once(&w.h.state).await.unwrap(), 4);
    let sent = w.h.mail.sent();
    let ready: Vec<_> = sent
        .iter()
        .filter(|m| m.subject == "Linear Like is ready")
        .collect();
    assert_eq!(ready.len(), 2, "{sent:?}");
    let asked = sent.iter().find(|m| m.to == "adam@x.io").unwrap();
    assert!(
        asked
            .subject
            .starts_with("Wendy Owner wants Linear Like in "),
        "{}",
        asked.subject
    );
    for m in &sent {
        let all = format!("{}\n{}\n{}", m.subject, m.body, m.link);
        assert!(
            !all.contains("carl@x.io") && !all.contains("wo@x.io") && !all.contains("mem@x.io"),
            "{all}"
        );
    }
    assert!(
        asked.link.contains("/apps?tab=apps&app=lin"),
        "{}",
        asked.link
    );
    assert_eq!(notify_once(&w.h.state).await.unwrap(), 0);
    // A re-evaluation with nothing open does nothing, and the periodic sweep's entry point is safe to repeat.
    reevaluate(&w.h.state, None).await;
    assert_eq!(w.notices("resolved").await, 2);
}

/// An app the organization switched off is not visible to a workspace owner, so there is nothing to ask.
#[tokio::test]
async fn a_switched_off_app_cannot_be_asked_for_by_a_workspace_owner() {
    let w = world().await;
    sqlx::query("UPDATE organizations SET app_posture = 'restricted' WHERE id = $1")
        .bind(w.org)
        .execute(w.h.pool())
        .await
        .unwrap();
    // The admin may ask for a switched-off app; a workspace owner cannot see it.
    let (s, _) = w.ask(&w.ws_owner, "lin").await;
    assert_eq!(
        s, 404,
        "a switched-off app is not visible to a workspace owner"
    );
}

/// SEC-135: a request changes nothing it does not own, and its mail holds only the allowed fields.
#[tokio::test]
async fn asking_and_declining_change_no_availability_client_or_access() {
    let w = world().await;
    let count = |t: &'static str| {
        let h = &w.h;
        async move {
            sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {t}")))
                .fetch_one(h.pool())
                .await
                .unwrap()
        }
    };
    let before = (
        count("app_oauth_clients").await,
        count("org_app_availability").await,
        count("app_connections").await,
        count("grants").await,
    );
    let (_, a) = w.ask(&w.ws_owner, "lin").await;
    let id = a["id"].as_str().unwrap();
    w.call(
        &w.org_admin,
        Method::POST,
        &format!("/v1/orgs/{}/app-setup-requests/{id}/decline", w.org),
        None,
    )
    .await;
    let after = (
        count("app_oauth_clients").await,
        count("org_app_availability").await,
        count("app_connections").await,
        count("grants").await,
    );
    assert_eq!(before, after);
    let org: String = sqlx::query_scalar("SELECT org_id::text FROM app_setup_requests")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(org, w.org.to_string());
}

#[tokio::test]
async fn the_twenty_first_ask_in_an_hour_is_429() {
    let w = world_with(|c| c.ratelimit.api.multiplier = 1.0).await;
    for i in 0..20 {
        let (s, v) = w.ask(&w.ws_owner, "lin").await;
        assert_eq!(s, 200, "ask {i}: {v}");
    }
    let (s, v) = w.ask(&w.ws_owner, "lin").await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (429, Some("rate_limited")),
        "{v}"
    );
}
