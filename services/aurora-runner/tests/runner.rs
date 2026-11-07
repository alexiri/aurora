#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use aurora_config::OrphanReap;
use aurora_protocol::runner::{
    DesiredState, RegisterResponse, ReportBody, ReportStatus, ReportsRequest, RunnerConfig,
    SandboxAssignment, SandboxResources, SandboxSpec, SandboxStart, SandboxState, SandboxStop,
    SandboxesResponse, StopReason, WorkItem, WorkItemBody, WorkResponse,
};
use aurora_runner::api::{ApiClient, RetryPolicy};
use aurora_runner::backend::{
    BackendError, LABEL_INSTALLATION, LABEL_RUNNER, ManagedObject, ObjectKind,
};
use aurora_runner::backends::fake::FakeBackend;
use aurora_runner::executor::{Executor, ExecutorSettings};
use aurora_runner::hub::{Frame, HubEvent, StubHub};
use aurora_runner::reconcile::{ReconcileCtx, reconcile_once};
use aurora_runner::reporter;
use aurora_runner::runner::installation_fingerprint;
use aurora_runner::{Runner, RunnerSettings};
use chrono::Utc;
use serde_json::{Value, json};
use tokio::sync::watch;
use uuid::Uuid;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const TOKEN: &str = "aur_testtoken";

fn fast_retry() -> RetryPolicy {
    RetryPolicy {
        attempts: 2,
        initial: Duration::from_millis(5),
        max: Duration::from_millis(20),
    }
}

fn settings(url: &str) -> RunnerSettings {
    RunnerSettings {
        api_url: url.to_owned(),
        token: TOKEN.into(),
        version: "test".into(),
        advertised_host: "runner.test".into(),
        orphan_reap: OrphanReap::Reap,
        retry: fast_retry(),
        executor: ExecutorSettings {
            max_concurrency: 4,
            shutdown_flush: Duration::from_millis(100),
            image: None,
        },
        drain_timeout: Duration::from_secs(5),
        min_empty_poll: Duration::from_millis(20),
    }
}

fn register_response(runner_id: Uuid, interval: u32) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(RegisterResponse {
        runner_id,
        replica_id: Uuid::new_v4(),
        protocol: 1,
        config: RunnerConfig {
            poll_wait_secs: 1,
            reconcile_interval_secs: interval,
            orphan_grace_secs: 3600,
            idle_stop_secs: 900,
        },
    })
}

fn start_item(sandbox: Uuid, boot: Uuid) -> WorkItem {
    WorkItem {
        item_id: Uuid::new_v4(),
        sandbox_id: sandbox,
        boot_id: boot,
        body: WorkItemBody::SandboxStart(SandboxStart {
            spec: SandboxSpec {
                image: "aurora-sandbox:test".into(),
                env: vec![],
                boot_secret: aurora_protocol::common::SecretString::new("secret"),
                home_ref: format!("home-{sandbox}"),
                home_gib: 1,
                resources: SandboxResources {
                    cpu_millis: 1000,
                    memory_mib: 512,
                    pids: 128,
                },
                labels: Default::default(),
            },
        }),
    }
}

fn stop_item(sandbox: Uuid, boot: Uuid) -> WorkItem {
    WorkItem {
        item_id: Uuid::new_v4(),
        sandbox_id: sandbox,
        boot_id: boot,
        body: WorkItemBody::SandboxStop(SandboxStop {
            reason: StopReason::Idle,
            grace_secs: 1,
        }),
    }
}

/// Answers every posted report with `status` (and records nothing itself).
struct ReportsAnswer(ReportStatus);
impl Respond for ReportsAnswer {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: ReportsRequest = serde_json::from_slice(&req.body).unwrap();
        let results: Vec<Value> = body
            .reports
            .iter()
            .map(|r| json!({ "report_id": r.report_id, "status": self.0.as_str() }))
            .collect();
        ResponseTemplate::new(200).set_body_json(json!({ "results": results }))
    }
}

async fn mount_basics(
    server: &MockServer,
    runner_id: Uuid,
    status: ReportStatus,
    items: Vec<Vec<WorkItem>>,
) {
    mount_full(server, runner_id, status, items, false, 3600).await;
}

