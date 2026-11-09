#![allow(clippy::unwrap_used, clippy::expect_used)]
//! CAS race tests against a real Vault dev server (`AURORA_TEST_VAULT=1`).
//!
//! The managed store never overwrites: every `put` is a new KV version, so concurrent writers
//! to one managed path must each get a distinct version holding exactly their own value. The
//! KV v2 check-and-set option (what a compare-and-swap writer would use) must let exactly one
//! of several racing writers win each version. The row-level swap in the migration command is
//! covered by `aurora-api` `secrets_migrate::racing_migrators_*`.

use std::collections::BTreeSet;
use std::sync::Arc;

use aurora_core::Secret;
use aurora_secrets::SecretStore;
use serde_json::json;
use uuid::Uuid;

use crate::support::{ctx, managed_prefix, real};

/// Below the dev server's default `max_versions` (10): a ref pins its version, and Vault prunes
/// the oldest versions beyond `max_versions` (operator docs: set it to at least 10).
const WRITERS: usize = 8;

#[tokio::test]
async fn concurrent_puts_to_one_path_get_distinct_versions_and_keep_their_own_value() {
    let Some(v) = real() else { return };
    let t = format!("cas-{}", Uuid::now_v7().simple());
    let s = Arc::new(v.store(&t, &[]));
    let c = ctx(Uuid::now_v7());

    let mut tasks = Vec::new();
    for i in 0..WRITERS {
        let s = s.clone();
        tasks.push(tokio::spawn(async move {
            let val = format!("value-{i}");
            let r = s
                .put(&c, &Secret::new(val.clone().into_bytes()))
                .await
                .unwrap();
            (val, r)
        }));
    }
    let mut versions = BTreeSet::new();
    let mut written = Vec::new();
    for t in tasks {
        let (val, r) = t.await.unwrap();
        let col = r.to_column();
        let ver: u64 = col.rsplit('@').next().unwrap().parse().unwrap();
        assert!(versions.insert(ver), "version {ver} handed to two writers");
        written.push((val, r));
    }
    // Exactly versions 1..=WRITERS, none skipped, none shared.
    assert_eq!(
        versions.into_iter().collect::<Vec<_>>(),
        (1..=WRITERS as u64).collect::<Vec<_>>()
    );
    // No value lost or mixed: each ref reads back exactly what its writer wrote.
    for (val, r) in &written {
        assert_eq!(s.get(&c, r).await.unwrap().expose(), val.as_bytes());
    }
}

#[tokio::test]
async fn kv_check_and_set_lets_exactly_one_racing_writer_win_each_version() {
    let Some(v) = real() else { return };
    let path = format!("{}/cas-kv/{}", managed_prefix("cas"), Uuid::now_v7());
    let post = |cas: u64, who: usize| {
        let (http, url, tok) = (
            v.http.clone(),
            format!("{}/v1/secret/data/{path}", v.addr),
            v.root_token.clone(),
        );
        async move {
            let r = http
                .post(url)
                .header("X-Vault-Token", tok)
                .json(&json!({"options": {"cas": cas}, "data": {"value": format!("w{who}")}}))
                .send()
                .await
                .unwrap();
            (who, r.status().as_u16())
        }
    };

    // cas = 0 creates the secret: exactly one of the racers creates it.
    let mut cur = 0_u64;
    for _round in 0..3 {
        let handles: Vec<_> = (0..8).map(|i| tokio::spawn(post(cur, i))).collect();
        let mut results = Vec::new();
        for h in handles {
            results.push(h.await.unwrap());
        }
        let winners: Vec<usize> = results
            .iter()
            .filter(|(_, s)| *s == 200)
            .map(|(w, _)| *w)
            .collect();
        assert_eq!(winners.len(), 1, "round at cas={cur}: {results:?}");
        assert!(
            results.iter().all(|(_, s)| *s == 200 || *s == 400),
            "losers must fail cleanly: {results:?}"
        );
        cur += 1;
        let (st, body) = v.read(&path, Some(cur)).await;
        assert_eq!(st, 200);
        assert_eq!(
            body["data"]["data"]["value"],
            format!("w{}", winners[0]),
            "version {cur} holds the winner's value"
        );
        assert_eq!(body["data"]["metadata"]["version"], cur);
    }
}
