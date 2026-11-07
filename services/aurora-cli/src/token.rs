//! BYO agent token handling: tokens come from the dashboard, never printed.
use std::io::{BufRead, IsTerminal};
use std::path::Path;

use anyhow::{Result, anyhow, bail};
use aurora_client::Client;
use aurora_client::config::{AgentEntry, Config};

pub const TOKEN_ENV: &str = "AURORA_AGENT_TOKEN";

pub fn read_token_interactive() -> Result<String> {
    let tty = std::io::stdin().is_terminal();
    if tty {
        eprint!("Agent token (from the dashboard Connect tab; input hidden): ");
        let _ = std::process::Command::new("stty").arg("-echo").status();
    }
    let mut line = String::new();
    let r = std::io::stdin().lock().read_line(&mut line);
    if tty {
        let _ = std::process::Command::new("stty").arg("echo").status();
        eprintln!();
    }
    r?;
    Ok(line.trim().to_string())
}

/// Validate (prefix + live check) and persist the token under `handle`.
pub async fn store(cfg: &mut Config, path: &Path, handle: &str, token: &str) -> Result<()> {
    if !token.starts_with("aug_") {
        bail!("that does not look like an agent token (expected prefix `aug_`)");
    }
    Client::new(&cfg.url, Some(token.to_string()))?
        .agent_container_config()
        .await
        .map_err(|e| anyhow!("token rejected by the server: {e}"))?;
    cfg.agents.insert(
        handle.to_string(),
        AgentEntry {
            token: token.to_string(),
        },
    );
    cfg.save(path)?;
    Ok(())
}

/// Order: environment variable value, then stored token, else an error.
pub fn resolve_for_run(env_val: Option<String>, cfg: &Config, agent: &str) -> Result<String> {
    if let Some(t) = env_val.filter(|t| !t.trim().is_empty()) {
        return Ok(t.trim().to_string());
    }
    if let Some(e) = cfg.agents.get(agent) {
        return Ok(e.token.clone());
    }
    bail!(
        "no token for agent `{agent}`: copy it from the agent's \"Connect\" tab in the dashboard, \
         then run `aurora agents token {agent}` (or set {TOKEN_ENV})"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use aurora_client::types::WorkspaceRef;
    use wiremock::matchers::{header, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn cfg(url: &str) -> Config {
        Config {
            url: url.into(),
            api_key: "auk_x".into(),
            workspace: WorkspaceRef {
                id: "w".into(),
                slug: "s".into(),
                name: "n".into(),
                org_id: "o".into(),
                org_slug: "os".into(),
            },
            agents: Default::default(),
        }
    }

    fn tmp() -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!("aurora-tok-{}", uuid::Uuid::now_v7()))
            .join("config.toml")
    }

    #[tokio::test]
    async fn valid_token_saved_0600() {
        use std::os::unix::fs::PermissionsExt;
        let s = MockServer::start().await;
        Mock::given(path("/v1/agent/container-config"))
            .and(header("authorization", "Bearer aug_good"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!(
                {"proxy_url":"p","env":{},"ca_pem":"x","ca_path":"/a","llm_env":{},"stubs":[]})))
            .expect(1)
            .mount(&s)
            .await;
        let p = tmp();
        let mut c = cfg(&s.uri());
        store(&mut c, &p, "bot", "aug_good").await.unwrap();
        assert_eq!(Config::load(&p).unwrap().agents["bot"].token, "aug_good");
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::remove_dir_all(p.parent().unwrap()).unwrap();
    }

    #[tokio::test]
    async fn bad_prefix_and_401_rejected() {
        let s = MockServer::start().await;
        Mock::given(path("/v1/agent/container-config"))
            .respond_with(ResponseTemplate::new(401).set_body_json(
                serde_json::json!({"error":{"code":"unauthenticated","message":"no"}}),
            ))
            .expect(1)
            .mount(&s)
            .await;
        let p = tmp();
        let mut c = cfg(&s.uri());
        assert!(store(&mut c, &p, "bot", "xyz_1").await.is_err());
        let e = store(&mut c, &p, "bot", "aug_bad").await.unwrap_err();
        assert!(e.to_string().contains("rejected"));
        assert!(!p.exists() && c.agents.is_empty());
    }

    #[test]
    fn run_resolution_order_and_error() {
        let mut c = cfg("http://x");
        let e = resolve_for_run(None, &c, "bot").unwrap_err().to_string();
        assert!(e.contains("Connect") && e.contains("aurora agents token bot"));
        c.agents.insert(
            "bot".into(),
            AgentEntry {
                token: "aug_stored".into(),
            },
        );
        assert_eq!(resolve_for_run(None, &c, "bot").unwrap(), "aug_stored");
        assert_eq!(
            resolve_for_run(Some("aug_env".into()), &c, "bot").unwrap(),
            "aug_env"
        );
    }
}
