use aurora_plugin_sdk::*;

struct Guest;
impl Hooks for Guest {
    fn token_mint(i: MintInput) -> Result<MintOutput, HookError> {
        log(LogLevel::Info, "minting");
        let r = fetch(&HttpRequest {
            method: "POST".into(),
            url: "https://example.com/token".into(),
            headers: vec![],
            body: None,
        })
        .map_err(|_| HookError::Upstream("fetch failed".into()))?;
        let _ = (r, i);
        Ok(MintOutput { credentials: kv([("access_token", "t")]), expires_at: now_unix_seconds() + 3600 })
    }
}
export_hooks!(Guest);
