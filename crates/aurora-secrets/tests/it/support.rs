#![allow(clippy::unwrap_used, clippy::expect_used, dead_code)]
//! Shared helpers for the Vault store tests.

use aurora_config::{SecretStoreConfig, VaultSettings};
use aurora_secrets::{Purpose, SealContext, VaultStore};
use serde_json::{Value, json};
use uuid::Uuid;

pub fn ctx(org: Uuid) -> SealContext {
    SealContext {
        purpose: Purpose::SecretValue,
        owner_id: Uuid::now_v7(),
        org_id: org,
        workspace_id: None,
    }
}

/// Parse a `[[secrets.stores]]` entry written as a TOML table body.
pub fn store_config(toml_text: &str) -> SecretStoreConfig {
    toml::from_str(toml_text).unwrap()
}

pub fn build(toml_text: &str) -> VaultStore {
    VaultStore::new(
        VaultSettings::parse(&store_config(toml_text)).unwrap(),
        None,
    )
    .unwrap()
}

/// A real Vault dev server in a throwaway container, enabled by `AURORA_TEST_VAULT=1`:
///
/// ```text
/// podman run -d --name p3-vlt-1-vault -p 127.0.0.1:18200:8200 \
///   -e VAULT_DEV_ROOT_TOKEN_ID=aurora-dev-root \
///   docker.io/hashicorp/vault@sha256:<digest> server -dev -dev-listen-address=0.0.0.0:8200
/// AURORA_TEST_VAULT=1 AURORA_TEST_VAULT_ADDR=http://127.0.0.1:18200 \
///   AURORA_TEST_VAULT_TOKEN=aurora-dev-root cargo test -p aurora-secrets
/// ```
///
/// The dev server mounts a KV v2 engine at `secret/`.
pub struct RealVault {
    pub addr: String,
    pub root_token: String,
    pub http: reqwest::Client,
}

pub fn managed_prefix(unique: &str) -> String {
    format!("{unique}-managed")
}

pub const TOKEN_ENV: &str = "AURORA_TEST_VAULT_TOKEN";

pub fn real() -> Option<RealVault> {
    // (FIX-140, D-198) unset: SKIPPED outside CI, a panic under CI=true.
    if !aurora_testkit::require_or_skip("AURORA_TEST_VAULT") {
        return None;
    }
    Some(RealVault {
        addr: std::env::var("AURORA_TEST_VAULT_ADDR")
            .unwrap_or_else(|_| "http://127.0.0.1:18200".into()),
        root_token: std::env::var(TOKEN_ENV).expect("AURORA_TEST_VAULT_TOKEN must be set"),
        http: reqwest::Client::new(),
    })
}

impl RealVault {
    /// A store using `token` auth (`AURORA_TEST_VAULT_TOKEN`) with the given reference scopes
    /// (`(org id, globs)`). `unique` names the test's own area of the KV mount: reference
    /// paths live under `<unique>/`, managed values under `<unique>-managed/`.
    pub fn store(&self, unique: &str, scopes: &[(Uuid, &[&str])]) -> VaultStore {
        build(&self.store_toml(&managed_prefix(unique), scopes))
    }

    pub fn store_toml(&self, prefix: &str, scopes: &[(Uuid, &[&str])]) -> String {
        let mut t = format!(
            "id = \"vault\"\nkind = \"hashicorp-vault\"\naddress = \"{}\"\nmanaged_prefix = \"{prefix}\"\nauth = {{ method = \"token\", token_env = \"{TOKEN_ENV}\" }}\n",
            self.addr
        );
        for (org, globs) in scopes {
            let list = globs
                .iter()
                .map(|g| format!("\"{g}\""))
                .collect::<Vec<_>>()
                .join(", ");
            t.push_str(&format!(
                "[[reference_scopes]]\norg_id = \"{org}\"\npaths = [{list}]\n"
            ));
        }
        t
    }

    /// Write `data` at `secret/data/<path>` as the root token (an external system's write).
    pub async fn write(&self, path: &str, data: Value) -> u64 {
        let r = self
            .http
            .post(format!("{}/v1/secret/data/{path}", self.addr))
            .header("X-Vault-Token", &self.root_token)
            .json(&json!({ "data": data }))
            .send()
            .await
            .unwrap();
        assert!(r.status().is_success(), "seed write failed: {}", r.status());
        let v: Value = r.json().await.unwrap();
        v["data"]["version"].as_u64().unwrap()
    }

    /// Raw KV read as the root token: `(status, data map or null)`.
    pub async fn read(&self, path: &str, version: Option<u64>) -> (u16, Value) {
        let q = version.map_or(String::new(), |v| format!("?version={v}"));
        let r = self
            .http
            .get(format!("{}/v1/secret/data/{path}{q}", self.addr))
            .header("X-Vault-Token", &self.root_token)
            .send()
            .await
            .unwrap();
        let s = r.status().as_u16();
        let v: Value = r.json().await.unwrap_or(Value::Null);
        (s, v)
    }
}
