#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! SET-1 Part C (D-236(f)): the gateway's settings snapshot reloads on `aurora_settings`, keeps
//! the previous snapshot when a generation does not validate (and counts it), and catches up by
//! poll. No real key is registered by SET-1, so the registry's test section drives it.

use aurora_db::repo::instance_settings as repo;
use aurora_gateway::invalidation::Hub;
use aurora_gateway::settings::{ReloadError, Settings};
use aurora_testkit::TestDb;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

async fn write(db: &TestDb, section: &str, value: Value) -> i64 {
    let mut conn = db.pool().acquire().await.unwrap();
    let cur = repo::get(&mut *conn, section)
        .await
        .unwrap()
        .map_or(0, |r| r.generation);
    repo::cas_write(&mut conn, section, cur, &value, None, "test")
        .await
        .unwrap()
        .unwrap()
}

async fn wait_for(mut f: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !f() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for the settings snapshot"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn limit(s: &Settings) -> Value {
    s.snapshot().effective.keys["settings_test.limit"]
        .value
        .clone()
}

#[tokio::test]
async fn notify_swaps_the_snapshot_and_reload_covers_a_missed_notify() {
    let db = TestDb::new().await;
    let settings = Settings::new(None);
    let hub = Hub::new();
    let (tx, rx) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(settings.clone().run(db.pool().clone(), hub, rx));
    // A committed write + NOTIFY: the snapshot follows.
    let g = write(&db, "settings_test", json!({ "settings_test.limit": 20 })).await;
    repo::notify(db.pool(), "settings_test", g).await.unwrap();
    let s = settings.clone();
    wait_for(move || limit(&s) == json!(20)).await;
    // Stop the background task first: its own poll tick / resync also reloads, and would
    // otherwise race the explicit reload below for "who observed generation 2".
    tx.send(true).unwrap();
    task.await.unwrap();
    // A write whose NOTIFY is lost is not seen by the listener; the poll's reload finds it.
    write(&db, "settings_test", json!({ "settings_test.limit": 30 })).await;
    let changed = settings.reload(db.pool()).await.unwrap();
    assert_eq!(changed, vec!["settings_test".to_string()]);
    assert_eq!(limit(&settings), json!(30));
    // Sections the gateway does not read are not tracked.
    write(&db, "scim", json!({})).await;
    assert!(settings.reload(db.pool()).await.unwrap().is_empty());
    assert!(!settings.snapshot().generations.contains_key("scim"));
}

/// SET-2: a swap of `governance` rebuilds the instance ceiling and the BYO ceiling at once,
/// without a restart, and drops the cached effective governance (a stale floor would loosen
/// enforcement). A key the gateway does not read leaves both alone.
#[tokio::test]
async fn a_governance_swap_rebuilds_the_instance_and_the_byo_ceiling() {
    use aurora_gateway::governance::{DbGovSource, Governance, instance_from_config};
    use aurora_gateway::resolver::DbCredentialResolver;
    use aurora_llm::governance::LlmPosture;
    let db = TestDb::new().await;
    let settings = Settings::new(None);
    let llm = Arc::new(
        aurora_llm::ProviderRegistry::from_config(&aurora_config::LlmSection::default()).unwrap(),
    );
    let base = aurora_config::RawConfig::default()
        .governance_config()
        .unwrap();
    let gov = Arc::new(Governance::new(
        instance_from_config(&base, &llm),
        llm.clone(),
        Arc::new(DbGovSource(db.pool().clone())),
    ));
    let resolver = Arc::new(DbCredentialResolver::new(db.pool().clone()));
    let policy = Arc::new(aurora_gateway::resolve::Resolver::new(
        Arc::new(aurora_gateway::resolve::DbSource(db.pool().clone())),
        llm,
    ));
    settings.rebuild_governance_on_change(gov.clone(), resolver.clone(), policy);
    assert_eq!(gov.instance().llm_minimum_posture, LlmPosture::Open);

    // An organization to cache an effective governance for.
    let org = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, slug, name, external_id) VALUES ($1,$2,'o',$2)")
        .bind(org)
        .bind(org.simple().to_string())
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(
        gov.effective(org).await.unwrap().llm_posture,
        LlmPosture::Open
    );

    write(
        &db,
        "governance",
        json!({"governance.llm.minimum_posture": "sanctioned_only", "byo.enabled": false}),
    )
    .await;
    let changed = settings.reload(db.pool()).await.unwrap();
    assert_eq!(changed, vec!["governance".to_string()]);
    assert_eq!(
        gov.instance().llm_minimum_posture,
        LlmPosture::SanctionedOnly
    );
    assert!(!resolver.byo().enabled, "byo.enabled is live");
    // The cached `open` is gone: the next read is computed from the new floor.
    assert_eq!(
        gov.effective(org).await.unwrap().llm_posture,
        LlmPosture::SanctionedOnly
    );
    // Back again.
    write(&db, "governance", json!({})).await;
    settings.reload(db.pool()).await.unwrap();
    assert_eq!(gov.instance().llm_minimum_posture, LlmPosture::Open);
    assert!(resolver.byo().enabled);
}

