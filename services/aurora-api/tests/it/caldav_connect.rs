//! (phase 5, P5-CALDAV, D-275, C-845, SEC-109) Connect of the `caldav` package (an `own_server`
//! app, the manifest as shipped): discovery (`/.well-known/caldav` → principal → calendar home)
//! through the guarded client against an in-process fake DAV server (wiremock; Radicale itself runs
//! in the hosted end-to-end), the pinned `url`/`hosts` metadata, the label and the fixed error codes.
//! The password — the canary — appears in no response, read endpoint, audit row or stored column,
//! and no refused address ever receives a request.
//!
//! A child module of `connect` (it reuses that file's world and helpers).
use super::*;
use aurora_testkit::wiremock::matchers::{header_exists, method, path};

const PW_CANARY: &str = "PWCANARY0xA1B2C3D4";

/// A second app with a static host claim, so `server_host_claimed` is tested against a package's
/// claim (the host is only ever reached by that app).
const CLAIMER: &str = r#"
  - about: { id: claimer, name: Claimer, description: "Holds a host.", category: productivity, api_docs: "https://claimed.example.test/docs" }
    credentials: [ { name: token, secret: true } ]
    connect:
      - { id: tok, kind: fields, label: Token, fields: [ { name: token, label: Token, type: secret, store: credentials, required: true } ] }
    hosts:
      - match: { host: claimed.example.test }
        inject: [ { bearer: "{credentials.token}" } ]
"#;

fn manifest() -> String {
    let mut m = include_str!("../../../../plugins/builtin/caldav/plugin.yaml")
        .replace("publisher: aurora", "publisher: acme");
    m.push_str(CLAIMER);
    m
}

fn multistatus(prop: &str, href: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:response><d:href>/x</d:href><d:propstat><d:prop>{prop}</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#,
        prop = match prop {
            "principal" => format!(
                "<d:current-user-principal><d:href>{href}</d:href></d:current-user-principal>"
            ),
            _ => format!("<c:calendar-home-set><d:href>{href}</d:href></c:calendar-home-set>"),
        }
    )
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
    /// A server that signs `alice` in: the well-known path redirects to `/dav/`, which names the
    /// principal, which names the home (`home` may be absolute: an account moved to a partition).
    async fn serve(&self, home: &str) {
        let dav = |p: &str| {
            Mock::given(method("PROPFIND"))
                .and(path(p.to_owned()))
                .and(header_exists("authorization"))
        };
        dav("/.well-known/caldav")
            .respond_with(
                ResponseTemplate::new(301)
                    .insert_header("location", "https://dav.example.test/dav/"),
            )
            .mount(&self.server)
            .await;
        dav("/dav/")
            .respond_with(
                ResponseTemplate::new(207)
                    .set_body_string(multistatus("principal", "/dav/principals/alice/")),
            )
            .mount(&self.server)
            .await;
        dav("/dav/principals/alice/")
            .respond_with(ResponseTemplate::new(207).set_body_string(multistatus("home", home)))
            .mount(&self.server)
            .await;
    }

    async fn connect(&self, url: &str, name: Option<&str>) -> (u16, Value) {
        let mut b = self.w.connect_body("app_password");
        let mut f = json!({"url": url, "username": "alice", "password": PW_CANARY});
        if let Some(n) = name {
            f["name"] = json!(n);
        }
        b["fields"] = f;
        let (s, _, v) = self
            .w
            .call(
                &self.w.owner,
                Method::POST,
                "/v1/apps/caldav/connect",
                Some(b),
            )
            .await;
        (s, v)
    }

    async fn requests(&self) -> usize {
        self.server.received_requests().await.unwrap().len()
    }

    /// The password is in no response, read endpoint, audit row or stored plaintext column.
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
            !seen.contains(PW_CANARY),
            "{what}: the password leaked: {seen}"
        );
    }

    /// The pinned origins as stored (the API's `Connection.metadata` carries strings only).
    async fn stored_hosts(&self) -> Value {
        sqlx::query_scalar("SELECT metadata->'hosts' FROM app_connections WHERE app_id = 'caldav'")
            .fetch_one(self.w.h.pool())
            .await
            .unwrap()
    }

    async fn rows(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM app_connections WHERE app_id = 'caldav'")
            .fetch_one(self.w.h.pool())
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn connect_discovers_the_home_and_pins_url_and_hosts() {
    let l = lane().await;
    l.serve("/dav/calendars/alice/").await;
    let (s, v) = l.connect("https://dav.example.test", None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["kind"], "connected");
    let c = &v["connection"];
    assert_eq!(
        c["metadata"]["url"],
        "https://dav.example.test/dav/calendars/alice/"
    );
    assert_eq!(l.stored_hosts().await, json!(["dav.example.test:443"]));
    // The label is `<username> at <host>`.
    assert_eq!(c["label"], "alice at dav.example.test");
    // Three requests, each with credentials (the fake answers 404 without `Authorization`, which would
    // fail discovery), and none carries the password anywhere but the header.
    let got = l.server.received_requests().await.unwrap();
    assert_eq!(got.len(), 3, "{got:?}");
    assert!(got.iter().all(|r| r.method.as_str() == "PROPFIND"));
    l.clean("connect", &v).await;
}

#[tokio::test]
async fn the_name_overrides_the_label() {
    let l = lane().await;
    l.serve("/dav/calendars/alice/").await;
    let (s, v) = l
        .connect("https://dav.example.test", Some("  Family  "))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["connection"]["label"], "Family");
}

