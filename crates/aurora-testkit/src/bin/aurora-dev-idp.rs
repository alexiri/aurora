//! `aurora-dev-idp` — the development OpenID Provider (phase 1, RFE-004; docs/04 §Development and
//! end-to-end sign-in). The in-process mock of `aurora_testkit::oidc` as a tiny binary: discovery,
//! JWKS, PKCE, nonce, a login page listing the fixture users and non-interactive sign-in with
//! `login_hint=<email>`. **Development and tests only**: never part of the Helm chart's default
//! values; the API refuses to register it unless `server.external_url` is a loopback origin.
//!
//! Environment:
//! - `AURORA_DEV_IDP_LISTEN`   listen address (default `127.0.0.1:4107`)
//! - `AURORA_DEV_IDP_ISSUER`   public issuer URL (default `http://localhost:<port><prefix>`)
//! - `AURORA_DEV_IDP_PREFIX`   URL path prefix, e.g. `/dev-idp` behind an ingress (default none)
//! - `AURORA_DEV_IDP_USERS`    JSON file `[{ "sub", "email", "name", "email_verified" }]`
//! - `AURORA_DEV_IDP_USERS_JSON`  the same list inline (used by the Helm chart; wins over the file)
//! - `AURORA_DEV_IDP_ALLOW_ANY_HINT=1`  also accept an unlisted `login_hint` (e2e harnesses)
#![forbid(unsafe_code)]

use aurora_testkit::oidc::{DevUser, MockIdp, Options};

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("aurora-dev-idp: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let bind = env("AURORA_DEV_IDP_LISTEN").unwrap_or_else(|| "127.0.0.1:4107".into());
    let prefix = env("AURORA_DEV_IDP_PREFIX").unwrap_or_default();
    let issuer = env("AURORA_DEV_IDP_ISSUER").unwrap_or_else(|| {
        let port = bind.rsplit(':').next().unwrap_or("4107");
        format!("http://localhost:{port}{prefix}")
    });
    let (raw, origin) = match (
        env("AURORA_DEV_IDP_USERS_JSON"),
        env("AURORA_DEV_IDP_USERS"),
    ) {
        (Some(json), _) => (json, "AURORA_DEV_IDP_USERS_JSON".to_owned()),
        (None, Some(path)) => (
            std::fs::read_to_string(&path).map_err(|e| format!("reading {path}: {e}"))?,
            path,
        ),
        (None, None) => return Err("AURORA_DEV_IDP_USERS (or _JSON) is required".into()),
    };
    let users: Vec<DevUser> =
        serde_json::from_str(&raw).map_err(|e| format!("parsing {origin}: {e}"))?;
    let allow_any_hint = env("AURORA_DEV_IDP_ALLOW_ANY_HINT").is_some_and(|v| v == "1");
    let idp = MockIdp::start_with(Options {
        bind: bind.clone(),
        issuer: Some(issuer.clone()),
        path_prefix: prefix,
        users: Some(users.clone()),
        allow_any_hint,
    })
    .await;
    eprintln!(
        "aurora-dev-idp: DEVELOPMENT ONLY. issuer {} listening on {bind} with {} user(s){}",
        idp.issuer(),
        users.len(),
        if allow_any_hint {
            ", any login_hint accepted"
        } else {
            ""
        },
    );
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|e| e.to_string())?;
    tokio::select! {
        r = tokio::signal::ctrl_c() => r.map_err(|e| e.to_string()),
        _ = term.recv() => Ok(()),
    }
}