/// SET-3: a swap of `llm` rebuilds the endpoint table of the policy resolver and of the
/// governance step, the reserved LLM hosts and the instance's endpoints and known LLM hosts
/// (built-in − endpoint hosts), drops the cached effective governance — and a removal undoes it.
#[tokio::test]
async fn an_llm_swap_rebuilds_the_endpoint_table_and_the_known_hosts() {
    use aurora_gateway::governance::{DbGovSource, Governance, instance_from_config};
    use aurora_gateway::resolver::DbCredentialResolver;
    let db = TestDb::new().await;
    let settings = Settings::new(None);
    let llm = Arc::new(
        aurora_llm::ProviderRegistry::from_config(&aurora_config::LlmSection::default()).unwrap(),
    );
    let base = aurora_config::RawConfig::default()
        .governance_config()
        .unwrap();
    let gov = Arc::new(Governance::new(
        instance_from_config(&base, &llm),
        llm.clone(),
        Arc::new(DbGovSource(db.pool().clone())),
    ));
    let resolver = Arc::new(DbCredentialResolver::new(db.pool().clone()));
    let policy = Arc::new(aurora_gateway::resolve::Resolver::new(
        Arc::new(aurora_gateway::resolve::DbSource(db.pool().clone())),
        llm,
    ));
    settings.rebuild_governance_on_change(gov.clone(), resolver, policy.clone());
    let org = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, slug, name, external_id) VALUES ($1,$2,'o',$2)")
        .bind(org)
        .bind(org.simple().to_string())
        .execute(db.pool())
        .await
        .unwrap();
    assert!(
        !gov.effective(org)
            .await
            .unwrap()
            .endpoints
            .contains_key("team"),
        "no `team` endpoint yet (and the effective view is cached now)"
    );
    assert!(
        gov.instance()
            .known_llm_hosts
            .iter()
            .any(|h| h == "api.openai.com")
    );
    assert!(!policy.llm().is_endpoint_host("litellm.team.example"));

    write(
        &db,
        "llm",
        json!({
            "llm.enabled_providers": ["litellm", "anthropic", "openai_compatible"],
            "llm.endpoints": [
                {"id": "team", "provider": "litellm", "base_url": "https://litellm.team.example"},
                {"id": "oa", "provider": "openai_compatible", "base_url": "https://api.openai.com"},
            ],
        }),
    )
    .await;
    let changed = settings.reload(db.pool()).await.unwrap();
    assert_eq!(changed, vec!["llm".to_string()]);
    for table in [policy.llm(), gov.registry()] {
        assert!(table.endpoint("team").is_some(), "endpoint table rebuilt");
        assert!(
            table.is_endpoint_host("litellm.team.example"),
            "host reserved"
        );
    }
    assert!(gov.instance().endpoints.iter().any(|e| e.id == "team"));
    assert!(
        !gov.instance()
            .known_llm_hosts
            .iter()
            .any(|h| h == "api.openai.com"),
        "an endpoint host leaves the known list"
    );
    assert!(
        gov.effective(org)
            .await
            .unwrap()
            .endpoints
            .contains_key("team"),
        "the cached effective governance did not survive the swap"
    );

    // Removed again: the table, the reservation and the known list return to the start.
    write(&db, "llm", json!({})).await;
    settings.reload(db.pool()).await.unwrap();
    assert!(policy.llm().endpoint("team").is_none());
    assert!(!gov.registry().is_endpoint_host("litellm.team.example"));
    assert!(
        gov.instance()
            .known_llm_hosts
            .iter()
            .any(|h| h == "api.openai.com")
    );
    assert!(
        !gov.effective(org)
            .await
            .unwrap()
            .endpoints
            .contains_key("team")
    );
}

