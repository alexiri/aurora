//! Test guest: behaviour is selected by the `mode` context entry.
use aurora_plugin_sdk::*;

struct G;

fn ctx<'a>(c: &'a [Kv], k: &str) -> &'a str {
    kv_get(c, k).unwrap_or("")
}

fn spin() -> ! {
    let mut i = 0u64;
    loop {
        i = std::hint::black_box(i.wrapping_add(1));
    }
}

fn bomb() -> usize {
    let mut keep: Vec<Vec<u8>> = Vec::new();
    for _ in 0..400 {
        keep.push(vec![1u8; 1024 * 1024]);
    }
    keep.len()
}

fn do_fetch(url: &str) -> Result<String, HookError> {
    match fetch(&HttpRequest { method: "GET".into(), url: url.into(), headers: vec![], body: None }) {
        Ok(r) => Ok(String::from_utf8_lossy(&r.body).into_owned()),
        Err(FetchError::Denied(m)) => Err(HookError::Upstream(format!("denied: {m}"))),
        Err(_) => Err(HookError::Upstream("fetch failed".into())),
    }
}

impl Hooks for G {
    fn connect_authorize(i: AuthorizeInput) -> Result<AuthorizeOutput, HookError> {
        match ctx(&i.context, "mode") {
            "loop" => spin(),
            "stdout" => {
                println!("hello from guest {}", i.state);
                Ok(AuthorizeOutput { url: "https://login.example.com/a".into(), continuation: None })
            }
            "badurl" => Ok(AuthorizeOutput { url: "http://login.example.com/a".into(), continuation: None }),
            _ => Ok(AuthorizeOutput {
                url: format!("https://login.example.com/a?state={}", i.state),
                continuation: Some("k".into()),
            }),
        }
    }

    fn connect_exchange(i: ExchangeInput) -> Result<ExchangeOutput, HookError> {
        let connected = |creds: Vec<Kv>, tenant: Option<&str>| {
            Ok(ExchangeOutput::Connected(Connected {
                credentials: creds,
                metadata: kv([("region", "eu")]),
                scopes: vec![],
                label: "acct".into(),
                expires_at: None,
                tenant_host: tenant.map(str::to_owned),
            }))
        };
        match ctx(&i.context, "mode") {
            "loop" => spin(),
            "alloc" => connected(kv([("token", bomb().to_string().as_str())]), None),
            "flood" => connected(kv([("token", "x".repeat(300_000).as_str())]), None),
            "bad_cred" => connected(kv([("nope", "1")]), None),
            "bad_tenant" => connected(kv([("token", "1")]), Some("evil.com")),
            "tenant" => connected(kv([("token", "1")]), Some("acme.example.com")),
            "continue" => Ok(ExchangeOutput::Continue(ContinueWith {
                url: "https://auth.example.com/next".into(),
                continuation: "c1".into(),
            })),
            "continue_bad" => Ok(ExchangeOutput::Continue(ContinueWith {
                url: "https://evil.com/next".into(),
                continuation: "c1".into(),
            })),
            "fetch" => {
                let body = do_fetch(ctx(&i.context, "url"))?;
                connected(kv([("token", body.as_str())]), None)
            }
            "log" => {
                for _ in 0..150 {
                    log(LogLevel::Info, "secret is s3cr3t-value");
                }
                connected(kv([("token", "1")]), None)
            }
            "reauth" => Err(HookError::ReauthRequired("expired".into())),
            _ => connected(kv([("token", "t1")]), None),
        }
    }

    fn token_mint(i: MintInput) -> Result<MintOutput, HookError> {
        match ctx(&i.context, "mode") {
            "fetch" => {
                let body = do_fetch(ctx(&i.context, "url"))?;
                Ok(MintOutput { credentials: kv([("token", body.as_str())]), expires_at: now_unix_seconds() + 60 })
            }
            _ => Ok(MintOutput { credentials: kv([("token", "minted")]), expires_at: now_unix_seconds() + 3600 }),
        }
    }

    fn approval_summarize(i: SummarizeInput) -> Result<Summary, HookError> {
        let mode = kv_get(&i.query, "mode").unwrap_or("");
        match mode {
            "loop" => spin(),
            "fetch" => {
                do_fetch(kv_get(&i.query, "url").unwrap_or("https://example.com/"))?;
                Err(HookError::Internal("fetch unexpectedly worked".into()))
            }
            _ => Ok(Summary {
                title: format!("{} {}", i.method, i.path),
                fields: kv([("host", i.host.as_str())]),
                text: None,
                link: None,
                group: None,
            }),
        }
    }
}
export_hooks!(G);
