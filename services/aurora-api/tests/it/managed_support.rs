#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, dead_code)]
//! Shared fixtures of the managed-connector tests (P4-MCON-1): a world with an instance admin, a
//! trusted publisher key, a connector host token and signed manifests of a test type that carries
//! every `instance_setup` field kind. Used by `connectors_managed.rs` and by the log-capture
//! binary `tests/managed_logs.rs`.

use crate::common::*;
use aurora_core::token::{self, TokenPrefix};
use aurora_crypto::{SigningKey, generate_signing_key};
use aurora_db::repo::connector_hosts;
use aurora_registry::builder::PackageBuilder;
use http::{HeaderMap, Method};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use uuid::Uuid;

pub const SECRET_VALUE: &str = "S3cret-Value-do-not-leak-0123456789";
pub const SECRET_VALUE_2: &str = "An0ther-Secret-Value-9876543210";

pub struct M {
    pub h: Harness,
    pub admin: String,
    pub key: SigningKey,
    pub host_token: String,
    pub mem: Option<Arc<aurora_secrets::MemoryStore>>,
}

pub fn hd(c: &str) -> [(&str, &str); 2] {
    [("cookie", c), ("origin", ORIGIN)]
}

fn base64_key() -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode([b'a'; 32])
}

/// The fields of the test type: every kind of `instance_setup` field.
pub fn setup_fields() -> Value {
    json!([
        {"name": "public_url", "label": "Public URL", "type": "url"},
        {"name": "catchup_window_secs", "label": "Catch-up", "type": "number",
         "default": 600, "min": 1, "max": 86400},
        {"name": "region", "label": "Region", "type": "select", "default": "eu",
         "options": [{"value": "eu", "label": "EU"}, {"value": "us", "label": "US"}]},
        {"name": "api_key", "label": "API key", "type": "secret", "required": true},
        {"name": "extra_token", "label": "Extra token", "type": "secret"},
        {"name": "callback_hmac_key", "label": "HMAC key", "type": "generated_secret"}
    ])
}

/// Manifest of type `id` (`package.id = id`).
pub fn manifest(id: &str) -> Value {
    json!({
        "package": {"id": id, "version": "1.0.0", "contract": 1, "publisher": "acme"},
        "about": {"name": format!("Type {id}")},
        "capabilities": {"threads": true, "reactions": true},
        "integration_setup": {"fields": []},
        "presence_setup": {},
        "limits": {},
        "public_callbacks": ["actions"],
        "instance_setup": {"fields": setup_fields()}
    })
}

impl M {
    pub async fn new() -> Self {
        Self::build(None, false).await
    }

    /// `plugins.allow_unsigned = true` (development, D-142).
    pub async fn with_allow_unsigned() -> Self {
        Self::build(None, true).await
    }

    /// With a memory store as the default secret store, which a test can make unavailable and
    /// whose managed values it can count.
    pub async fn with_memory_store() -> Self {
        Self::build(
            Some(Arc::new(aurora_secrets::MemoryStore::new("memory"))),
            false,
        )
        .await
    }

