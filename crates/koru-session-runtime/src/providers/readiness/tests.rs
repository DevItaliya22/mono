// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::providers::{
    AgentCapabilities, CompatibilityProbe, DelegationFacts, ManagedProvider,
    ManagedProviderFactory, ManagedProviderRequest,
};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

struct ControlledFactory {
    probes: Mutex<VecDeque<mpsc::Receiver<Result<AgentCapabilities, ProviderFailure>>>>,
    started: mpsc::Sender<()>,
    calls: Arc<AtomicUsize>,
}

impl ManagedProviderFactory for ControlledFactory {
    fn create(&self, _: ManagedProviderRequest) -> Result<ManagedProvider, ProviderFailure> {
        unreachable!("readiness must not create a managed session")
    }
    fn compatibility_probe(
        &self,
        _: std::ffi::OsString,
        _: PathBuf,
    ) -> Result<Box<dyn CompatibilityProbe>, ProviderFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(ControlledProbe {
            receive: self.probes.lock().unwrap().pop_front().unwrap(),
            started: self.started.clone(),
        }))
    }
}

struct ControlledProbe {
    receive: mpsc::Receiver<Result<AgentCapabilities, ProviderFailure>>,
    started: mpsc::Sender<()>,
}
impl CompatibilityProbe for ControlledProbe {
    fn provider(&self) -> ProviderKind {
        "fixture".parse().unwrap()
    }
    fn raw_available(&self) -> bool {
        true
    }
    fn probe(&self, _: ProbeLimits) -> Result<AgentCapabilities, ProviderFailure> {
        self.started.send(()).unwrap();
        self.receive.recv_timeout(Duration::from_secs(5)).unwrap()
    }
}

type ProbeReplies = Vec<mpsc::Sender<Result<AgentCapabilities, ProviderFailure>>>;
type ReadinessFixture = (
    Arc<ProviderReadinessCache>,
    ProbeReplies,
    mpsc::Receiver<()>,
    Arc<AtomicUsize>,
);

