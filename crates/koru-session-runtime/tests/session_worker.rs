// SPDX-License-Identifier: MPL-2.0

use std::ffi::OsString;
use std::path::Path;
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use koru_session_observability::SessionTrace;
use koru_session_protocol::{
    BuiltInProfileKey, CapabilitySet, GridSize, ProfileKind, RenderUpdate, SessionEvent,
    SessionProfileKey, ViewportGeneration,
};
use koru_session_runtime::providers::ManagedProviderRegistry;
use koru_session_runtime::{
    NoopLifecycleSink, NoopResourceLifecycle, SessionCreate, SessionEventUpdate, SessionLaunch,
    SessionLifecycleSink, SessionRegistry, SessionRuntimeConfig, SpawnRequest,
};

struct CountEvents(AtomicUsize);

impl SessionLifecycleSink for CountEvents {
    fn publish(&self, _event: koru_events::Draft) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn size() -> GridSize {
    GridSize::new(80, 24).unwrap()
}

#[cfg(windows)]
fn command(script: &str, cwd: &Path) -> SpawnRequest {
    SpawnRequest::new(
        [
            OsString::from("powershell.exe"),
            OsString::from("-NoLogo"),
            OsString::from("-NoProfile"),
            OsString::from("-NonInteractive"),
            OsString::from("-Command"),
            OsString::from(script),
        ],
        cwd.to_path_buf(),
        size(),
    )
}

#[cfg(unix)]
fn command(script: &str, cwd: &Path) -> SpawnRequest {
    SpawnRequest::new(
        [
            OsString::from("/bin/bash"),
            OsString::from("-c"),
            OsString::from(script),
        ],
        cwd.to_path_buf(),
        size(),
    )
}

fn create(root: &Path, spawn: SpawnRequest) -> SessionCreate {
    SessionCreate {
        profile: SessionProfileKey::BuiltIn(BuiltInProfileKey::from_str("platform-shell").unwrap()),
        parent_session_id: None,
        profile_kind: ProfileKind::Shell,
        display_name: "Integration shell".to_owned(),
        working_directory: root.to_path_buf(),
        attachment_delivery: koru_session_protocol::AttachmentDeliveryPolicy::Unsupported,
        capabilities: CapabilitySet::default(),
        launch: SessionLaunch::Raw(spawn),
        created_at_ms: 1,
        origin: Default::default(),
    }
}

fn registry() -> SessionRegistry {
    SessionRegistry::new(SessionRuntimeConfig {
        stats_enabled: true,
        trace: SessionTrace::new(false),
        lifecycle: std::sync::Arc::new(NoopLifecycleSink),
        resources: std::sync::Arc::new(NoopResourceLifecycle),
        attachment_paths: std::sync::Arc::new(koru_session_runtime::NoopAttachmentPathResolver),
        managed_providers: std::sync::Arc::new(ManagedProviderRegistry::default()),
    })
}

fn eventually(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(Instant::now() < deadline, "real PTY did not settle");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn real_pty_output_becomes_a_bounded_render_delta_then_exits() {
    let root = tempfile::tempdir().unwrap();
    #[cfg(windows)]
    let spawn = command(
        "Start-Sleep -Milliseconds 100; Write-Output 'KORU:WORKER'; Start-Sleep -Milliseconds 50",
        root.path(),
    );
    #[cfg(unix)]
    let spawn = command("sleep .1; printf 'KORU:WORKER\\n'; sleep .05", root.path());
    let registry = registry();
    let handle = registry.create(create(root.path(), spawn)).unwrap();
    let events = handle.subscribe_events();
    let attached = handle
        .attach_live(ViewportGeneration::new(1).unwrap(), 80, 2, 1)
        .unwrap();
    assert!(matches!(
        attached.render.recv_timeout(Duration::from_secs(2)),
        Ok(RenderUpdate::Snapshot { .. })
    ));
    // A shell may negotiate focus/mouse modes before it prints its marker.
    // Those are valid mode-only deltas, so wait for the first bounded update
    // which actually carries rows rather than assigning text semantics to the
    // first event after attach.
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let update = attached
            .render
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap();
        match update {
            RenderUpdate::Delta { delta } if !delta.rows.is_empty() => {
                assert!(delta.rows.len() <= 28);
                break;
            }
            RenderUpdate::Snapshot { snapshot } if !snapshot.rows.is_empty() => {
                assert!(snapshot.rows.len() <= 28);
                break;
            }
            RenderUpdate::Delta { .. } | RenderUpdate::Snapshot { .. } => {
                assert!(Instant::now() < deadline, "PTY emitted no text rows");
            }
            RenderUpdate::Gap { .. } => panic!("a single output batch cannot gap"),
        }
    }
    eventually(|| handle.is_ended());
    let mut saw_exit = false;
    while let Ok(event) = events.recv_timeout(Duration::from_millis(10)) {
        saw_exit |= matches!(
            event,
            SessionEventUpdate::Event(koru_session_protocol::SequencedSessionEvent {
                event: SessionEvent::Exited { .. },
                ..
            })
        );
    }
    assert!(saw_exit);
    registry.reap();
    assert!(registry.list().is_empty());
}

#[test]
fn a_real_process_survives_detach_and_only_terminate_ends_it() {
    let root = tempfile::tempdir().unwrap();
    #[cfg(windows)]
    let spawn = command("Start-Sleep -Seconds 30", root.path());
    #[cfg(unix)]
    let spawn = command("sleep 30", root.path());
    let registry = registry();
    let handle = registry.create(create(root.path(), spawn)).unwrap();
    let first = handle
        .attach_live(ViewportGeneration::new(1).unwrap(), 80, 0, 1)
        .unwrap();
    handle.detach(first.lease.token).unwrap();
    assert_eq!(registry.list().len(), 1);
    assert!(!handle.is_ended());
    let second = handle
        .attach_live(ViewportGeneration::new(2).unwrap(), 80, 0, 2)
        .unwrap();
    handle.terminate(second.lease.token).unwrap();
    eventually(|| handle.is_ended());
    registry.reap();
    assert!(registry.list().is_empty());
}

#[test]
fn failed_spawn_creates_neither_a_registry_row_nor_a_lifecycle_fact() {
    let root = tempfile::tempdir().unwrap();
    let events = std::sync::Arc::new(CountEvents(AtomicUsize::new(0)));
    let registry = SessionRegistry::new(SessionRuntimeConfig {
        stats_enabled: true,
        trace: SessionTrace::new(false),
        lifecycle: events.clone(),
        resources: std::sync::Arc::new(NoopResourceLifecycle),
        attachment_paths: std::sync::Arc::new(koru_session_runtime::NoopAttachmentPathResolver),
        managed_providers: std::sync::Arc::new(ManagedProviderRegistry::default()),
    });
    let spawn = SpawnRequest::new(
        ["koru-definitely-missing-session-executable"],
        root.path().to_path_buf(),
        size(),
    );
    let error = match registry.create(create(root.path(), spawn)) {
        Ok(_) => panic!("missing executable started"),
        Err(error) => error,
    };
    assert_eq!(error.code, koru_session_protocol::ErrorCode::SpawnFailed);
    assert!(registry.list().is_empty());
    assert_eq!(events.0.load(Ordering::Relaxed), 0);
}