async fn mount_full(
    server: &MockServer,
    runner_id: Uuid,
    status: ReportStatus,
    items: Vec<Vec<WorkItem>>,
    reconciler: bool,
    interval: u32,
) {
    Mock::given(method("POST"))
        .and(path("/v1/runner/register"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .and(header("x-aurora-protocol", "runner/1"))
        .respond_with(register_response(runner_id, interval))
        .mount(server)
        .await;
    for batch in items {
        Mock::given(method("POST"))
            .and(path("/v1/runner/work"))
            .respond_with(ResponseTemplate::new(200).set_body_json(WorkResponse {
                items: batch,
                reconciler,
            }))
            .up_to_n_times(1)
            .mount(server)
            .await;
    }
    Mock::given(method("POST"))
        .and(path("/v1/runner/work"))
        .respond_with(ResponseTemplate::new(200).set_body_json(WorkResponse {
            items: vec![],
            reconciler,
        }))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/runner/reports"))
        .respond_with(ReportsAnswer(status))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/runner/retire"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
        .mount(server)
        .await;
}

async fn posted_reports(server: &MockServer) -> Vec<aurora_protocol::runner::Report> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == "/v1/runner/reports")
        .flat_map(|r| {
            serde_json::from_slice::<ReportsRequest>(&r.body)
                .unwrap()
                .reports
        })
        .collect()
}

async fn wait_for<F: Fn() -> bool>(f: F) {
    for _ in 0..300 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("condition not reached");
}

