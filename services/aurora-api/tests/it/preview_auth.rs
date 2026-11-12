#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Browser authorization of port previews (phase 4, P4-PRV-2, D-146/D-147, SEC-81/82/85;
//! docs/04 §Preview access, docs/18 §Port previews "Browser authorization"): the dashboard
//! `GET /v1/previews/authorize` grant exchange, the relay-plane `resolve`/`redeem`/`streams`/
//! `validate` surface (called in-process as the relay would), the uniform-404 matrix, the
//! replay/expiry/hostility properties of the sealed grant, the `__Host-aurora_preview`
//! session law (revocation cascades, API refusal, CSRF rule) and the
//! `preview.session.create` audit row.
//!
//! The relay logic itself (header hygiene, Fetch Metadata, framing) is covered by the relay
//! crate's own edge tests; here the API answers through the same router the relay talks to.

use crate::common::*;
use crate::hosted::{Rig, rig_with};
use aurora_core::token::{self, TokenPrefix};
use http::Method;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const DOMAIN: &str = "prv.example.org";

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

/// SHA-256 hex of a cookie token (what the relay sends as `session_hash`).
fn sess_hash(tok: &str) -> String {
    let mut h = Sha256::new();
    h.update(tok.as_bytes());
    h.finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
}

/// The rig with previews enabled on the instance and the org switch on, plus a registered
/// relay replica (`aul_` token upserted by hand, exactly what the bootstrap env does).
struct Prv {
    r: Rig,
    /// The relay anchor token (`aul_…`), accepted by the relay-plane extractors.
    relay_tok: String,
    replica: Uuid,
}

async fn prv_rig() -> Prv {
    let r = rig_with(|c| {
        c.previews.enabled = true;
        c.previews.domain = Some(DOMAIN.into());
    })
    .await;
    r.org_on().await;
    let tok = token::generate(TokenPrefix::Relay).expose().to_owned();
    sqlx::query("INSERT INTO relay_tokens (id, token_hash) VALUES ($1,$2)")
        .bind(Uuid::now_v7())
        .bind(&token::hash(&tok)[..])
        .execute(r.h.pool())
        .await
        .unwrap();
    // Register a replica through the real route (validates the whole `RelayToken` path).
    let (s, _, v) =
        r.h.send(
            Method::POST,
            "/v1/relay/register",
            &[("authorization", &bearer(&tok))],
            Some(json!({"version": "test/1"})),
        )
        .await;
    assert_eq!(s.as_u16(), 200, "{v}");
    let replica = v["replica_id"].as_str().unwrap().parse().unwrap();
    assert_eq!(v["config"]["previews_domain"], DOMAIN, "{v}");
    Prv {
        r,
        relay_tok: tok,
        replica,
    }
}

fn sha(u: Uuid) -> String {
    u.simple().to_string()
}

impl Prv {
    /// A relay-plane call: bearer `aul_` + `X-Aurora-Replica` + `X-Aurora-Protocol: relay/1`.
    async fn relay(&self, path: &str, body: Value) -> (u16, Value) {
        let extra = [
            ("authorization", bearer(&self.relay_tok)),
            ("x-aurora-replica", self.replica.to_string()),
            ("x-aurora-protocol", "relay/1".to_owned()),
        ];
        let hdrs: Vec<(&str, &str)> = extra.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let (s, _, v) = self.r.h.send(Method::POST, path, &hdrs, Some(body)).await;
        (s.as_u16(), v)
    }

    async fn resolve(&self, label: &str, hash: Option<&str>, nav: bool) -> (u16, Value) {
        self.relay(
            "/v1/relay/resolve",
            json!({"label": label, "session_hash": hash, "navigation": nav}),
        )
        .await
    }

    /// The rig's member creates a share and mints a live preview credential for it through
    /// the real authorize → redeem flow (a sealed grant produced in-process).
    async fn share_with_session(&self, port: i32) -> (Value, String, Uuid) {
        let r = &self.r;
        let agent = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness) \
             VALUES ($1,$2,$3,$1::text,$1::text,'hosted','fake')",
        )
        .bind(agent)
        .bind(r.org)
        .bind(r.ws)
        .execute(r.h.pool())
        .await
        .unwrap();
        let (s, v) = r
            .call(
                Method::POST,
                &format!("/v1/agents/{agent}/previews"),
                Some(json!({"port": port})),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        let label = v["label"].as_str().unwrap().to_owned();
        let (tok, session) = self.authorize_and_redeem(&label).await;
        (v, tok, session)
    }

    /// The full grant exchange of `authorize`: seal through the route, redeem through the
    /// relay plane. Returns the `auv_` preview credential and its dashboard session id.
    async fn authorize_and_redeem(&self, label: &str) -> (String, Uuid) {
        let nonce = Uuid::now_v7().simple().to_string();
        let (s, hd, _v) = self
            .r
            .h
            .send(
                Method::GET,
                &format!("/v1/previews/authorize?host={label}&state={nonce}"),
                &[("cookie", &self.r.cookie), ("origin", ORIGIN)],
                None,
            )
            .await;
        assert_eq!(s.as_u16(), 302, "authorize must redirect");
        let loc = hd["location"].to_str().unwrap().to_owned();
        assert!(
            loc.starts_with(&format!("https://{label}.{DOMAIN}/_aurora/grant?g=")),
            "{loc}"
        );
        let g = loc.rsplit("=").next().unwrap().to_owned();
        let (s, v) = self
            .relay(
                "/v1/relay/grants/redeem",
                json!({"grant": g, "label": label, "nonce": nonce}),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        // The preview session is bound to the dashboard session that authorized it: look
        // the row up by its token hash (the cookie carries `aurora_session=<token>`).
        let dash_tok = self
            .r
            .cookie
            .strip_prefix("aurora_session=")
            .unwrap_or(&self.r.cookie);
        let sid: Uuid = sqlx::query_scalar("SELECT id FROM sessions WHERE token_hash = $1")
            .bind(&token::hash(dash_tok)[..])
            .fetch_one(self.r.h.pool())
            .await
            .unwrap();
        (v["token"].as_str().unwrap().to_owned(), sid)
    }

    /// Mint a stream grant (`aux_`) for a share with a running sandbox.
    async fn streams_grant(&self, label: &str, tok: &str) -> (u16, Value) {
        self.relay(
            "/v1/relay/streams",
            json!({"label": label, "session_hash": sess_hash(tok)}),
        )
        .await
    }

    async fn org_off(&self) {
        sqlx::query(
            "UPDATE organizations SET settings = coalesce(settings,'{}'::jsonb) || '{\"previews_enabled\": false}'::jsonb WHERE id = $1",
        )
        .bind(self.r.org)
        .execute(self.r.h.pool())
        .await
        .unwrap();
    }

    async fn org_on(&self) {
        sqlx::query(
            "UPDATE organizations SET settings = coalesce(settings,'{}'::jsonb) || '{\"previews_enabled\": true}'::jsonb WHERE id = $1",
        )
        .bind(self.r.org)
        .execute(self.r.h.pool())
        .await
        .unwrap();
    }
}