fn setup(count: usize) -> ReadinessFixture {
    let (started, observed) = mpsc::channel();
    let mut sends = Vec::new();
    let mut receives = VecDeque::new();
    for _ in 0..count {
        let (send, receive) = mpsc::channel();
        sends.push(send);
        receives.push_back(receive);
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = ManagedProviderRegistry::default();
    registry
        .register(
            koru_session_profiles::ManagedProviderRegistration {
                kind: "fixture",
                ..koru_session_profiles::CODEX_REGISTRATION
            },
            Arc::new(ControlledFactory {
                probes: Mutex::new(receives),
                started,
                calls: calls.clone(),
            }),
        )
        .unwrap();
    (
        ProviderReadinessCache::new(Arc::new(registry), "fixture-cwd".into()),
        sends,
        observed,
        calls,
    )
}

fn compatible() -> AgentCapabilities {
    AgentCapabilities {
        provider: "fixture".parse().unwrap(),
        executable_version: "1".parse().unwrap(),
        protocol_version: "1".parse().unwrap(),
        capabilities: Default::default(),
        delegation: DelegationFacts::undeclared(),
    }
}

#[test]
fn cached_reads_and_a_cold_subscription_start_no_process_or_session() {
    let (cache, _, _, calls) = setup(0);
    let initial = cache.snapshot();
    let (snapshot, _) = cache.subscribe();
    assert_eq!(snapshot, initial);
    assert_eq!(initial[0].availability, ExecutableAvailability::Checking);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn slow_probe_does_not_hold_cache_reads_or_ordinary_host_work() {
    let (cache, sends, started, _) = setup(1);
    let (_, updates) = cache.subscribe();
    cache.refresh("fixture".parse().unwrap()).unwrap();
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(
        cache.snapshot()[0].availability,
        ExecutableAvailability::Checking
    );
    let (snapshot, _) = cache.subscribe();
    assert_eq!(snapshot.len(), 1);
    // Ordinary runtime catalog requests do not consult provider discovery.
    let registry = crate::SessionRegistry::new(crate::SessionRuntimeConfig::default());
    assert!(registry.catalog().1.is_empty());
    sends[0].send(Ok(compatible())).unwrap();
    assert_eq!(
        updates
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .availability,
        ExecutableAvailability::Checking
    );
    assert!(matches!(
        updates
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .availability,
        ExecutableAvailability::Available { .. }
    ));
}

#[test]
fn late_signed_out_completion_cannot_replace_a_refreshed_compatible_result() {
    let (cache, sends, started, _) = setup(2);
    let (_, updates) = cache.subscribe();
    cache.refresh("fixture".parse().unwrap()).unwrap();
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    cache.refresh("fixture".parse().unwrap()).unwrap();
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    updates.recv_timeout(Duration::from_secs(2)).unwrap();
    updates.recv_timeout(Duration::from_secs(2)).unwrap();
    sends[1].send(Ok(compatible())).unwrap();
    let accepted = updates.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(matches!(
        accepted.availability,
        ExecutableAvailability::Available { .. }
    ));
    sends[0]
        .send(Err(ProviderFailure::new(
            ErrorCode::ProviderUnavailable,
            ProviderStage::Initialize,
            ProviderDiagnostic::AuthenticationRequired,
        )))
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while cache
        .state
        .lock()
        .unwrap()
        .entries
        .values()
        .any(|entry| entry.active != 0)
    {
        assert!(
            std::time::Instant::now() < deadline,
            "old probe did not finish"
        );
        std::thread::yield_now();
    }
    assert_eq!(cache.snapshot(), vec![accepted]);
    assert!(updates.try_recv().is_err(), "stale result was published");
}

#[test]
fn discovery_failure_projection_keeps_stage_and_diagnostic() {
    let projected = crate::providers::ManagedAvailability::Failed {
        code: ErrorCode::ProviderUnavailable,
        stage: ProviderStage::Initialize,
        diagnostic: ProviderDiagnostic::Timeout,
    }
    .readiness("fixture".parse().unwrap());
    assert_eq!(
        projected.failure.unwrap(),
        koru_session_protocol::ProviderReadinessFailure {
            stage: ProviderStage::Initialize,
            diagnostic: ProviderDiagnostic::Timeout,
        }
    );
}

#[test]
fn disabled_probe_policy_settles_startup_and_explicit_refresh_without_factory_calls() {
    let (original, _, _, calls) = setup(0);
    let cache = ProviderReadinessCache::with_probe_policy(
        original.registry.clone(),
        "fixture-cwd".into(),
        false,
    );
    cache.refresh_all();
    cache.refresh("fixture".parse().unwrap()).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(
        cache
            .snapshot()
            .iter()
            .all(|row| matches!(row.availability, ExecutableAvailability::Failed { .. }))
    );
}

#[test]
fn timed_out_inner_probes_keep_their_slots_and_coalesced_retries_stay_finite() {
    let (mut cache, replies, started, calls) = setup(3);
    Arc::get_mut(&mut cache).unwrap().discovery_timeout = Duration::from_millis(40);
    let (_, updates) = cache.subscribe();
    let kind: ProviderKind = "fixture".parse().unwrap();
    for _ in 0..2 {
        cache.refresh(kind.clone()).unwrap();
        started.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(
            updates
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .availability,
            ExecutableAvailability::Checking
        ));
        let timed_out = updates.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(
            timed_out.failure.unwrap().diagnostic,
            ProviderDiagnostic::Timeout
        );
    }
    for _ in 0..20 {
        cache.refresh(kind.clone()).unwrap();
        let queued = updates.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(
            queued.availability,
            ExecutableAvailability::Failed { .. }
        ));
        assert_eq!(
            queued.failure.unwrap().diagnostic,
            ProviderDiagnostic::Backpressure
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(cache.state.lock().unwrap().entries[&kind].active, 2);
    }
    // Only an actual worker exit releases a slot for the newest queued retry.
    replies[0].send(Ok(compatible())).unwrap();
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    replies[2].send(Ok(compatible())).unwrap();
    assert!(matches!(
        updates
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .availability,
        ExecutableAvailability::Available { .. }
    ));
    replies[1].send(Ok(compatible())).unwrap();
}
