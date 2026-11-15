#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! SET-1 Part C (D-236(f)): the gateway's settings snapshot reloads on `aurora_settings`, keeps
//! the previous snapshot when a generation does not validate (and counts it), and catches up by
//! poll. No real key is registered by SET-1, so the registry's test section drives it.

use aurora_db::repo::instance_settings as repo;
use aurora_gateway::invalidation::Hub;
use aurora_gateway::settings::{ReloadError, Settings};
use aurora_testkit::TestDb;
use serde_json::{Value, json};
use std::time::Duration;

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
    // A write whose NOTIFY is lost is not seen by the listener; the poll's reload finds it.
    write(&db, "settings_test", json!({ "settings_test.limit": 30 })).await;
    let changed = settings.reload(db.pool()).await.unwrap();
    assert_eq!(changed, vec!["settings_test".to_string()]);
    assert_eq!(limit(&settings), json!(30));
    // Sections the gateway does not read are not tracked.
    write(&db, "scim", json!({})).await;
    assert!(settings.reload(db.pool()).await.unwrap().is_empty());
    assert!(!settings.snapshot().generations.contains_key("scim"));
    tx.send(true).unwrap();
    let _ = task.await;
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