async fn wait_reports(server: &MockServer, n: usize) -> Vec<aurora_protocol::runner::Report> {
    for _ in 0..300 {
        let r = posted_reports(server).await;
        if r.len() >= n {
            return r;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("expected {n} reports");
}

fn states(reports: &[aurora_protocol::runner::Report]) -> Vec<SandboxState> {
    reports
        .iter()
        .filter_map(|r| match &r.body {
            ReportBody::SandboxStatus(s) => Some(s.state.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn registers_executes_start_and_stop_reports_and_retires() {
    let server = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_basics(
        &server,
        Uuid::new_v4(),
        ReportStatus::Applied,
        vec![
            vec![start_item(sandbox, boot)],
            vec![stop_item(sandbox, boot)],
        ],
    )
    .await;
    let backend = Arc::new(FakeBackend::new(4));
    let (tx, rx) = watch::channel(false);
    let runner = Runner::new(
        settings(&server.uri()),
        backend.clone(),
        Arc::new(StubHub::default()),
    );
    let task = tokio::spawn(runner.run(rx));

    let reports = wait_reports(&server, 2).await;
    assert_eq!(
        states(&reports),
        vec![SandboxState::Running, SandboxState::Stopped]
    );
    match &reports[0].body {
        ReportBody::SandboxStatus(s) => assert!(s.container_ref.is_some()),
        _ => panic!(),
    }
    assert_eq!((reports[0].sandbox_id, reports[0].boot_id), (sandbox, boot));
    assert_eq!(
        backend.log(),
        vec![
            format!("provision:{sandbox}"),
            format!("start:{sandbox}"),
            format!("stop:{sandbox}")
        ]
    );

    tx.send(true).unwrap();
    task.await.unwrap().unwrap();

    let reqs = server.received_requests().await.unwrap();
    let register = reqs
        .iter()
        .find(|r| r.url.path().ends_with("/register"))
        .unwrap();
    assert!(register.headers.get("x-aurora-replica").is_none());
    let work = reqs
        .iter()
        .find(|r| r.url.path().ends_with("/work"))
        .unwrap();
    assert!(work.headers.get("x-aurora-replica").is_some());
    let body: Value = serde_json::from_slice(&work.body).unwrap();
    assert_eq!(body["wait_secs"], 1);
    assert_eq!(body["capacity"]["max"], 4);
    assert!(reqs.iter().any(|r| r.url.path().ends_with("/retire")));
}

#[tokio::test]
async fn failed_start_maps_reason_codes_and_hold_is_reported() {
    let server = MockServer::start().await;
    let (s1, s2, b) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    mount_basics(
        &server,
        Uuid::new_v4(),
        ReportStatus::Applied,
        vec![vec![start_item(s1, b)], vec![start_item(s2, b)]],
    )
    .await;
    let backend = Arc::new(FakeBackend::new(4));
    backend.fail_next_start(BackendError::ImageUnavailable("nope\nline".into()));
    let (tx, rx) = watch::channel(false);
    let task = tokio::spawn(
        Runner::new(
            settings(&server.uri()),
            backend.clone(),
            Arc::new(StubHub::default()),
        )
        .run(rx),
    );
    let reports = wait_reports(&server, 1).await;
    match &reports[0].body {
        ReportBody::SandboxStatus(s) => {
            assert_eq!(s.state, SandboxState::Failed);
            assert_eq!(
                s.reason_code.as_ref().unwrap().as_str(),
                "image_unavailable"
            );
            assert!(!s.message.as_deref().unwrap().contains('\n'));
        }
        _ => panic!(),
    }
    tx.send(true).unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn duplicate_and_fenced_results_are_not_retried() {
    for status in [ReportStatus::Duplicate, ReportStatus::Fenced] {
        let server = MockServer::start().await;
        let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
        mount_basics(
            &server,
            Uuid::new_v4(),
            status,
            vec![vec![start_item(sandbox, boot)]],
        )
        .await;
        let (tx, rx) = watch::channel(false);
        let task = tokio::spawn(
            Runner::new(
                settings(&server.uri()),
                Arc::new(FakeBackend::new(4)),
                Arc::new(StubHub::default()),
            )
            .run(rx),
        );
        wait_reports(&server, 1).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(posted_reports(&server).await.len(), 1, "no re-post");
        tx.send(true).unwrap();
        task.await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn supervisor_frames_become_reports_with_deterministic_ids() {
    let server = MockServer::start().await;
    mount_basics(&server, Uuid::new_v4(), ReportStatus::Applied, vec![]).await;
    let hub = Arc::new(StubHub::default());
    let (tx, rx) = watch::channel(false);
    let task = tokio::spawn(
        Runner::new(
            settings(&server.uri()),
            Arc::new(FakeBackend::new(4)),
            hub.clone(),
        )
        .run(rx),
    );
    let (sandbox, boot, turn) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let ev = |seq, kind: &str, body: Value| HubEvent {
        sandbox_id: sandbox,
        boot_id: boot,
        seq,
        frame: Frame {
            kind: kind.into(),
            body,
        },
    };
    hub.emit(ev(
        7,
        "turn.event",
        json!({ "turn_id": turn, "n": 3, "event": { "kind": "text_delta", "text": "hi" } }),
    ))
    .await;
    hub.emit(ev(
        7,
        "turn.event",
        json!({ "turn_id": turn, "n": 3, "event": { "kind": "text_delta", "text": "hi" } }),
    ))
    .await;
    hub.emit(ev(8, "turn.progress", json!({ "turn_id": turn })))
        .await;
    hub.emit(ev(9, "mystery", json!({}))).await;
    let reports = wait_reports(&server, 3).await;
    assert_eq!(
        reports[0].report_id,
        aurora_protocol::runner::frame_report_id(boot, 7)
    );
    assert_eq!(reports[0].report_id, reports[1].report_id);
    assert_ne!(reports[0].report_id, reports[2].report_id);
    assert!(matches!(&reports[0].body, ReportBody::TurnEvents(e) if e.events[0].n == 3));
    tx.send(true).unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn executor_serializes_per_sandbox_and_forwards_socket_work() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/runner/reports"))
        .respond_with(ReportsAnswer(ReportStatus::Applied))
        .mount(&server)
        .await;
    let api = Arc::new(ApiClient::new(&server.uri(), TOKEN, fast_retry()).unwrap());
    let (_tx, rx) = watch::channel(false);
    let (sink, _task) = reporter::spawn(api, rx);
    let backend = Arc::new(FakeBackend::new(4));
    backend.set_delays(Duration::from_millis(60), Duration::from_millis(60));
    let hub = Arc::new(StubHub::default());
    let exec = Executor::new(
        backend.clone(),
        hub.clone(),
        sink,
        ExecutorSettings::default(),
    );
    let (a, b, boot) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    exec.submit(start_item(a, boot));
    exec.submit(stop_item(a, boot));
    exec.submit(start_item(a, Uuid::new_v4()));
    exec.submit(start_item(b, boot));
    assert!(exec.is_busy(a));
    assert!(exec.drain(Duration::from_secs(5)).await);
    assert_eq!(backend.max_concurrent(a), 1);
    let ops: Vec<String> = backend
        .log()
        .into_iter()
        .filter(|l| l.ends_with(&a.to_string()) && !l.starts_with("prov"))
        .collect();
    assert_eq!(
        ops,
        vec![
            format!("start:{a}"),
            format!("stop:{a}"),
            format!("start:{a}")
        ]
    );
    assert_eq!(exec.running_count(), 2);

    // Socket-bound work goes to the hub in order; not-connected is not fatal.
    hub.set_connected(a, true);
    let turn = Uuid::new_v4();
    for kind in [
        WorkItemBody::TurnAbort(aurora_protocol::runner::TurnRef { turn_id: turn }),
        WorkItemBody::TurnSendNow(aurora_protocol::runner::TurnRef { turn_id: turn }),
    ] {
        exec.submit(WorkItem {
            item_id: Uuid::new_v4(),
            sandbox_id: a,
            boot_id: boot,
            body: kind,
        });
    }
    exec.submit(WorkItem {
        item_id: Uuid::new_v4(),
        sandbox_id: b,
        boot_id: boot,
        body: WorkItemBody::TurnAbort(aurora_protocol::runner::TurnRef { turn_id: turn }),
    });
    assert!(exec.drain(Duration::from_secs(5)).await);
    let sent = hub.sent();
    let kinds: Vec<&str> = sent.iter().map(|(_, f)| f.kind.as_str()).collect();
    assert_eq!(kinds, vec!["turn.abort", "turn.send_now"]);
    assert_eq!(sent[0].1.body, json!({ "turn_id": turn }));
    wait_for(|| true).await;
}

#[allow(clippy::too_many_arguments)]
fn obj(
    kind: ObjectKind,
    name: &str,
    sandbox: Option<Uuid>,
    boot: Option<Uuid>,
    runner: Option<Uuid>,
    inst: &str,
    age_secs: i64,
    running: bool,
) -> ManagedObject {
    ManagedObject {
        kind,
        reference: name.into(),
        sandbox_id: sandbox,
        boot_id: boot,
        runner: runner.map(|r| r.to_string()),
        installation: Some(inst.into()),
        created_at: Utc::now() - chrono::Duration::seconds(age_secs),
        running,
    }
}

struct World {
    server: MockServer,
    api: ApiClient,
    backend: FakeBackend,
    exec: Executor,
    runner_id: Uuid,
    fp: String,
    live: Uuid,
    live_boot: Uuid,
}

async fn world(check_status: u16) -> World {
    let server = MockServer::start().await;
    let (live, live_boot) = (Uuid::new_v4(), Uuid::new_v4());
    let assignment = SandboxAssignment {
        sandbox_id: live,
        agent_id: Uuid::new_v4(),
        boot_id: Some(live_boot),
        desired: DesiredState::Running,
        home_ref: "h".into(),
    };
    Mock::given(method("GET"))
        .and(path("/v1/runner/sandboxes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(SandboxesResponse {
            sandboxes: vec![assignment],
        }))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/runner/sandboxes/check"))
        .respond_with(ResponseTemplate::new(check_status).set_body_json(json!({ "known": [] })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/runner/strays"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;
    let api = ApiClient::new(&server.uri(), TOKEN, fast_retry()).unwrap();
    let backend = FakeBackend::new(4);
    let (_t, rx) = watch::channel(false);
    let (sink, _) = reporter::spawn(
        Arc::new(ApiClient::new(&server.uri(), TOKEN, fast_retry()).unwrap()),
        rx,
    );
    let exec = Executor::new(
        Arc::new(FakeBackend::new(1)),
        Arc::new(StubHub::default()),
        sink,
        ExecutorSettings::default(),
    );
    World {
        server,
        api,
        backend,
        exec,
        runner_id: Uuid::new_v4(),
        fp: installation_fingerprint(TOKEN),
        live,
        live_boot,
    }
}

fn plant(w: &World) {
    let (gone, other_runner) = (Uuid::new_v4(), Uuid::new_v4());
    let r = Some(w.runner_id);
    let fp = w.fp.as_str();
    // orphan of this runner, old / young
    w.backend.insert_object(obj(
        ObjectKind::Container,
        "old-orphan",
        Some(gone),
        Some(Uuid::new_v4()),
        r,
        fp,
        7200,
        true,
    ));
    w.backend.insert_object(obj(
        ObjectKind::Container,
        "young-orphan",
        Some(Uuid::new_v4()),
        None,
        r,
        fp,
        10,
        true,
    ));
    // stale boot of a live sandbox
    w.backend.insert_object(obj(
        ObjectKind::Container,
        "stale-boot",
        Some(w.live),
        Some(Uuid::new_v4()),
        r,
        fp,
        7200,
        true,
    ));
    // current boot: kept
    w.backend.insert_object(obj(
        ObjectKind::Container,
        "current",
        Some(w.live),
        Some(w.live_boot),
        r,
        fp,
        7200,
        true,
    ));
    // other runner of this installation, unknown sandbox
    w.backend.insert_object(obj(
        ObjectKind::Container,
        "sweep-orphan",
        Some(Uuid::new_v4()),
        None,
        Some(other_runner),
        fp,
        7200,
        true,
    ));
    // other installation: never touched
    w.backend.insert_object(obj(
        ObjectKind::Container,
        "foreign",
        Some(Uuid::new_v4()),
        None,
        Some(other_runner),
        "ffffffffffffffff",
        99999,
        true,
    ));
    let _ = LABEL_INSTALLATION;
    let _ = LABEL_RUNNER;
}

fn ctx(w: &World, mode: OrphanReap) -> ReconcileCtx<'_> {
    ReconcileCtx {
        runner_id: w.runner_id,
        installation: &w.fp,
        mode,
        grace: Duration::from_secs(3600),
        now: Utc::now(),
    }
}

#[tokio::test]
async fn reconcile_reaps_orphans_after_grace_in_reap_mode() {
    let w = world(200).await;
    plant(&w);
    let out = reconcile_once(&w.api, &w.backend, &w.exec, &ctx(&w, OrphanReap::Reap))
        .await
        .unwrap();
    let mut d = out.destroyed.clone();
    d.sort();
    assert_eq!(d, vec!["old-orphan", "stale-boot", "sweep-orphan"]);
    assert_eq!(w.backend.destroyed().len(), 3);
    let _ = &w.server;
}

#[tokio::test]
async fn reconcile_only_logs_in_log_only_mode() {
    let w = world(200).await;
    plant(&w);
    let out = reconcile_once(&w.api, &w.backend, &w.exec, &ctx(&w, OrphanReap::LogOnly))
        .await
        .unwrap();
    assert!(out.destroyed.is_empty());
    assert_eq!(out.would_destroy.len(), 3);
    assert!(w.backend.destroyed().is_empty());
}

#[tokio::test]
async fn reconcile_api_error_aborts_with_zero_destruction() {
    let w = world(500).await;
    plant(&w);
    let r = reconcile_once(&w.api, &w.backend, &w.exec, &ctx(&w, OrphanReap::Reap)).await;
    assert!(r.is_err());
    assert!(w.backend.destroyed().is_empty());
}

#[test]
fn fingerprint_is_16_hex() {
    let f = installation_fingerprint(TOKEN);
    assert_eq!(f.len(), 16);
    assert!(f.chars().all(|c| c.is_ascii_hexdigit()));
}

async fn reconcile_run(reconciler: Option<bool>) -> Vec<String> {
    let server = MockServer::start().await;
    let runner_id = Uuid::new_v4();
    mount_full(
        &server,
        runner_id,
        ReportStatus::Applied,
        vec![],
        reconciler == Some(true),
        1,
    )
    .await;
    if reconciler.is_none() {
        // Response without the field at all.
        server.reset().await;
        Mock::given(method("POST"))
            .and(path("/v1/runner/register"))
            .respond_with(register_response(runner_id, 1))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/runner/work"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": [] })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/runner/retire"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/v1/runner/sandboxes"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(SandboxesResponse { sandboxes: vec![] }),
        )
        .mount(&server)
        .await;
    let backend = Arc::new(FakeBackend::new(4));
    let fp = installation_fingerprint(TOKEN);
    backend.insert_object(obj(
        ObjectKind::Container,
        "orphan",
        Some(Uuid::new_v4()),
        None,
        Some(runner_id),
        &fp,
        7200,
        false,
    ));
    let (tx, rx) = watch::channel(false);
    let task = tokio::spawn(
        Runner::new(
            settings(&server.uri()),
            backend.clone(),
            Arc::new(StubHub::default()),
        )
        .run(rx),
    );
    tokio::time::sleep(Duration::from_millis(2600)).await;
    tx.send(true).unwrap();
    task.await.unwrap().unwrap();
    backend.destroyed()
}

#[tokio::test]
async fn reconciles_only_when_work_response_says_reconciler() {
    assert_eq!(reconcile_run(Some(true)).await, vec!["orphan"]);
    assert!(reconcile_run(Some(false)).await.is_empty());
    assert!(reconcile_run(None).await.is_empty());
}
