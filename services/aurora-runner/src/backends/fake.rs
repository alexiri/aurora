//! In-memory backend: simulates homes, boots, crashes, capacity holds and image errors.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use aurora_protocol::runner::{
    HarnessId, HomeDurability, Observed, Range, ResourceRanges, RunnerCapabilities,
};
use chrono::Utc;
use uuid::Uuid;

use crate::backend::{
    Backend, BackendError, ContainerRef, HoldCallback, LABEL_BOOT, LABEL_INSTALLATION,
    LABEL_RUNNER, LABEL_SANDBOX, ManagedObject, ObjectKind, Observation, StartRequest,
};

#[derive(Debug, Clone)]
struct Container {
    reference: String,
    boot_id: Uuid,
    running: bool,
    created: chrono::DateTime<Utc>,
    labels: std::collections::BTreeMap<String, String>,
}

#[derive(Default)]
struct State {
    homes: HashSet<String>,
    containers: HashMap<Uuid, Container>,
    extra: Vec<ManagedObject>,
    log: Vec<String>,
    destroyed: Vec<String>,
    fail_start: VecDeque<BackendError>,
    hold_next: Option<Duration>,
    start_delay: Duration,
    stop_delay: Duration,
    active: HashMap<Uuid, usize>,
    max_active: HashMap<Uuid, usize>,
}

pub struct FakeBackend {
    max_sandboxes: u32,
    st: Mutex<State>,
}

/// Decrements the per-sandbox activity counter on drop.
struct Activity<'a>(&'a FakeBackend, Uuid);
impl Drop for Activity<'_> {
    fn drop(&mut self) {
        self.0.with(|s| {
            if let Some(n) = s.active.get_mut(&self.1) {
                *n = n.saturating_sub(1);
            }
        });
    }
}

impl FakeBackend {
    pub fn new(max_sandboxes: u32) -> Self {
        Self {
            max_sandboxes,
            st: Mutex::default(),
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        let mut g = self
            .st
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut g)
    }

    fn enter(&self, sandbox: Uuid, what: &str) -> Activity<'_> {
        self.with(|s| {
            s.log.push(format!("{what}:{sandbox}"));
            let n = s.active.entry(sandbox).or_default();
            *n += 1;
            let n = *n;
            let m = s.max_active.entry(sandbox).or_default();
            *m = (*m).max(n);
        });
        Activity(self, sandbox)
    }

    // ---- scripting (tests / dev)

    pub fn set_delays(&self, start: Duration, stop: Duration) {
        self.with(|s| {
            s.start_delay = start;
            s.stop_delay = stop;
        });
    }

    /// The next `start` fails with `err`.
    pub fn fail_next_start(&self, err: BackendError) {
        self.with(|s| s.fail_start.push_back(err));
    }

    /// The next `start` calls the hold callback, then waits `d` before proceeding.
    pub fn hold_next_start(&self, d: Duration) {
        self.with(|s| s.hold_next = Some(d));
    }

    /// Simulate a container exiting on its own.
    pub fn crash(&self, sandbox_id: Uuid) {
        self.with(|s| {
            if let Some(c) = s.containers.get_mut(&sandbox_id) {
                c.running = false;
            }
        });
    }

    /// Plant an arbitrary object (orphans, other runners, other installations).
    pub fn insert_object(&self, obj: ManagedObject) {
        self.with(|s| s.extra.push(obj));
    }

    /// Operation log: `start:<sandbox>`, `stop:<sandbox>`, `provision:<sandbox>`.
    pub fn log(&self) -> Vec<String> {
        self.with(|s| s.log.clone())
    }

    /// References passed to `destroy`.
    pub fn destroyed(&self) -> Vec<String> {
        self.with(|s| s.destroyed.clone())
    }

    /// Highest number of overlapping operations seen for the sandbox.
    pub fn max_concurrent(&self, sandbox_id: Uuid) -> usize {
        self.with(|s| s.max_active.get(&sandbox_id).copied().unwrap_or(0))
    }
}

