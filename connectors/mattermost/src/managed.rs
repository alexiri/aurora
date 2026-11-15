//! Library entry point for the connector host (13 §3.1): a typed [`Settings`] built from a managed
//! connector's assignment (`settings` + decrypted `secrets`), shared with the standalone binary's
//! code path (both end in [`MattermostConnector::new`] and the SDK runtime).

use std::collections::BTreeMap;
use std::time::Duration;

use aurora_connector_sdk::Connector;
use aurora_protocol::connector::ConnectorManifest;
use serde_json::Value;

use crate::connector::MattermostConnector;
use crate::integ::Settings;

/// The type id of this connector (`package.id`).
pub const TYPE_ID: &str = "mattermost";
const MIN_HMAC_KEY_BYTES: usize = 32;
const DEFAULT_CATCHUP_SECS: u64 = 600;
const DEFAULT_TOKEN_CHECK_SECS: u64 = 30;
const MAX_CATCHUP_SECS: u64 = 86_400;
const MAX_TOKEN_CHECK_SECS: u64 = 3_600;

/// The manifest this type announces to the core (with `instance_setup`).
pub fn manifest() -> ConnectorManifest {
    MattermostConnector::new(placeholder_settings())
        .managed()
        .manifest()
}

fn placeholder_settings() -> Settings {
    Settings {
        public_url: None,
        hmac_key: Vec::new(),
        catchup_window: Duration::from_secs(DEFAULT_CATCHUP_SECS),
        api_url: None,
        token_check: Duration::from_secs(DEFAULT_TOKEN_CHECK_SECS),
        capabilities: None,
        test_control: false,
        net: crate::api::NetPolicy::from_env(),
    }
}

fn number(settings: &Value, name: &str, default: u64, max: u64) -> Result<u64, String> {
    match settings.get(name) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_u64()
            .filter(|n| (1..=max).contains(n))
            .ok_or_else(|| format!("setting `{name}` must be a whole number from 1 to {max}")),
    }
}

/// Builds the connector [`Settings`] from an assignment. Errors name the setting, never a value.
pub fn settings_from_assignment(
    settings: &Value,
    secrets: &BTreeMap<String, String>,
    api_url: &str,
) -> Result<Settings, String> {
    let key = secrets
        .get("callback_hmac_key")
        .map(String::as_str)
        .unwrap_or_default();
    if key.len() < MIN_HMAC_KEY_BYTES {
        return Err("secret `callback_hmac_key` is missing or too short".into());
    }
    let public_url = match settings.get("public_url") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => {
            Some(s.trim_end_matches('/').to_owned()).filter(|u| !u.is_empty())
        }
        Some(_) => return Err("setting `public_url` must be a string".into()),
    };
    Ok(Settings {
        public_url,
        hmac_key: key.as_bytes().to_vec(),
        catchup_window: Duration::from_secs(number(
            settings,
            "catchup_window_secs",
            DEFAULT_CATCHUP_SECS,
            MAX_CATCHUP_SECS,
        )?),
        api_url: Some(api_url.trim_end_matches('/').to_owned()),
        token_check: Duration::from_secs(number(
            settings,
            "token_check_secs",
            DEFAULT_TOKEN_CHECK_SECS,
            MAX_TOKEN_CHECK_SECS,
        )?),
        capabilities: None,
        test_control: false,
        net: crate::api::NetPolicy::from_env(),
    })
}

/// The connector instance for one assignment: run it with the SDK runtime and mount its
/// [`Connector::router`] under `/connectors/<id>/`.
pub fn from_assignment(
    settings: &Value,
    secrets: &BTreeMap<String, String>,
    api_url: &str,
) -> Result<MattermostConnector, String> {
    Ok(MattermostConnector::new(settings_from_assignment(settings, secrets, api_url)?).managed())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn secrets(k: &str) -> BTreeMap<String, String> {
        BTreeMap::from([("callback_hmac_key".to_owned(), k.to_owned())])
    }

    #[test]
    fn manifest_declares_instance_setup_and_icon() {
        let m = manifest();
        let fields = m.instance_setup.expect("instance_setup").fields;
        let names: Vec<_> = fields.iter().filter_map(|f| f["name"].as_str()).collect();
        assert_eq!(
            names,
            [
                "public_url",
                "callback_hmac_key",
                "catchup_window_secs",
                "token_check_secs"
            ]
        );
        assert_eq!(fields[1]["type"], "generated_secret");
        assert!(m.about.icon_png_b64.is_some());
        assert_eq!(m.package.id, TYPE_ID);
    }

    #[test]
    fn standalone_manifest_has_no_instance_setup() {
        let m = MattermostConnector::new(placeholder_settings()).manifest();
        assert!(m.instance_setup.is_none());
    }

    #[test]
    fn settings_apply_defaults_and_trim() {
        let key = "k".repeat(44);
        let s = settings_from_assignment(
            &json!({"public_url": "https://a.example/connectors/mattermost/"}),
            &secrets(&key),
            "http://api:4101/",
        )
        .expect("settings");
        assert_eq!(
            s.public_url.as_deref(),
            Some("https://a.example/connectors/mattermost")
        );
        assert_eq!(s.catchup_window, Duration::from_secs(600));
        assert_eq!(s.token_check, Duration::from_secs(30));
        assert_eq!(s.api_url.as_deref(), Some("http://api:4101"));
    }

    #[test]
    fn bad_settings_are_named_without_values() {
        let key = "k".repeat(44);
        let e = settings_from_assignment(
            &json!({"catchup_window_secs": 0}),
            &secrets(&key),
            "http://api",
        )
        .expect_err("must fail");
        assert!(e.contains("catchup_window_secs"));
        let e = settings_from_assignment(&json!({}), &secrets("short-secret-value"), "http://api")
            .expect_err("must fail");
        assert!(e.contains("callback_hmac_key") && !e.contains("short-secret-value"));
    }
}
