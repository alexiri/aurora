#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-UX-4 (C-917): an accounts list costs the same number of queries for two accounts or for
//! many (no N+1). Own test binary, like `db_spans.rs`: it installs a scoped tracing subscriber to
//! count sqlx's `sqlx::query` events, which must not race with other tests' callsite registration.

#[path = "it/common/mod.rs"]
mod common;

use crate::common::*;
use http::Method;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tracing_subscriber::layer::{Context, SubscriberExt};
use uuid::Uuid;

struct Count(Arc<AtomicUsize>);

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Count {
    fn on_event(&self, e: &tracing::Event<'_>, _: Context<'_, S>) {
        if e.metadata().target() == "sqlx::query" {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
}

struct World {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    owner: String,
    agents: Vec<Uuid>,
}

impl World {
    async fn list(&self) -> serde_json::Value {
        let (s, _, v) = self
            .h
            .send(
                Method::GET,
                &format!("/v1/workspaces/{}/accounts", self.ws),
                &[("cookie", self.owner.as_str()), ("origin", ORIGIN)],
                None,
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v
    }

    /// The number of statements one list runs. The tests of this binary run on a current-thread
    /// runtime, so a scoped default covers every task of the request.
    async fn queries(&self) -> (usize, usize) {
        let n = Arc::new(AtomicUsize::new(0));
        let guard =
            tracing::subscriber::set_default(tracing_subscriber::registry().with(Count(n.clone())));
        let v = self.list().await;
        drop(guard);
        (
            n.load(Ordering::SeqCst),
            v["items"].as_array().unwrap().len(),
        )
    }

    async fn add_accounts(&self, n: usize) {
        for _ in 0..n {
            let (conn, org_conn, site, key) = (
                Uuid::now_v7(),
                Uuid::now_v7(),
                Uuid::now_v7(),
                Uuid::now_v7(),
            );
            for (id, ws) in [(conn, Some(self.ws)), (org_conn, None)] {
                sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref) VALUES ($1,$2,$3,'some_app','p','1.0.0','c','active','x')")
                    .bind(id).bind(self.org).bind(ws)
                    .execute(self.h.pool()).await.unwrap();
            }
            for (id, kind) in [(site, "generic"), (key, "llm")] {
                sqlx::query("INSERT INTO secrets (id, org_id, workspace_id, name, kind, llm_provider, llm_auth_mode, llm_endpoint_id, host_pattern, injection, value_source, value_ref) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'api.example.com','{}','managed','x')")
                    .bind(id).bind(self.org).bind(self.ws).bind(format!("{kind} {id}")).bind(kind)
                    .bind((kind == "llm").then_some("anthropic"))
                    .bind((kind == "llm").then_some("api_key"))
                    .bind((kind == "llm").then_some("anthropic"))
                    .execute(self.h.pool()).await.unwrap();
            }
            // Every agent holds a grant on every kind of account.
            for (n, agent) in self.agents.iter().enumerate() {
                for (conn_id, secret_id) in [
                    (Some(conn), None),
                    (Some(org_conn), None),
                    (None, Some(site)),
                    (None, Some(key)),
                ] {
                    sqlx::query("INSERT INTO grants (id, workspace_id, agent_id, connection_id, secret_id, mode) VALUES ($1,$2,$3,$4,$5,$6)")
                        .bind(Uuid::now_v7()).bind(self.ws).bind(agent).bind(conn_id).bind(secret_id)
                        .bind(conn_id.map(|_| if n == 0 { "full" } else { "custom" }))
                        .execute(self.h.pool()).await.unwrap();
                }
            }
        }
    }
}

async fn world() -> World {
    let h = Harness::new().await;
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let o = h.user("owner@x.io", false).await;
    h.member(org, o, "owner", "active").await;
    h.bind_ws(ws, o, "owner").await;
    let (_, t) = h.good_session(o).await;
    let mut agents = Vec::new();
    for handle in ["one", "two", "three"] {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES ($1,$2,$3,$4,$4,'byo','medium','')")
            .bind(id).bind(org).bind(ws).bind(handle).execute(h.pool()).await.unwrap();
        agents.push(id);
    }
    World {
        h,
        org,
        ws,
        owner: cookie(&t),
        agents,
    }
}

#[tokio::test]
async fn a_list_costs_the_same_number_of_queries_for_two_accounts_or_many() {
    let w = world().await;
    w.add_accounts(1).await;
    // The first request warms caches (session, snapshot); count from the second.
    w.list().await;
    let (small, n_small) = w.queries().await;
    assert_eq!(n_small, 4);
    assert!(small > 0, "the counter must see the list's queries");
    w.add_accounts(9).await;
    let (big, n_big) = w.queries().await;
    assert_eq!(n_big, 40);
    assert_eq!(
        small, big,
        "the number of queries must not grow with the accounts"
    );
}
