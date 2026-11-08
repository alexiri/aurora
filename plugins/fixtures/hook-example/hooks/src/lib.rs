//! Example hook guest of a built-in package: `token.mint` (POST to the vendor's mint endpoint
//! with the stored installation id) and `approval.summarize`. The other hook points keep the
//! SDK's `unsupported` defaults, which is what `permissions.hooks` declares.
use aurora_plugin_sdk::*;

struct Guest;

impl Hooks for Guest {
    fn token_mint(i: MintInput) -> Result<MintOutput, HookError> {
        let installation = kv_get(&i.credentials, "installation_id")
            .ok_or_else(|| HookError::ReauthRequired("installation missing".into()))?;
        let r = fetch(&HttpRequest {
            method: "POST".into(),
            url: "https://auth.hook-example.example.test/mint".into(),
            headers: kv([("content-type", "application/json")]),
            body: Some(format!("{{\"installation_id\":\"{installation}\"}}").into_bytes()),
        })
        .map_err(|e| match e {
            FetchError::Denied(m) => HookError::Upstream(format!("egress denied: {m}")),
            FetchError::Timeout => HookError::Upstream("timeout".into()),
            FetchError::TooLarge => HookError::Upstream("response too large".into()),
            FetchError::Network(m) => HookError::Upstream(m),
        })?;
        match r.status {
            200 => Ok(MintOutput {
                credentials: kv([("access_token", String::from_utf8_lossy(&r.body).as_ref())]),
                expires_at: now_unix_seconds() + 3600,
            }),
            401 | 403 => Err(HookError::ReauthRequired("installation revoked".into())),
            s => Err(HookError::Upstream(format!("mint endpoint answered {s}"))),
        }
    }

    fn approval_summarize(i: SummarizeInput) -> Result<Summary, HookError> {
        Ok(Summary {
            title: "Create item".into(),
            fields: kv([("Method", i.method.as_str()), ("Path", i.path.as_str())]),
            text: None,
            link: None,
            group: None,
        })
    }
}

export_hooks!(Guest);