// ---------------------------------------------------------------- the grant exchange

/// The full exchange: no cookie → 302 to sign-in; with one → 302 to
/// `https://<label>.<domain>/_aurora/grant?g=…`, whose redeem sets the preview cookie law.
#[tokio::test]
async fn authorize_flow_redirects_and_redeem_mints_the_session() {
    let p = prv_rig().await;
    let r = &p.r;
    let agent = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness) \
         VALUES ($1,$2,$3,$1::text,$1::text,'hosted','fake')",
    )
    .bind(agent)
    .bind(r.org)
    .bind(r.ws)
    .execute(r.h.pool())
    .await
    .unwrap();
    let (_, v) = r
        .call(
            Method::POST,
            &format!("/v1/agents/{agent}/previews"),
            Some(json!({"port": 3000})),
        )
        .await;
    let label = v["label"].as_str().unwrap().to_owned();

    // Step 1 without a session: 302 to the sign-in with a same-origin return_to.
    let (s, hd, _v) =
        r.h.send(
            Method::GET,
            &format!("/v1/previews/authorize?host={label}&state=n1"),
            &[],
            None,
        )
        .await;
    assert_eq!(s.as_u16(), 302);
    let loc = hd["location"].to_str().unwrap();
    assert!(loc.starts_with("/auth/login?return_to="), "{loc}");
    assert!(
        loc.contains("previews%2Fauthorize") || loc.contains("previews/authorize"),
        "{loc}"
    );

    // Step 2 with the member session: the sealed grant on the share's own host.
    let nonce = "nonce1234567890";
    let (s, hd, _v) =
        r.h.send(
            Method::GET,
            &format!("/v1/previews/authorize?host={label}&state={nonce}"),
            &[("cookie", &r.cookie), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!(s.as_u16(), 302);
    let loc = hd["location"].to_str().unwrap().to_owned();
    assert!(
        loc.starts_with(&format!("https://{label}.{DOMAIN}/_aurora/grant?g=")),
        "{loc}"
    );
    // The label in the redirect is the share's, never echoed request input.
    assert!(!loc.contains("EVIL"));

    // Step 3: the relay redeems the sealed grant; the `auv_` token comes back once.
    let g = loc.rsplit('=').next().unwrap();
    let (s, v) = p
        .relay(
            "/v1/relay/grants/redeem",
            json!({"grant": g, "label": label, "nonce": nonce}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let tok = v["token"].as_str().unwrap();
    assert!(tok.starts_with("auv_"), "{tok}");
    assert_eq!(v["idle_secs"], 3600);
    // The cookie the relay will set from this answer carries the full host law (checked in
    // the relay tests); here: the session resolves.
    let (s, v) = p.resolve(&label, Some(&sess_hash(tok)), true).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["decision"], "allow", "{v}");
    assert_eq!(v["share"]["port"], 3000, "{v}");
    // The relay-plane answer shape: no runner endpoint until a sandbox runs.
    assert_eq!(v["route"]["sandbox"], "unavailable", "{v}");

    // The audit row of the mint.
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'preview.session.create'",
    )
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 1, "one preview.session.create row");
}

