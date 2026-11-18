//! (phase 5, P5-ICS, D-273, C-843, SEC-107) Connect of the `ics-calendar` package (a secret-link
//! app, the manifest as shipped): the URL is validated, fetched once through the guarded client,
//! checked against `expect_prefix` and stored as the credential, with its host as metadata. Every
//! refusal is one of the fixed codes, and the URL — its path and query above all (the canaries) —
//! appears in no response, no read endpoint and no audit row.
//!
//! A child module of `connect` (it reuses that file's world and helpers).
use super::*;
use aurora_testkit::wiremock::matchers::{method, path};

/// In the URL's path and in its query: neither may appear anywhere after connect.
const PATH_CANARY: &str = "PATHCANARY0xA1B2C3";
const QUERY_CANARY: &str = "QUERYCANARY0xD4E5F6";
const ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n";

fn secret_path() -> String {
    format!("/ics/{PATH_CANARY}/basic.ics")
}

fn url_on(host: &str) -> String {
    format!("https://{host}{}?key={QUERY_CANARY}", secret_path())
}

fn manifest() -> String {
    include_str!("../../../../plugins/builtin/ics-calendar/plugin.yaml")
        .replace("publisher: aurora", "publisher: acme")
}

struct L {
    w: W,
    server: MockServer,
}

async fn lane() -> L {
    let mut w = world_with(None, &manifest(), false).await;
    let server = MockServer::start().await;
    w.h.state = w.h.state.clone().with_connect_override(&server.uri());
    w.h.app = test_router(w.h.state.clone());
    L { w, server }
}

impl L {
    async fn serve(&self, status: u16, body: &str) {
        Mock::given(method("GET"))
            .and(path(secret_path()))
            .respond_with(ResponseTemplate::new(status).set_body_string(body.to_owned()))
            .mount(&self.server)
            .await;
    }

    async fn connect(&self, name: &str, url: &str) -> (u16, Value) {
        let mut b = self.w.connect_body("link");
        b["fields"] = json!({"name": name, "url": url});
        let (s, _, v) = self
            .w
            .call(
                &self.w.owner,
                Method::POST,
                "/v1/apps/ics-calendar/connect",
                Some(b),
            )
            .await;
        (s, v)
    }

    /// No canary in the response, in any read endpoint, nor in a stored audit row.
    async fn clean(&self, what: &str, v: &Value) {
        let mut seen = v.to_string();
        let (_, _, list) = self
            .w
            .call(
                &self.w.owner,
                Method::GET,
                &format!("/v1/workspaces/{}/connections", self.w.ws),
                None,
            )
            .await;
        seen.push_str(&list.to_string());
        let audit: Vec<String> = sqlx::query_scalar("SELECT metadata::text FROM audit_log")
            .fetch_all(self.w.h.pool())
            .await
            .unwrap();
        seen.push_str(&audit.join("\n"));
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT concat_ws(' ', label, metadata::text, tenant_host, credentials_ref) FROM app_connections",
        )
        .fetch_all(self.w.h.pool())
        .await
        .unwrap();
        seen.push_str(&rows.join("\n"));
        assert!(
            !seen.contains(PATH_CANARY) && !seen.contains(QUERY_CANARY),
            "{what}: the URL leaked: {seen}"
        );
    }
}

#[tokio::test]
async fn connect_stores_the_url_as_the_credential_and_its_host_as_metadata() {
    let l = lane().await;
    l.serve(200, ICS).await;
    let (s, v) = l.connect("Private calendar", &url_on("cal.example")).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["kind"], "connected");
    assert_eq!(v["connection"]["label"], "Private calendar");
    assert_eq!(v["connection"]["metadata"]["host"], "cal.example");
    // The API fetched exactly the URL once, with a calendar Accept.
    let got = l.server.received_requests().await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].url.path(), secret_path());
    assert_eq!(
        got[0].url.query(),
        Some(format!("key={QUERY_CANARY}").as_str())
    );
    // Sealed: nothing readable in the row, the responses or the audit trail.
    l.clean("connect", &v).await;
    let id = v["connection"]["id"].as_str().unwrap();
    let (s, _, one) =
        l.w.call(
            &l.w.owner,
            Method::GET,
            &format!("/v1/connections/{id}"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{one}");
    l.clean("read", &one).await;
}

#[tokio::test]
async fn webcal_is_normalized_and_the_name_is_the_label() {
    let l = lane().await;
    l.serve(200, ICS).await;
    let webcal = url_on("cal.example").replace("https://", "webcal://");
    let (s, v) = l.connect("  Work  ", &webcal).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["connection"]["label"], "Work");
    l.clean("webcal", &v).await;
}

