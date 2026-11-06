//! Fixture guest: implements all four hook points against `auth.hooks.example.test`.
use aurora_plugin_sdk::*;

struct Guest;

fn post(url: &str, body: &str) -> Result<HttpResponse, HookError> {
    fetch(&HttpRequest {
        method: "POST".into(),
        url: url.into(),
        headers: kv([("content-type", "application/json")]),
        body: Some(body.as_bytes().to_vec()),
    })
    .map_err(|e| match e {
        FetchError::Denied(m) => HookError::Upstream(format!("egress denied: {m}")),
        FetchError::Timeout => HookError::Upstream("timeout".into()),
        FetchError::TooLarge => HookError::Upstream("response too large".into()),
        FetchError::Network(m) => HookError::Upstream(m),
    })
}

impl Hooks for Guest {
    fn connect_authorize(i: AuthorizeInput) -> Result<AuthorizeOutput, HookError> {
        let account = kv_get(&i.fields, "account")
            .ok_or_else(|| HookError::InvalidInput("account is required".into()))?;
        let url = format!(
            "https://app.hooks.example.test/install?account={account}&state={}&redirect_uri={}",
            i.state, i.redirect_uri
        );
        Ok(AuthorizeOutput {
            url,
            continuation: Some(format!("account={account}")),
        })
    }

    fn connect_exchange(i: ExchangeInput) -> Result<ExchangeOutput, HookError> {
        let installation = match &i.source {
            ExchangeSource::OauthCallback(q) => kv_get(q, "installation_id")
                .ok_or_else(|| HookError::InvalidInput("missing installation_id".into()))?
                .to_owned(),
            ExchangeSource::Form(_) => return Err(HookError::Unsupported),
        };
        // First pass asks the user to continue to a second consent screen.
        if i.continuation.as_deref().is_some_and(|c| !c.contains("stage=2")) {
            return Ok(ExchangeOutput::Continue(ContinueWith {
                url: format!("https://app.hooks.example.test/consent?installation={installation}"),
                continuation: format!("stage=2;installation={installation}"),
            }));
        }
        let r = post(
            "https://auth.hooks.example.test/token",
            &format!("{{\"installation_id\":\"{installation}\"}}"),
        )?;
        if r.status != 200 {
            return Err(HookError::Upstream(format!("token endpoint {}", r.status)));
        }
        Ok(ExchangeOutput::Connected(Connected {
            credentials: kv([
                ("access_token", String::from_utf8_lossy(&r.body).as_ref()),
                ("installation_id", installation.as_str()),
            ]),
            metadata: kv([("account", "fixture")]),
            scopes: vec!["items".into()],
            label: format!("installation {installation}"),
            expires_at: Some(now_unix_seconds() + 3600),
            tenant_host: None,
        }))
    }

    fn token_mint(i: MintInput) -> Result<MintOutput, HookError> {
        let installation = kv_get(&i.credentials, "installation_id")
            .ok_or_else(|| HookError::ReauthRequired("installation missing".into()))?;
        let r = post(
            "https://auth.hooks.example.test/mint",
            &format!("{{\"installation_id\":\"{installation}\"}}"),
        )?;
        if r.status == 401 {
            return Err(HookError::ReauthRequired("installation revoked".into()));
        }
        Ok(MintOutput {
            credentials: kv([("access_token", String::from_utf8_lossy(&r.body).as_ref())]),
            expires_at: now_unix_seconds() + 3600,
        })
    }

    fn approval_summarize(i: SummarizeInput) -> Result<Summary, HookError> {
        // approval.summarize never has egress: this call must be denied by the host.
        let egress = match fetch(&HttpRequest {
            method: "GET".into(),
            url: "https://auth.hooks.example.test/probe".into(),
            headers: vec![],
            body: None,
        }) {
            Err(FetchError::Denied(_)) => "denied",
            _ => "allowed",
        };
        log(LogLevel::Info, "summarizing");
        Ok(Summary {
            title: "Create item".into(),
            fields: kv([("Egress", egress), ("Path", i.path.as_str())]),
            text: None,
            link: None,
            group: None,
        })
    }
}

export_hooks!(Guest);