/// (P4-LLM-6, D-262) The destination guard's implicit allowance comes from the Helm-defined LLM
/// endpoints only: a Helm endpoint on a private address passes, the same entry as a dashboard
/// endpoint is refused until `gateway.allowed_private` covers it (live), a name that resolves to
/// a link-local or metadata address is refused for both sources, and removing an endpoint revokes
/// its pass at the next judgement.
#[test]
fn only_helm_endpoints_open_their_private_hosts_in_the_guard() {
    use aurora_config::{ConfigSource, Stored};
    use aurora_gateway::guard::Guard;
    use aurora_gateway::settings::helm_endpoint_hosts;
    let ip = |s: &str| s.parse::<std::net::IpAddr>().unwrap();
    let helm = |extra: &str| {
        ConfigSource::from_toml(
            &format!(
                "[llm]\nenabled_providers = [\"litellm\", \"openai_compatible\", \"anthropic\"]\n\
                 [[llm.endpoints]]\nid = \"central\"\nprovider = \"litellm\"\nbase_url = \"http://llm.cluster.internal:4000\"\n{extra}"
            ),
            Default::default(),
        )
        .unwrap()
    };
    let stored = |entries: Value| {
        let mut s = Stored::default();
        let llm = s.sections.entry("llm".into()).or_default();
        llm.insert(
            "llm.enabled_providers".into(),
            json!(["litellm", "anthropic", "openai_compatible"]),
        );
        llm.insert("llm.endpoints".into(), entries);
        s
    };
    let dash = json!([
        {"id": "team", "provider": "openai_compatible", "base_url": "http://team-llm.corp:8080"},
        {"id": "lit", "provider": "openai_compatible", "base_url": "http://169.254.169.254:8080"},
    ]);
    let eff = helm("").effective(&stored(dash.clone())).unwrap();
    let hosts = helm_endpoint_hosts(&eff).unwrap();
    assert_eq!(hosts, vec!["llm.cluster.internal".to_string()]);

    let g = Guard::new(&[], vec!["0.0.0.0:4102".parse().unwrap()]).unwrap();
    g.set_derived_hosts(&hosts).unwrap();
    // Helm endpoint: its name passes for a private address, other names do not.
    assert!(
        g.check_ip(ip("10.0.0.7"), "llm.cluster.internal", 4000)
            .is_ok()
    );
    // Dashboard endpoint: refused until `gateway.allowed_private` covers it (live).
    assert!(g.check_ip(ip("10.0.0.9"), "team-llm.corp", 8080).is_err());
    g.set_allowed_private(&["10.0.0.0/24".to_owned()]).unwrap();
    assert!(g.check_ip(ip("10.0.0.9"), "team-llm.corp", 8080).is_ok());
    g.set_allowed_private(&[]).unwrap();
    assert!(g.check_ip(ip("10.0.0.9"), "team-llm.corp", 8080).is_err());
    // Link-local and metadata addresses: refused for both sources, by name or literal.
    for bad in ["169.254.169.254", "169.254.7.7", "fe80::1"] {
        for host in ["llm.cluster.internal", "team-llm.corp", bad] {
            assert!(g.check_ip(ip(bad), host, 80).is_err(), "{bad} for {host}");
        }
    }
    // Removing the Helm endpoint (a new deployment) revokes its pass on the next rebuild.
    let none = ConfigSource::empty().effective(&stored(dash)).unwrap();
    g.set_derived_hosts(&helm_endpoint_hosts(&none).unwrap())
        .unwrap();
    assert!(
        g.check_ip(ip("10.0.0.7"), "llm.cluster.internal", 4000)
            .is_err()
    );
}

#[tokio::test]
async fn an_invalid_generation_keeps_the_previous_snapshot_and_counts() {
    let db = TestDb::new().await;
    let settings = Settings::new(None);
    write(&db, "settings_test", json!({ "settings_test.limit": 12 })).await;
    settings.reload(db.pool()).await.unwrap();
    assert_eq!(limit(&settings), json!(12));
    // Generation 2 is out of its bounds: not swapped in, never the default, counted.
    write(&db, "settings_test", json!({ "settings_test.limit": 9999 })).await;
    let err = settings.reload(db.pool()).await.unwrap_err();
    assert!(matches!(err, ReloadError::Invalid(_)), "{err}");
    assert_eq!(limit(&settings), json!(12), "the previous snapshot stays");
    assert_eq!(settings.snapshot().generations["settings_test"], 1);
    assert_eq!(settings.reload_failures(), 1);
    // The next valid generation swaps.
    write(&db, "settings_test", json!({ "settings_test.limit": 13 })).await;
    settings.reload(db.pool()).await.unwrap();
    assert_eq!(limit(&settings), json!(13));
    assert_eq!(settings.snapshot().generations["settings_test"], 3);
}

#[tokio::test]
async fn an_unreadable_table_keeps_the_snapshot_and_the_gateway_running() {
    let db = TestDb::new().await;
    let settings = Settings::new(None);
    write(&db, "settings_test", json!({ "settings_test.limit": 12 })).await;
    settings.reload(db.pool()).await.unwrap();
    // A gateway whose role has no grant yet (rollout order) sees an error, not a swap.
    sqlx::query("ALTER TABLE instance_settings RENAME TO instance_settings_gone")
        .execute(db.pool())
        .await
        .unwrap();
    let err = settings.reload(db.pool()).await.unwrap_err();
    assert!(matches!(err, ReloadError::Database(_)), "{err}");
    assert_eq!(limit(&settings), json!(12));
}