#[async_trait]
impl Backend for FakeBackend {
    fn id(&self) -> &'static str {
        "fake"
    }

    fn capabilities(&self) -> RunnerCapabilities {
        RunnerCapabilities {
            backend: "fake".into(),
            home_durability: HomeDurability::Resident,
            resources: ResourceRanges {
                cpu: Range {
                    min: 250,
                    max: 4000,
                },
                memory_mib: Range {
                    min: 256,
                    max: 8192,
                },
                pids: Range { min: 64, max: 4096 },
                home_gib: Range { min: 1, max: 100 },
            },
            max_sandboxes: self.max_sandboxes,
            harnesses: vec![HarnessId::Fake],
            max_attachment_bytes: 25 * 1024 * 1024,
            // Filled in by the runner from `runner.image` at registration.
            image: None,
            held_awake_ceiling: None,
            defaults: None,
            image_capabilities: vec![],
        }
    }

    async fn provision_home(
        &self,
        sandbox_id: Uuid,
        home_ref: &str,
        _home_gib: u32,
    ) -> Result<(), BackendError> {
        self.with(|s| {
            s.log.push(format!("provision:{sandbox_id}"));
            s.homes.insert(home_ref.to_owned());
        });
        Ok(())
    }

    async fn start(
        &self,
        req: &StartRequest,
        hold: &dyn HoldCallback,
    ) -> Result<ContainerRef, BackendError> {
        let _act = self.enter(req.sandbox_id, "start");
        let (delay, hold_for, fail) =
            self.with(|s| (s.start_delay, s.hold_next.take(), s.fail_start.pop_front()));
        if let Some(d) = hold_for {
            hold.on_hold();
            tokio::time::sleep(d).await;
        }
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        if let Some(e) = fail {
            return Err(e);
        }
        self.with(|s| {
            if let Some(c) = s.containers.get(&req.sandbox_id)
                && c.boot_id == req.boot_id
                && c.running
            {
                return Ok(ContainerRef(c.reference.clone()));
            }
            // Other boots of the same sandbox are removed first.
            let running = s.containers.values().filter(|c| c.running).count() as u32;
            let replaces = s.containers.get(&req.sandbox_id).is_some_and(|c| c.running);
            if running - u32::from(replaces) >= self.max_sandboxes {
                return Err(BackendError::Capacity("fake backend is full".into()));
            }
            let reference = format!("fake-{}-{}", req.sandbox_id.simple(), req.boot_id.simple());
            let mut labels = req.spec.labels.clone();
            labels.insert(LABEL_SANDBOX.into(), req.sandbox_id.to_string());
            labels.insert(LABEL_BOOT.into(), req.boot_id.to_string());
            s.containers.insert(
                req.sandbox_id,
                Container {
                    reference: reference.clone(),
                    boot_id: req.boot_id,
                    running: true,
                    created: Utc::now(),
                    labels,
                },
            );
            Ok(ContainerRef(reference))
        })
    }

    async fn stop(&self, container: &ContainerRef, _grace: Duration) -> Result<(), BackendError> {
        let sandbox = self.with(|s| {
            s.containers
                .iter()
                .find(|(_, c)| c.reference == container.0)
                .map(|(id, _)| *id)
        });
        let Some(sandbox) = sandbox else {
            return Ok(());
        };
        let _act = self.enter(sandbox, "stop");
        let delay = self.with(|s| s.stop_delay);
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        self.with(|s| s.containers.remove(&sandbox));
        Ok(())
    }

    async fn inspect(&self, sandbox_id: Uuid) -> Result<Option<Observation>, BackendError> {
        Ok(self.with(|s| {
            s.containers.get(&sandbox_id).map(|c| Observation {
                container: ContainerRef(c.reference.clone()),
                running: c.running,
                boot_id: Some(c.boot_id),
                phase: if c.running { "running" } else { "exited" }.into(),
                observed: c.running.then_some(Observed {
                    cpu_millis: 10,
                    memory_mib: 64,
                }),
            })
        }))
    }

    async fn list_managed(&self) -> Result<Vec<ManagedObject>, BackendError> {
        Ok(self.with(|s| {
            let mut v: Vec<_> = s
                .containers
                .iter()
                .map(|(id, c)| ManagedObject {
                    kind: ObjectKind::Container,
                    reference: c.reference.clone(),
                    sandbox_id: Some(*id),
                    boot_id: Some(c.boot_id),
                    runner: c.labels.get(LABEL_RUNNER).cloned(),
                    installation: c.labels.get(LABEL_INSTALLATION).cloned(),
                    created_at: c.created,
                    running: c.running,
                })
                .collect();
            v.extend(s.extra.iter().cloned());
            v
        }))
    }

    async fn destroy(&self, obj: &ManagedObject) -> Result<(), BackendError> {
        self.with(|s| {
            s.destroyed.push(obj.reference.clone());
            s.containers.retain(|_, c| c.reference != obj.reference);
            s.extra.retain(|o| o.reference != obj.reference);
            s.homes.remove(&obj.reference);
        });
        Ok(())
    }
}