#[tokio::test]
async fn an_account_moved_to_a_partition_host_pins_both_origins() {
    let l = lane().await;
    l.serve("https://p42-dav.example.test:8443/p42/calendars/alice/")
        .await;
    let (s, v) = l.connect("https://dav.example.test", None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        l.stored_hosts().await,
        json!(["dav.example.test:443", "p42-dav.example.test:8443"])
    );
    assert_eq!(
        v["connection"]["metadata"]["url"],
        "https://p42-dav.example.test:8443/p42/calendars/alice/"
    );
    l.clean("partition", &v).await;
}

#[tokio::test]
async fn a_wrong_password_is_caldav_unauthorized_and_stores_nothing() {
    let l = lane().await;
    Mock::given(method("PROPFIND"))
        .respond_with(ResponseTemplate::new(401).insert_header("www-authenticate", "Basic realm=x"))
        .mount(&l.server)
        .await;
    let (s, v) = l.connect("https://dav.example.test", None).await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["code"], "caldav_unauthorized", "{v}");
    assert!(!v.to_string().contains(PW_CANARY));
    assert_eq!(l.rows().await, 0);
    l.clean("401", &v).await;
}

#[tokio::test]
async fn http_is_refused_and_nothing_is_sent() {
    let l = lane().await;
    l.serve("/dav/calendars/alice/").await;
    let (s, v) = l.connect("http://dav.example.test", None).await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["code"], "caldav_not_https", "{v}");
    assert_eq!(l.requests().await, 0);
}

#[tokio::test]
async fn a_redirect_to_http_is_refused_before_it_is_followed() {
    let l = lane().await;
    Mock::given(method("PROPFIND"))
        .respond_with(
            ResponseTemplate::new(301).insert_header("location", "http://dav.example.test/dav/"),
        )
        .mount(&l.server)
        .await;
    let (s, v) = l.connect("https://dav.example.test", None).await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["code"], "caldav_not_https", "{v}");
    assert_eq!(l.requests().await, 1);
}

#[tokio::test]
async fn a_private_address_needs_allowed_private() {
    let l = lane().await;
    l.serve("/dav/calendars/alice/").await;
    let (s, v) = l.connect("https://10.1.2.3/dav/", None).await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["code"], "caldav_destination_refused", "{v}");
    assert_eq!(l.requests().await, 0);
    // A host that is not an IP literal goes to the guarded client (which resolves and refuses
    // private answers itself): the test override stands in for that.
}

#[tokio::test]
async fn a_host_a_package_claims_is_refused_before_any_request() {
    let l = lane().await;
    l.serve("/dav/calendars/alice/").await;
    let (s, v) = l.connect("https://claimed.example.test", None).await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["code"], "server_host_claimed", "{v}");
    assert_eq!(
        l.requests().await,
        0,
        "no request, so no credential, reached it"
    );
    assert!(!v.to_string().contains(PW_CANARY));
    assert_eq!(l.rows().await, 0);
}

#[tokio::test]
async fn aurora_addresses_are_refused_before_any_request() {
    let l = lane().await;
    l.serve("/dav/calendars/alice/").await;
    for url in [
        "https://anything.aurora.internal/",
        "https://aurora-api/dav/",
        "https://aurora-gateway:8080/",
    ] {
        let (s, v) = l.connect(url, None).await;
        assert_eq!(s, 422, "{url}: {v}");
        assert_eq!(v["error"]["code"], "server_host_claimed", "{url}: {v}");
    }
    assert_eq!(l.requests().await, 0);
}

#[tokio::test]
async fn a_redirect_into_a_claimed_host_stops_before_the_credential_goes_there() {
    let l = lane().await;
    Mock::given(method("PROPFIND"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", "https://claimed.example.test/dav/"),
        )
        .mount(&l.server)
        .await;
    let (s, v) = l.connect("https://dav.example.test", None).await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["code"], "server_host_claimed", "{v}");
    // Only the first hop was sent; the claimed host never got a request.
    assert_eq!(l.requests().await, 1);
    assert_eq!(l.rows().await, 0);
}

#[tokio::test]
async fn google_and_exchange_hosts_point_at_the_right_app() {
    let l = lane().await;
    for (url, code) in [
        (
            "https://apidata.googleusercontent.com/caldav/v2/",
            "caldav_needs_oauth",
        ),
        (
            "https://www.google.com/calendar/dav/a@gmail.com/events/",
            "caldav_needs_oauth",
        ),
        ("https://outlook.office365.com/", "caldav_unsupported"),
        ("https://outlook.live.com/", "caldav_unsupported"),
    ] {
        let (s, v) = l.connect(url, None).await;
        assert_eq!(s, 422, "{url}: {v}");
        assert_eq!(v["error"]["code"], code, "{url}: {v}");
    }
    assert_eq!(l.requests().await, 0);
}

#[tokio::test]
async fn a_server_that_is_not_caldav_is_not_caldav() {
    let l = lane().await;
    Mock::given(method("PROPFIND"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&l.server)
        .await;
    let (s, v) = l.connect("https://dav.example.test", None).await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["code"], "caldav_not_caldav", "{v}");
    // The well-known path, then the entered address.
    assert_eq!(l.requests().await, 2);
}

#[tokio::test]
async fn an_unreachable_server_is_caldav_unreachable() {
    let l = lane().await;
    Mock::given(method("PROPFIND"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&l.server)
        .await;
    let (s, v) = l.connect("https://dav.example.test", None).await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["code"], "caldav_unreachable", "{v}");
}
