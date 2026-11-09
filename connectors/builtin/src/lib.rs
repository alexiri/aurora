//! The connector types compiled into the connector host (SEC-69, `docs/13` §3.1).
//!
//! The set is fixed at build time: nothing in an assignment selects code, an image or a binary.
//! This crate lives under `connectors/` so that the host service itself stays free of
//! platform-specific code (SEC-33).
#![forbid(unsafe_code)]

pub mod signing;

use std::sync::{Arc, OnceLock};

use aurora_connector_fake::FakeConnector;
use aurora_connector_mattermost::managed as mattermost;
use aurora_connector_sdk::Connector;
use aurora_crypto::SigningKey;
use aurora_protocol::connector::ConnectorManifest;
use aurora_protocol::host::Assignment;
use aurora_registry::builder::PackageBuilder;
use axum::Router;

/// A to-be-run instance: the connector, its public callbacks and (test types only) a test
/// control API.
pub struct Built {
    pub connector: Arc<dyn Connector>,
    /// Mounted under `/connectors/<id>/` on the callback port.
    pub callbacks: Option<Router>,
    /// Mounted under `/test/<id>/` on the ops port; only the `fake` type has one.
    pub test_control: Option<Router>,
}

/// Builds one instance from an assignment and the API origin. Errors name a setting, never a value.
pub type BuildFn = Arc<dyn Fn(&Assignment, &str) -> Result<Built, String> + Send + Sync>;

/// One compiled connector type.
#[derive(Clone)]
pub struct CompiledType {
    pub type_id: String,
    pub manifest: ConnectorManifest,
    /// Detached signature embedded at build time (empty when the build was not signed).
    pub signature: String,
    pub build: BuildFn,
}

/// The embedded signature of `type_id` (see `build.rs`).
fn embedded_signature(type_id: &str) -> String {
    match type_id {
        "mattermost" => include_str!(concat!(env!("OUT_DIR"), "/mattermost.sig")),
        "fake" => include_str!(concat!(env!("OUT_DIR"), "/fake.sig")),
        _ => "",
    }
    .to_owned()
}

fn mattermost_type() -> CompiledType {
    CompiledType {
        type_id: mattermost::TYPE_ID.into(),
        manifest: mattermost::manifest(),
        signature: embedded_signature(mattermost::TYPE_ID),
        build: Arc::new(|a, api_url| {
            let c = mattermost::from_assignment(&a.settings, &a.secrets, api_url)?;
            let callbacks = c.router();
            Ok(Built {
                connector: Arc::new(c),
                callbacks,
                test_control: None,
            })
        }),
    }
}

/// Publisher of the `fake` test type's manifest. The project key never signs a test type, so the
/// host signs it with a key of its own (below) that an instance admin has to trust for this
/// publisher, exactly as for any third-party connector (docs/13 §3).
pub const TEST_PUBLISHER: &str = "e2e-fake";

/// The ephemeral key of the test types, generated once per process (never stored, never used
/// outside `test_types`).
fn test_key() -> Option<&'static SigningKey> {
    static KEY: OnceLock<Option<SigningKey>> = OnceLock::new();
    // No random source: the test type is then announced unsigned and the core refuses it.
    KEY.get_or_init(|| aurora_crypto::generate_signing_key().ok())
        .as_ref()
}

/// `{ "publisher", "key_id" }` of the test types' signing key, served on the ops port at
/// `/test/signing-key` so a test can trust it (`POST /v1/admin/trusted-keys`).
pub fn test_key_info() -> serde_json::Value {
    serde_json::json!({
        "publisher": TEST_PUBLISHER,
        "key_id": test_key().map(PackageBuilder::key_id),
    })
}

fn fake_manifest() -> ConnectorManifest {
    let mut m = FakeConnector::catalogue_manifest();
    TEST_PUBLISHER.clone_into(&mut m.package.publisher);
    m
}

fn fake_type() -> CompiledType {
    let manifest = fake_manifest();
    let signature = test_key()
        .and_then(|k| signing::sign_manifest(k, &manifest).ok())
        .unwrap_or_default();
    CompiledType {
        type_id: aurora_connector_fake::connector::TYPE_ID.into(),
        manifest,
        signature,
        build: Arc::new(|a, _api_url| {
            let c = Arc::new(
                FakeConnector::from_assignment(&a.settings, &a.secrets)?
                    .with_publisher(TEST_PUBLISHER),
            );
            Ok(Built {
                connector: c.clone(),
                callbacks: c.router(),
                test_control: Some(aurora_connector_fake::control::router(c)),
            })
        }),
    }
}

/// The types of this build: the production types, plus the `fake` test type only when
/// `test_types` is set (development and e2e).
pub fn compiled(test_types: bool) -> Vec<CompiledType> {
    let mut v = vec![mattermost_type()];
    if test_types {
        v.push(fake_type());
    }
    v
}

/// Types that production images sign with the project key (never the test types).
pub fn signable() -> Vec<CompiledType> {
    compiled(false)
}
