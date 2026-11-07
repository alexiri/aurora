//! Sample inputs for unit-testing hook logic on the host target.
use crate::*;

pub fn sample_authorize_input() -> AuthorizeInput {
    AuthorizeInput {
        app_id: "example".into(),
        method_id: "hook".into(),
        client: kv([("client_id", "cid")]),
        redirect_uri: "https://aurora.example/callback".into(),
        state: "state123".into(),
        scopes: vec!["read".into()],
        access_level: None,
        fields: vec![],
        context: vec![],
    }
}

pub fn sample_exchange_input() -> ExchangeInput {
    ExchangeInput {
        app_id: "example".into(),
        method_id: "hook".into(),
        source: ExchangeSource::OauthCallback(kv([("code", "abc"), ("state", "state123")])),
        client: kv([("client_id", "cid")]),
        client_secret: kv([("client_secret", "shh")]),
        redirect_uri: "https://aurora.example/callback".into(),
        pkce_verifier: Some("verifier".into()),
        continuation: None,
        context: vec![],
    }
}

pub fn sample_mint_input() -> MintInput {
    MintInput {
        app_id: "example".into(),
        credentials: kv([("refresh_token", "r")]),
        metadata: vec![],
        tenant_host: None,
        client: kv([("client_id", "cid")]),
        client_secret: kv([("client_secret", "shh")]),
        context: vec![],
    }
}

pub fn sample_summarize_input() -> SummarizeInput {
    SummarizeInput {
        app_id: "example".into(),
        tool_id: Some("close_task".into()),
        method: "POST".into(),
        host: "api.example.com".into(),
        path: "/tasks/123/close".into(),
        query: vec![],
        headers: vec![],
        body_prefix: vec![],
        truncated: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Defaults;
    impl Hooks for Defaults {}

    #[test]
    fn defaults_are_unsupported() {
        assert!(matches!(
            Defaults::connect_authorize(sample_authorize_input()),
            Err(HookError::Unsupported)
        ));
        assert!(matches!(
            Defaults::connect_exchange(sample_exchange_input()),
            Err(HookError::Unsupported)
        ));
        assert!(matches!(
            Defaults::token_mint(sample_mint_input()),
            Err(HookError::Unsupported)
        ));
        assert!(matches!(
            Defaults::approval_summarize(sample_summarize_input()),
            Err(HookError::Unsupported)
        ));
    }
}