    async fn build(mem: Option<Arc<aurora_secrets::MemoryStore>>, allow_unsigned: bool) -> Self {
        let mut h = if allow_unsigned {
            Harness::with_config(|c| c.plugins.allow_unsigned = true).await
        } else {
            Harness::new().await
        };
        if let Some(mem) = &mem {
            let keyring = Arc::new(
                aurora_crypto::Keyring::parse(&format!("k1:{}", base64_key()), "k1").unwrap(),
            );
            let mut sr =
                aurora_secrets::StoreRegistry::from_config(&h.state.config.secrets, keyring)
                    .unwrap();
            sr.register(mem.clone());
            sr.set_default("memory").unwrap();
            h.state = h
                .state
                .clone()
                .with_vault(Arc::new(aurora_secrets::SecretVault::new(sr)));
            h.app = test_router(h.state.clone());
        }
        let root = h.user("root@x.io", true).await;
        let admin = cookie(&h.good_session(root).await.1);
        let key = generate_signing_key().unwrap();
        let (s, _, v) = h
            .send(
                Method::POST,
                "/v1/admin/trusted-keys",
                &hd(&admin),
                Some(json!({"publisher": "acme", "name": "k", "public_key": PackageBuilder::key_id(&key)})),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        let host_token = token::generate(TokenPrefix::ConnectorHost).expose().clone();
        connector_hosts::upsert_token(h.pool(), &token::hash(&host_token))
            .await
            .unwrap();
        Self {
            h,
            admin,
            key,
            host_token,
            mem,
        }
    }

    pub fn sign(&self, m: &Value) -> String {
        let typed: aurora_protocol::connector::ConnectorManifest =
            serde_json::from_value(m.clone()).unwrap();
        let v = serde_json::to_value(&typed).unwrap();
        let d: [u8; 32] = Sha256::digest(aurora_registry::snapshot::canonical_bytes(&v)).into();
        PackageBuilder::sign_digest(&self.key, &PackageBuilder::key_id(&self.key), &d)
    }

    /// Digest (hex) of a manifest as the API computes it.
    pub fn digest(&self, m: &Value) -> String {
        let typed: aurora_protocol::connector::ConnectorManifest =
            serde_json::from_value(m.clone()).unwrap();
        let v = serde_json::to_value(&typed).unwrap();
        let d: [u8; 32] = Sha256::digest(aurora_registry::snapshot::canonical_bytes(&v)).into();
        d.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub async fn admin(&self, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        let (s, _, v) = self.h.send(m, uri, &hd(&self.admin), body).await;
        (s.as_u16(), v)
    }

    /// Request on the host plane with the host token (or `tok`).
    pub async fn host_req(
        &self,
        tok: Option<&str>,
        m: Method,
        path: &str,
        body: Option<Value>,
    ) -> (u16, HeaderMap, Value) {
        let b = bearer(tok.unwrap_or(&self.host_token));
        let (s, hm, v) = self
            .h
            .send(
                m,
                &format!("/v1/connector-host{path}"),
                &[("authorization", &b)],
                body,
            )
            .await;
        (s.as_u16(), hm, v)
    }

    /// Host `register` with signed manifests.
    pub async fn host_register(
        &self,
        instance: Uuid,
        version: &str,
        manifests: &[&Value],
    ) -> (u16, Value) {
        let types: Vec<Value> = manifests
            .iter()
            .map(|m| {
                json!({
                    "type_id": m["package"]["id"],
                    "manifest": m,
                    "manifest_signature": self.sign(m),
                })
            })
            .collect();
        self.host_register_raw(instance, version, json!(types))
            .await
    }

    pub async fn host_register_raw(
        &self,
        instance: Uuid,
        version: &str,
        types: Value,
    ) -> (u16, Value) {
        let (s, _, v) = self
            .host_req(
                None,
                Method::POST,
                "/register",
                Some(json!({"instance_id": instance, "version": version, "types": types})),
            )
            .await;
        (s, v)
    }

    /// Valid managed-create body for type `id`.
    pub fn create_body(&self, id: &str) -> Value {
        json!({
            "id": id,
            "managed": true,
            "settings": {"catchup_window_secs": 300},
            "secrets": {"api_key": SECRET_VALUE},
            "approve_manifest_digest": self.digest(&manifest(id)),
        })
    }

    /// Announce type `id` from a fresh host and add it as a managed connector.
    pub async fn add_managed(&self, id: &str) -> (Uuid, Value) {
        let host = Uuid::now_v7();
        let (s, v) = self.host_register(host, "1.0.0", &[&manifest(id)]).await;
        assert_eq!(s, 200, "{v}");
        let (s, v) = self
            .admin(
                Method::POST,
                "/v1/admin/connectors",
                Some(self.create_body(id)),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        (host, v)
    }

    pub async fn assignments(
        &self,
        host: Option<Uuid>,
        revision: Option<i64>,
        wait: u32,
    ) -> (u16, HeaderMap, Value) {
        let mut q = format!("?wait={wait}");
        if let Some(r) = revision {
            q.push_str(&format!("&revision={r}"));
        }
        if let Some(h) = host {
            q.push_str(&format!("&instance_id={h}"));
        }
        self.host_req(None, Method::GET, &format!("/assignments{q}"), None)
            .await
    }

    /// Current assignments revision.
    pub async fn revision(&self) -> i64 {
        let (s, _, v) = self.assignments(None, None, 0).await;
        assert_eq!(s, 200, "{v}");
        v["revision"].as_i64().unwrap()
    }
}