#[tokio::test]
async fn every_refusal_is_a_fixed_code_that_never_echoes_the_url() {
    let l = lane().await;
    let u = url_on("cal.example");
    let long = format!("https://cal.example/{}/{PATH_CANARY}", "a".repeat(2_048));
    let cases: Vec<(String, &str)> = vec![
        (u.replace("https://", "http://"), "link_not_https"),
        (format!("ftp://cal.example/{PATH_CANARY}"), "link_not_https"),
        (format!("not a url {PATH_CANARY}"), "link_invalid"),
        (long, "link_invalid"),
        (
            format!("https://user:pw@cal.example/{PATH_CANARY}"),
            "link_invalid",
        ),
        (
            format!("https://10.1.2.3/{PATH_CANARY}?key={QUERY_CANARY}"),
            "link_destination_refused",
        ),
    ];
    for (url, code) in &cases {
        let (s, v) = l.connect("c", url).await;
        assert_eq!((s, v["error"]["code"].as_str()), (422, Some(*code)), "{v}");
        l.clean(code, &v).await;
    }
    assert!(
        l.server.received_requests().await.unwrap().is_empty(),
        "a refused link is never fetched"
    );
    // A private address is accepted where `gateway.allowed_private` covers it.
    l.w.h
        .state
        .connect
        .set_allowed_private(&["10.0.0.0/8".to_owned()]);
    l.serve(200, ICS).await;
    let (s, v) = l.connect("lan", &url_on("10.1.2.3")).await;
    assert_eq!(s, 200, "{v}");
    l.clean("allowed_private", &v).await;
}

#[tokio::test]
async fn the_servers_answer_decides_not_found_unreachable_and_unexpected_content() {
    for (status, body, code) in [
        (404, "", "link_not_found"),
        (410, "", "link_not_found"),
        (403, "", "link_not_found"),
        (500, "boom", "link_unreachable"),
        (200, "<html>sign in</html>", "link_unexpected_content"),
        (200, "", "link_unexpected_content"),
    ] {
        let l = lane().await;
        l.serve(status, body).await;
        let (s, v) = l.connect("c", &url_on("cal.example")).await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (422, Some(code)),
            "{status}: {v}"
        );
        l.clean(code, &v).await;
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM app_connections")
            .fetch_one(l.w.h.pool())
            .await
            .unwrap();
        assert_eq!(n, 0, "a refused link stores nothing");
    }
}

#[tokio::test]
async fn a_redirect_is_followed_over_https_only() {
    let l = lane().await;
    Mock::given(method("GET"))
        .and(path("/moved"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", secret_path().as_str()))
        .mount(&l.server)
        .await;
    l.serve(200, ICS).await;
    let (s, v) = l
        .connect(
            "c",
            &format!("https://cal.example/moved?key={QUERY_CANARY}"),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    l.clean("redirect", &v).await;

    let l = lane().await;
    Mock::given(method("GET"))
        .and(path("/to-http"))
        .respond_with(ResponseTemplate::new(302).insert_header(
            "location",
            format!("http://cal.example{}", secret_path()).as_str(),
        ))
        .mount(&l.server)
        .await;
    let (s, v) = l.connect("c", "https://cal.example/to-http").await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("link_not_https")),
        "{v}"
    );
}

#[tokio::test]
async fn reconnect_replaces_the_url() {
    let l = lane().await;
    l.serve(200, ICS).await;
    let (s, v) = l.connect("Private calendar", &url_on("cal.example")).await;
    assert_eq!(s, 200, "{v}");
    let id: Uuid = v["connection"]["id"].as_str().unwrap().parse().unwrap();
    let (before_ref, before_v): (String, i64) = sqlx::query_as(
        "SELECT credentials_ref, credentials_version FROM app_connections WHERE id = $1",
    )
    .bind(id)
    .fetch_one(l.w.h.pool())
    .await
    .unwrap();
    // The new link is at another host and path.
    Mock::given(method("GET"))
        .and(path("/new/NEWPATHCANARY.ics"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ICS))
        .mount(&l.server)
        .await;
    let mut b = l.w.connect_body("link");
    b["reconnect_connection_id"] = json!(id);
    b["fields"] =
        json!({"name": "Private calendar", "url": "https://other.example/new/NEWPATHCANARY.ics"});
    let (s, _, v) =
        l.w.call(
            &l.w.owner,
            Method::POST,
            "/v1/apps/ics-calendar/connect",
            Some(b),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["connection"]["id"], json!(id));
    assert_eq!(v["connection"]["metadata"]["host"], "other.example");
    let (after_ref, after_v): (String, i64) = sqlx::query_as(
        "SELECT credentials_ref, credentials_version FROM app_connections WHERE id = $1",
    )
    .bind(id)
    .fetch_one(l.w.h.pool())
    .await
    .unwrap();
    assert_ne!(
        before_ref, after_ref,
        "the sealed credentials were replaced"
    );
    assert!(after_v > before_v, "the credential version moved");
    assert!(!v.to_string().contains("NEWPATHCANARY"));
    l.clean("reconnect", &v).await;
}