/// Hostile `host`/`state` parameters: the redirect target is always constructed from the
/// share's label and the dashboard origin (docs/18 property), failures are uniform 404.
#[tokio::test]
async fn authorize_hostile_inputs_never_shape_the_redirect() {
    let p = prv_rig().await;
    let r = &p.r;
    let agent = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness) \
         VALUES ($1,$2,$3,$1::text,$1::text,'hosted','fake')",
    )
    .bind(agent)
    .bind(r.org)
    .bind(r.ws)
    .execute(r.h.pool())
    .await
    .unwrap();
    let (_, v) = r
        .call(
            Method::POST,
            &format!("/v1/agents/{agent}/previews"),
            Some(json!({"port": 3000})),
        )
        .await;
    let label = v["label"].as_str().unwrap().to_owned();
    let ok = |body: &str| {
        assert!(
            body.starts_with(&format!("https://{label}.{DOMAIN}/")),
            "redirect must be the share host: {body}"
        );
    };

    // Unknown label, malformed label, oversized nonce: uniform 404 (never a redirect).
    for (host, state) in [
        ("punknown00000000000000000", "n"),
        ("../etc-passwd", "n"),
        ("PABCDEFGHIJABCDEFGHIJ2345", "n"), // upper case never matches
        (label.as_str(), ""),               // empty nonce
        (label.as_str(), &"x".repeat(129)), // oversized nonce
    ] {
        let (s, _, v) =
            r.h.send(
                Method::GET,
                &format!("/v1/previews/authorize?host={host}&state={state}"),
                &[("cookie", &r.cookie), ("origin", ORIGIN)],
                None,
            )
            .await;
        assert_eq!(s.as_u16(), 404, "host={host} state={state}: {v}");
        assert_eq!(code(&v), "not_found", "{v}");
    }

    // A valid call with hostile-looking `state` still redirects to the share host only.
    let (s, hd, _v) =
        r.h.send(
            Method::GET,
            &format!("/v1/previews/authorize?host={label}&state=n&x=//evil.example"),
            &[("cookie", &r.cookie), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!(s.as_u16(), 302);
    ok(hd["location"].to_str().unwrap());
}

// ---------------------------------------------------------------- uniform 404 matrix

/// The resolve/redeem/streams matrix: every "no such preview or no access" cause answers the
/// uniform answer (`deny not_found` / 404), never a distinguishing error (SEC-85).
#[tokio::test]
async fn resolve_matrix_is_uniform_for_every_denial_cause() {
    let p = prv_rig().await;
    let r = &p.r;
    let (share, tok, _sid) = p.share_with_session(3000).await;
    let label = share["label"].as_str().unwrap().to_owned();

    // The baseline: a member resolves.
    let (s, v) = p.resolve(&label, Some(&sess_hash(&tok)), true).await;
    assert_eq!((s, v["decision"].as_str().unwrap()), (200, "allow"), "{v}");

    // Unknown label with a valid-looking hash: deny not_found (no oracle about labels).
    let (s, v) = p
        .resolve("punknown00000000000000000", Some(&sess_hash(&tok)), true)
        .await;
    assert_eq!((s, v["decision"].as_str().unwrap()), (200, "deny"), "{v}");
    assert_eq!(v["reason"], "not_found", "{v}");

    // A malformed session hash, a hash of a foreign token, no hash: `login` (flow restarts).
    let (_, v) = p.resolve(&label, Some("nothex"), true).await;
    assert_eq!(v["decision"], "login", "{v}");
    let (_, v) = p
        .resolve(&label, Some(&sess_hash("auv_neverminted")), true)
        .await;
    assert_eq!(v["decision"], "login", "{v}");
    let (_, v) = p.resolve(&label, None, true).await;
    assert_eq!(v["decision"], "login", "{v}");

    // No navigation (an XHR/subresource): still resolved by hash (the relay gates the rest).
    let (_, v) = p.resolve(&label, Some(&sess_hash(&tok)), false).await;
    assert_eq!(v["decision"], "allow", "{v}");

    // Malformed label: uniform 404 from the label validator itself (never a deny).
    let (s, v) = p.resolve("bad-label", Some(&sess_hash(&tok)), true).await;
    assert_eq!(s, 404, "{v}");

    // Another member of a different org with their own preview session of a different share:
    // the session belongs to the other share → `login` (never an access oracle).
    let other = r.h.user("other@x.io", false).await;
    r.h.member(r.org, other, "member", "active").await;
    r.h.bind_ws(r.ws, other, "member").await;
    let (_, ost) = r.h.good_session(other).await;
    let (s, hd, v) =
        r.h.send(
            Method::GET,
            &format!("/v1/previews/authorize?host={label}&state=n2"),
            &[("cookie", &cookie(&ost)), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!(s.as_u16(), 302, "{v}");
    let og = hd["location"]
        .to_str()
        .unwrap()
        .rsplit('=')
        .next()
        .unwrap()
        .to_owned();
    let (s, v) = p
        .relay(
            "/v1/relay/grants/redeem",
            json!({"grant": og, "label": label, "nonce": "n2"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let otok = v["token"].as_str().unwrap().to_owned();
    // Same share, different person: allowed by the law (both can `use` the workspace).
    let (_, v) = p.resolve(&label, Some(&sess_hash(&otok)), true).await;
    assert_eq!(v["decision"], "allow", "{v}");

    // A member who cannot use the workspace: their authorize is 404 (no grant at all).
    let ws2 = r.h.workspace(r.org).await;
    let outsider = r.h.user("out2@x.io", false).await;
    r.h.member(r.org, outsider, "member", "active").await;
    let (_, ost2) = r.h.good_session(outsider).await;
    let (s, _, v) =
        r.h.send(
            Method::GET,
            &format!("/v1/previews/authorize?host={label}&state=n3"),
            &[("cookie", &cookie(&ost2)), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!(s.as_u16(), 404, "{v}");
    let _ = ws2;
}

// ---------------------------------------------------------------- replay / expiry / tamper

/// A grant is single-use (replay 400), bound to its label and nonce, and dies with its seal.
#[tokio::test]
async fn grant_replay_wrong_label_nonce_and_tamper() {
    let p = prv_rig().await;
    let r = &p.r;
    let agent = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness) \
         VALUES ($1,$2,$3,$1::text,$1::text,'hosted','fake')",
    )
    .bind(agent)
    .bind(r.org)
    .bind(r.ws)
    .execute(r.h.pool())
    .await
    .unwrap();
    let (_, v) = r
        .call(
            Method::POST,
            &format!("/v1/agents/{agent}/previews"),
            Some(json!({"port": 3000})),
        )
        .await;
    let label = v["label"].as_str().unwrap().to_owned();
    let nonce = "nonce-replay-1";
    let (s, hd, _v) =
        r.h.send(
            Method::GET,
            &format!("/v1/previews/authorize?host={label}&state={nonce}"),
            &[("cookie", &r.cookie), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!(s.as_u16(), 302);
    let g = hd["location"]
        .to_str()
        .unwrap()
        .rsplit('=')
        .next()
        .unwrap()
        .to_owned();

    // First redeem: 200. Replay: 400 `grant_invalid` (the jti ledger).
    let (s, v) = p
        .relay(
            "/v1/relay/grants/redeem",
            json!({"grant": g, "label": label, "nonce": nonce}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = p
        .relay(
            "/v1/relay/grants/redeem",
            json!({"grant": g, "label": label, "nonce": nonce}),
        )
        .await;
    assert_eq!((s, code(&v)), (400, "grant_invalid"), "{v}");

    // A fresh grant redeemed with the wrong label or wrong nonce: 400.
    let nonce2 = "nonce-replay-2";
    let (s, hd, _v) =
        r.h.send(
            Method::GET,
            &format!("/v1/previews/authorize?host={label}&state={nonce2}"),
            &[("cookie", &r.cookie), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!(s.as_u16(), 302);
    let g2 = hd["location"]
        .to_str()
        .unwrap()
        .rsplit('=')
        .next()
        .unwrap()
        .to_owned();
    let (s, v) = p
        .relay(
            "/v1/relay/grants/redeem",
            json!({"grant": g2, "label": "pwrong0000000000000000000", "nonce": nonce2}),
        )
        .await;
    assert_eq!((s, code(&v)), (400, "grant_invalid"), "{v}");
    let (s, v) = p
        .relay(
            "/v1/relay/grants/redeem",
            json!({"grant": g2, "label": label, "nonce": "other"}),
        )
        .await;
    assert_eq!((s, code(&v)), (400, "grant_invalid"), "{v}");

    // A tampered seal (flipped byte in the base64 payload) and garbage: 400.
    let mut bytes = g2.clone().into_bytes();
    let mid = bytes.len() / 2;
    bytes[mid] = if bytes[mid] == b'A' { b'B' } else { b'A' };
    let tampered = String::from_utf8(bytes).unwrap();
    for bad in [tampered, "garbage".to_owned(), String::new()] {
        let (s, v) = p
            .relay(
                "/v1/relay/grants/redeem",
                json!({"grant": bad, "label": label, "nonce": nonce2}),
            )
            .await;
        assert_eq!((s, code(&v)), (400, "grant_invalid"), "{bad}: {v}");
    }
    // The wrong AAD (a grant sealed for another purpose) cannot open: garbage stays 400.
    // (Verified through the seal: a foreign-purpose seal never redeems — the redeem opens
    // only Purpose::PreviewGrant seals; a Purpose::OidcState string of the same length fails.)
    let foreign =
        r.h.state
            .derived
            .seal(aurora_crypto::Purpose::OidcState, b"other", b"{}")
            .unwrap();
    let (s, v) = p
        .relay(
            "/v1/relay/grants/redeem",
            json!({"grant": foreign, "label": label, "nonce": nonce2}),
        )
        .await;
    assert_eq!((s, code(&v)), (400, "grant_invalid"), "{v}");
    // g2 is still unused (the wrong-label attempts did not consume it): a correct redeem now
    // succeeds, proving the failed attempts never consumed the jti.
    let (s, v) = p
        .relay(
            "/v1/relay/grants/redeem",
            json!({"grant": g2, "label": label, "nonce": nonce2}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
}

/// An expired grant (exp in the past) is refused even though the seal verifies.
#[tokio::test]
async fn expired_grant_is_refused() {
    let p = prv_rig().await;
    let r = &p.r;
    let agent = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness) \
         VALUES ($1,$2,$3,$1::text,$1::text,'hosted','fake')",
    )
    .bind(agent)
    .bind(r.org)
    .bind(r.ws)
    .execute(r.h.pool())
    .await
    .unwrap();
    let (_, v) = r
        .call(
            Method::POST,
            &format!("/v1/agents/{agent}/previews"),
            Some(json!({"port": 3000})),
        )
        .await;
    let share_id: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let label = v["label"].as_str().unwrap().to_owned();
    // Seal a grant with exp in the past (the service's own struct, the rig's key).
    let g = aurora_api::domains::relay::service::PreviewGrant {
        jti: Uuid::now_v7(),
        share_id,
        user_id: Uuid::now_v7(),
        session_id: Uuid::now_v7(),
        nonce: "expired-nonce".into(),
        exp: chrono::Utc::now().timestamp() - 10,
    };
    let sealed =
        r.h.state
            .derived
            .seal(
                aurora_crypto::Purpose::PreviewGrant,
                b"preview-grant",
                &serde_json::to_vec(&g).unwrap(),
            )
            .unwrap();
    let (s, v) = p
        .relay(
            "/v1/relay/grants/redeem",
            json!({"grant": sealed, "label": label, "nonce": "expired-nonce"}),
        )
        .await;
    assert_eq!((s, code(&v)), (400, "grant_invalid"), "{v}");
}

/// The dashboard session dying between authorize and redeem refuses the mint.
#[tokio::test]
async fn dead_dashboard_session_cannot_redeem() {
    let p = prv_rig().await;
    let r = &p.r;
    let agent = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness) \
         VALUES ($1,$2,$3,$1::text,$1::text,'hosted','fake')",
    )
    .bind(agent)
    .bind(r.org)
    .bind(r.ws)
    .execute(r.h.pool())
    .await
    .unwrap();
    let (_, v) = r
        .call(
            Method::POST,
            &format!("/v1/agents/{agent}/previews"),
            Some(json!({"port": 3000})),
        )
        .await;
    let label = v["label"].as_str().unwrap().to_owned();
    let nonce = "n-dead";
    let (s, hd, v) =
        r.h.send(
            Method::GET,
            &format!("/v1/previews/authorize?host={label}&state={nonce}"),
            &[("cookie", &r.cookie), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!(s.as_u16(), 302, "{v}");
    let g = hd["location"]
        .to_str()
        .unwrap()
        .rsplit('=')
        .next()
        .unwrap()
        .to_owned();
    // Revoke the dashboard session (sign-out) before the redeem.
    let sid: Uuid = sqlx::query_scalar("SELECT id FROM sessions ORDER BY created_at DESC LIMIT 1")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE sessions SET revoked_at = now() WHERE id = $1")
        .bind(sid)
        .execute(r.h.pool())
        .await
        .unwrap();
    let (s, v) = p
        .relay(
            "/v1/relay/grants/redeem",
            json!({"grant": g, "label": label, "nonce": nonce}),
        )
        .await;
    assert_eq!((s, code(&v)), (400, "grant_invalid"), "{v}");
}

// ---------------------------------------------------------------- the auv_ session law

/// Revocation paths: sign-out sweep, expired preview session, lost `can_use`, paused share,
/// frozen agent, org switch — the next resolve denies/validates revoked and streams stop.
#[tokio::test]
async fn revocation_paths_reach_the_next_resolve() {
    let p = prv_rig().await;
    let r = &p.r;
    let (share, tok, sid) = p.share_with_session(3000).await;
    let label = share["label"].as_str().unwrap().to_owned();
    let (_, v) = p.resolve(&label, Some(&sess_hash(&tok)), true).await;
    assert_eq!(v["decision"], "allow", "{v}");

    // 1. Sign-out: deleting the preview sessions of the dashboard session.
    let n = aurora_db::repo::preview_relay::delete_preview_sessions_of_session(r.h.pool(), sid)
        .await
        .unwrap();
    assert_eq!(n, 1);
    let (_, v) = p.resolve(&label, Some(&sess_hash(&tok)), true).await;
    assert_eq!(v["decision"], "login", "{v}");

    // 2. Lost can_use (membership status suspended).
    let (share, tok, _sid) = p.share_with_session(3001).await;
    let label = share["label"].as_str().unwrap().to_owned();
    let uid: Uuid = sqlx::query_scalar(
        "SELECT u.id FROM users u JOIN preview_sessions ps ON ps.user_id = u.id LIMIT 1",
    )
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    sqlx::query("UPDATE org_members SET status = 'suspended' WHERE user_id = $1")
        .bind(uid)
        .execute(r.h.pool())
        .await
        .unwrap();
    // The preview session is still live, but its holder lost `can_use`: the uniform
    // `not_found` deny (docs/18 "next resolve denies"), never a `login` restart.
    let (_, v) = p.resolve(&label, Some(&sess_hash(&tok)), true).await;
    assert_eq!(v["decision"], "deny", "{v}");
    assert_eq!(v["reason"], "not_found", "{v}");
    sqlx::query("UPDATE org_members SET status = 'active' WHERE user_id = $1")
        .bind(uid)
        .execute(r.h.pool())
        .await
        .unwrap();

    // 3. Paused share: uniform not_found deny (a specific reason only the relay page shows).
    let share_id: Uuid = share["id"].as_str().unwrap().parse().unwrap();
    let (s, v) = r
        .call(
            Method::PATCH,
            &format!("/v1/previews/{share_id}"),
            Some(json!({"state": "paused"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (_, v) = p.resolve(&label, Some(&sess_hash(&tok)), true).await;
    assert_eq!(v["decision"], "deny", "{v}");
    assert_eq!(v["reason"], "not_found", "{v}");
    let (s, v) = r
        .call(
            Method::PATCH,
            &format!("/v1/previews/{share_id}"),
            Some(json!({"state": "active"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");

    // 4. Frozen agent: no access (folded into not_found on the relay plane).
    sqlx::query("UPDATE agents SET frozen_at = now() WHERE id = $1")
        .bind(
            sqlx::query_scalar::<_, Uuid>("SELECT agent_id FROM preview_shares WHERE id = $1")
                .bind(share_id)
                .fetch_one(r.h.pool())
                .await
                .unwrap(),
        )
        .execute(r.h.pool())
        .await
        .unwrap();
    let (_, v) = p.resolve(&label, Some(&sess_hash(&tok)), true).await;
    assert_eq!(v["decision"], "deny", "{v}");
    sqlx::query("UPDATE agents SET frozen_at = NULL WHERE id = $1")
        .bind(
            sqlx::query_scalar::<_, Uuid>("SELECT agent_id FROM preview_shares WHERE id = $1")
                .bind(share_id)
                .fetch_one(r.h.pool())
                .await
                .unwrap(),
        )
        .execute(r.h.pool())
        .await
        .unwrap();

    // 5. Org switch off: the same uniform deny; on again restores.
    p.org_off().await;
    let (_, v) = p.resolve(&label, Some(&sess_hash(&tok)), true).await;
    assert_eq!(v["decision"], "deny", "{v}");
    p.org_on().await;
    let (_, v) = p.resolve(&label, Some(&sess_hash(&tok)), true).await;
    assert_eq!(v["decision"], "allow", "{v}");

    // 6. validate() reports the revoked entries the relay must close (4403).
    let (s, v) = p
        .relay(
            "/v1/relay/validate",
            json!({"sessions": [{"label": label, "session_hash": sess_hash(&tok)},
                                 {"label": label, "session_hash": sess_hash("auv_gone")}]}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let revoked: Vec<&Value> = v["revoked"].as_array().unwrap().iter().collect();
    assert_eq!(revoked.len(), 1, "{v}");
    assert_eq!(
        revoked[0]["session_hash"],
        json!(sess_hash("auv_gone")),
        "{v}"
    );
}

/// The preview credential is accepted by no API route (it is not a Caller anywhere).
#[tokio::test]
async fn preview_credential_never_authenticates_api_routes() {
    let p = prv_rig().await;
    let (share, tok, _sid) = p.share_with_session(3000).await;
    let label = share["label"].as_str().unwrap().to_owned();
    // `auv_` is parsed as a known prefix but matches no credential table: 401 on every
    // authenticated surface (the share's own management routes and the admin relay plane).
    let b = bearer(&tok);
    let share_id = share["id"].as_str().unwrap();
    for uri in [
        format!("/v1/previews/{share_id}"),
        format!("/v1/agents/{}/previews", sha(Uuid::now_v7())),
        "/v1/admin/relays".to_owned(),
    ] {
        let (s, _, v) =
            p.r.h
                .send(Method::GET, &uri, &[("authorization", &b)], None)
                .await;
        assert_eq!(s.as_u16(), 401, "{uri}: {v}");
    }
    // And as a cookie (the relay sets `__Host-aurora_preview`; the API only knows
    // `aurora_session`): still 401 on the share's own route.
    let (s, _, v) =
        p.r.h
            .send(
                Method::GET,
                &format!("/v1/previews/{share_id}"),
                &[
                    ("cookie", &format!("__Host-aurora_preview={tok}")),
                    ("origin", ORIGIN),
                ],
                None,
            )
            .await;
    assert_eq!(s.as_u16(), 401, "{v}");
    let _ = label;
}

/// The CSRF rule (SEC-81): while previews are on, a same-site XHR with the session cookie is
/// 403 `csrf_rejected`; navigations pass; with previews off nothing changes.
#[tokio::test]
async fn csrf_same_site_xhr_refused_while_previews_on() {
    let p = prv_rig().await;
    let r = &p.r;
    // same-site cors XHR: refused.
    let (s, _, v) =
        r.h.send(
            Method::GET,
            "/v1/instance",
            &[
                ("cookie", &r.cookie),
                ("sec-fetch-site", "same-site"),
                ("sec-fetch-mode", "cors"),
                ("sec-fetch-dest", "empty"),
            ],
            None,
        )
        .await;
    assert_eq!((s.as_u16(), code(&v)), (403, "csrf_rejected"), "{v}");
    // cross-site too.
    let (s, _, v) =
        r.h.send(
            Method::GET,
            "/v1/instance",
            &[
                ("cookie", &r.cookie),
                ("sec-fetch-site", "cross-site"),
                ("sec-fetch-mode", "cors"),
                ("sec-fetch-dest", "empty"),
            ],
            None,
        )
        .await;
    assert_eq!((s.as_u16(), code(&v)), (403, "csrf_rejected"), "{v}");
    // A top-level navigation passes.
    let (s, _, v) =
        r.h.send(
            Method::GET,
            "/v1/instance",
            &[
                ("cookie", &r.cookie),
                ("sec-fetch-site", "same-site"),
                ("sec-fetch-mode", "navigate"),
                ("sec-fetch-dest", "document"),
            ],
            None,
        )
        .await;
    assert_eq!(s.as_u16(), 200, "{v}");
    // No Sec-Fetch headers (curl): not refused by this rule.
    let (s, _, v) =
        r.h.send(
            Method::GET,
            "/v1/instance",
            &[("cookie", &r.cookie), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!(s.as_u16(), 200, "{v}");
    // While previews are off, the same XHR passes again (rule scoped to previews).
    p.org_off().await;
    let r2 = rig_with(|c| {
        c.previews.enabled = false;
    })
    .await;
    let (s, _, v) =
        r2.h.send(
            Method::GET,
            "/v1/instance",
            &[
                ("cookie", &r2.cookie),
                ("sec-fetch-site", "same-site"),
                ("sec-fetch-mode", "cors"),
                ("sec-fetch-dest", "empty"),
            ],
            None,
        )
        .await;
    assert_eq!(s.as_u16(), 200, "{v}");
}

// ---------------------------------------------------------------- streams (aux_ grants)

/// Stream grants: minted only for a running sandbox with the port listening; single-use and
/// bound to (share, sandbox, boot, port) — the relay just carries them.
#[tokio::test]
async fn stream_grants_require_running_sandbox_and_are_single_use() {
    let p = prv_rig().await;
    let r = &p.r;
    let (share, tok, _sid) = p.share_with_session(3000).await;
    let label = share["label"].as_str().unwrap().to_owned();
    let share_id: Uuid = share["id"].as_str().unwrap().parse().unwrap();

    // No sandbox row: 409 not_running.
    let (s, v) = p.streams_grant(&label, &tok).await;
    assert_eq!((s, code(&v)), (409, "not_running"), "{v}");

    // A running sandbox (port not yet listening): the grant mints — `resolve` is what
    // reports `listening: false` (the runner refuses a connect to a closed port anyway).
    let runner: Uuid = sqlx::query_scalar("SELECT id FROM runners LIMIT 1")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    let sb = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO sandboxes (id, agent_id, runner_id, channel_replica_id, state, boot_id) \
         VALUES ($1,(SELECT agent_id FROM preview_shares WHERE id = $4),$2,$3,'running',$5)",
    )
    .bind(sb)
    .bind(runner)
    .bind(p.replica_dummy().await)
    .bind(share_id)
    .bind(Uuid::now_v7())
    .execute(r.h.pool())
    .await
    .unwrap();
    let (_, v) = p.resolve(&label, Some(&sess_hash(&tok)), true).await;
    assert_eq!(v["decision"], "allow", "{v}");
    assert_eq!(v["route"]["sandbox"], "running", "{v}");
    assert_eq!(v["route"]["listening"], false, "{v}");
    // No `stream_tls` capability (the in-cluster default, `16 §W18`): plain `ws://`.
    assert_eq!(
        v["route"]["runner_endpoint"], "ws://runner-a:4108/v1/preview-stream",
        "{v}"
    );

    // Listening: `resolve` says so and the grant mints, bound and single-use. The dial
    // scheme follows the runner's registered `stream_tls` capability.
    sqlx::query("UPDATE sandboxes SET listening_ports = ARRAY[3000] WHERE id = $1")
        .bind(sb)
        .execute(r.h.pool())
        .await
        .unwrap();
    let (s, v) = p.streams_grant(&label, &tok).await;
    assert_eq!(s, 200, "{v}");
    let grant = v["grant"].as_str().unwrap();
    assert!(grant.starts_with("aux_"), "{grant}");
    let endpoint = v["runner_endpoint"].as_str().unwrap();
    assert!(endpoint.starts_with("ws://runner-a:4108/"), "{endpoint}");
    assert_eq!(v["expires_in"], 30);
    sqlx::query(
        "UPDATE runners SET capabilities = jsonb_set(capabilities, '{stream_tls}', 'true')",
    )
    .execute(r.h.pool())
    .await
    .unwrap();
    let (_, v) = p.streams_grant(&label, &tok).await;
    assert!(
        v["runner_endpoint"]
            .as_str()
            .unwrap()
            .starts_with("wss://runner-a:4108/"),
        "{v}"
    );

    // The runner plane consumes it (any runner of the installation answers; the channel
    // check fenced the grant) and a second consume is 403.
    let (s, v) = r
        .runner(
            Some(r.rep),
            "/v1/runner/preview-streams/open",
            json!({"grant": grant}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["port"], 3000, "{v}");
    assert_eq!(v["share_id"], json!(share_id.to_string()), "{v}");
    assert!(v["sandbox_id"].is_string());
    assert!(v["boot_id"].is_string());
    assert!(v["window"].as_u64().unwrap() > 0);
    let (s, v) = r
        .runner(
            Some(r.rep),
            "/v1/runner/preview-streams/open",
            json!({"grant": grant}),
        )
        .await;
    assert_eq!((s, code(&v)), (403, "grant_invalid"), "{v}");

    // An invented grant: 403.
    let (s, v) = r
        .runner(
            Some(r.rep),
            "/v1/runner/preview-streams/open",
            json!({"grant": format!("aux_{}", Uuid::now_v7().simple())}),
        )
        .await;
    assert_eq!((s, code(&v)), (403, "grant_invalid"), "{v}");
}

impl Prv {
    /// A runner replica row for the channel FK (the rig's registered runner is the channel
    /// holder in practice; a real replica id keeps the insert honest).
    async fn replica_dummy(&self) -> Uuid {
        sqlx::query_scalar("SELECT id FROM runner_replicas LIMIT 1")
            .fetch_one(self.r.h.pool())
            .await
            .unwrap()
    }

    /// A relay-plane call with raw headers (for the auth matrix).
    async fn relay_raw(&self, headers: &[(&str, String)], body: Value) -> (u16, Value) {
        let hdrs: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let (s, _, v) = self
            .r
            .h
            .send(Method::POST, "/v1/relay/resolve", &hdrs, Some(body))
            .await;
        (s.as_u16(), v)
    }
}

/// Boot mismatch and pause after minting: the runner plane answers 409 `not_current`.
#[tokio::test]
async fn stream_grant_stale_boot_or_paused_share_is_not_current() {
    let p = prv_rig().await;
    let r = &p.r;
    let (share, tok, _sid) = p.share_with_session(3000).await;
    let label = share["label"].as_str().unwrap().to_owned();
    let share_id: Uuid = share["id"].as_str().unwrap().parse().unwrap();
    let runner: Uuid = sqlx::query_scalar("SELECT id FROM runners LIMIT 1")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    let sb = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO sandboxes (id, agent_id, runner_id, channel_replica_id, state, boot_id, listening_ports) \
         VALUES ($1,(SELECT agent_id FROM preview_shares WHERE id = $4),$2,$3,'running',$5,ARRAY[3000])",
    )
    .bind(sb)
    .bind(runner)
    .bind(p.replica_dummy().await)
    .bind(share_id)
    .bind(Uuid::now_v7())
    .execute(r.h.pool())
    .await
    .unwrap();
    let (s, v) = p.streams_grant(&label, &tok).await;
    assert_eq!(s, 200, "{v}");
    let grant = v["grant"].as_str().unwrap().to_owned();

    // New boot: the grant's boot is stale.
    sqlx::query("UPDATE sandboxes SET boot_id = $2 WHERE id = $1")
        .bind(sb)
        .bind(Uuid::now_v7())
        .execute(r.h.pool())
        .await
        .unwrap();
    let (s, v) = r
        .runner(
            Some(r.rep),
            "/v1/runner/preview-streams/open",
            json!({"grant": grant}),
        )
        .await;
    assert_eq!((s, code(&v)), (409, "not_current"), "{v}");

    // Fresh grant, then pause: not_current (the share must be active at redeem time).
    let (s, v) = p.streams_grant(&label, &tok).await;
    assert_eq!(s, 200, "{v}");
    let grant2 = v["grant"].as_str().unwrap().to_owned();
    sqlx::query("UPDATE preview_shares SET state = 'paused' WHERE id = $1")
        .bind(share_id)
        .execute(r.h.pool())
        .await
        .unwrap();
    let (s, v) = r
        .runner(
            Some(r.rep),
            "/v1/runner/preview-streams/open",
            json!({"grant": grant2}),
        )
        .await;
    assert_eq!((s, code(&v)), (409, "not_current"), "{v}");
}

/// The `closed` accounting call refreshes the share and the awake stamp, and never fails.
#[tokio::test]
async fn stream_closed_is_accounting_only() {
    let p = prv_rig().await;
    let r = &p.r;
    let (share, _tok, _sid) = p.share_with_session(3000).await;
    let share_id: Uuid = share["id"].as_str().unwrap().parse().unwrap();
    sqlx::query("UPDATE preview_shares SET last_accessed_at = NULL WHERE id = $1")
        .bind(share_id)
        .execute(r.h.pool())
        .await
        .unwrap();
    let (s, v) = r
        .runner(
            Some(r.rep),
            "/v1/runner/preview-streams/closed",
            json!({"share_id": share_id, "bytes_to_app": 12, "bytes_from_app": 34, "reason": "done"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["ok"], true, "{v}");
    let accessed: Option<Option<chrono::DateTime<chrono::Utc>>> =
        sqlx::query_scalar("SELECT last_accessed_at FROM preview_shares WHERE id = $1")
            .bind(share_id)
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert!(accessed.flatten().is_some(), "last_accessed_at refreshed");
    // An unknown share id: still ok (advisory).
    let (s, v) = r
        .runner(
            Some(r.rep),
            "/v1/runner/preview-streams/closed",
            json!({"share_id": Uuid::now_v7(), "reason": "gone"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
}

// ---------------------------------------------------------------- token hygiene

/// The relay plane itself: 401 without/with a wrong `aul_`, 426 on a newer protocol,
/// 403 `replica_unknown` without or with a foreign replica.
#[tokio::test]
async fn relay_plane_auth_is_uniform_and_protocol_checked() {
    let p = prv_rig().await;
    // No token / wrong token / wrong-prefix token: one identical 401 body.
    let repl = p.replica.to_string();
    for auth in [
        None,
        Some("Bearer aul_wrong"),
        Some("Bearer auv_also_wrong"),
    ] {
        let mut hdrs: Vec<(&str, String)> = vec![
            ("x-aurora-replica", repl.clone()),
            ("x-aurora-protocol", "relay/1".to_owned()),
        ];
        if let Some(a) = auth {
            hdrs.push(("authorization", a.to_owned()));
        }
        let (s, v) = p
            .relay_raw(&hdrs, json!({"label": "paaaaaaaaaaaaaaaaaaaaaaaa"}))
            .await;
        assert_eq!((s, code(&v)), (401, "unauthenticated"), "{auth:?}: {v}");
    }
    // A newer protocol version: 426 with the supported list.
    let hdrs = [
        ("authorization", bearer(&p.relay_tok)),
        ("x-aurora-replica", p.replica.to_string()),
        ("x-aurora-protocol", "relay/2".to_owned()),
    ];
    let (s, v) = p
        .relay_raw(&hdrs, json!({"label": "paaaaaaaaaaaaaaaaaaaaaaaa"}))
        .await;
    assert_eq!(s, 426, "{v}");
    assert_eq!(code(&v), "protocol_unsupported", "{v}");
    // A valid token but an unknown replica: 403 `replica_unknown`.
    let hdrs = [
        ("authorization", bearer(&p.relay_tok)),
        ("x-aurora-replica", Uuid::now_v7().to_string()),
        ("x-aurora-protocol", "relay/1".to_owned()),
    ];
    let (s, v) = p
        .relay_raw(&hdrs, json!({"label": "paaaaaaaaaaaaaaaaaaaaaaaa"}))
        .await;
    assert_eq!((s, code(&v)), (403, "replica_unknown"), "{v}");
}
