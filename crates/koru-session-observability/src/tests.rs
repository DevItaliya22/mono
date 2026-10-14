// SPDX-License-Identifier: MPL-2.0

use std::sync::{Arc, Mutex};

use koru_events::{
    AttrValue, Draft, EmitContext, EventId, SessionId as EventSessionId, TerminalDeliveryMode,
    TerminalDeliveryOutcome, TerminalInputKind, TerminalMode, TerminalProfileKind,
    TerminalProviderKind, TerminalSessionOutcome, validate,
};
use koru_session_protocol::{AttachmentId, SessionId};
use tracing::Subscriber;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt as _;

use super::*;

fn protocol_id<T: std::str::FromStr>(value: &str) -> T
where
    T::Err: std::fmt::Debug,
{
    value.parse().unwrap()
}

#[test]
fn disabled_stats_touch_nothing_and_return_no_snapshot() {
    let stats = SessionStats::new(false);
    stats.add(Counter::OutputBytes, u64::MAX);
    stats.observe_high_water(Counter::QueueHighWater, u64::MAX);
    stats.record_timing_micros(Timing::RenderCpu, u64::MAX);
    assert!(!stats.enabled());
    assert!(stats.snapshot().is_none());
}

#[test]
fn session_diagnostic_switches_are_in_the_canonical_registry() {
    assert!(koru_env::get(koru_env::name::KORU_SESSION_STATS).is_some());
    assert!(koru_env::get(koru_env::name::KORU_SESSION_DEBUG).is_some());
}

#[test]
fn enabled_stats_snapshot_exact_counters_timings_and_high_water() {
    let stats = SessionStats::new(true);
    stats.add(Counter::OutputBytes, 12);
    stats.add(Counter::OutputBytes, 8);
    stats.observe_high_water(Counter::QueueHighWater, 3);
    stats.observe_high_water(Counter::QueueHighWater, 2);
    stats.record_timing_micros(Timing::RenderCpu, 20);
    stats.record_timing_micros(Timing::RenderCpu, 30);
    let snapshot = stats.snapshot().unwrap();
    assert_eq!(snapshot.counter(Counter::OutputBytes), 20);
    assert_eq!(snapshot.counter(Counter::QueueHighWater), 3);
    assert_eq!(
        snapshot.timing(Timing::RenderCpu),
        TimingSnapshot {
            samples: 2,
            total_micros: 50,
            max_micros: 30,
        }
    );
}

#[test]
fn fixed_histogram_boundaries_and_cap_are_stable() {
    let mut histogram = FixedHistogram::default();
    for value in [0, 1, 2, 3, 4, 5, 5_000, 50_000] {
        histogram.observe(value);
    }
    assert_eq!(histogram.samples(), 8);
    assert_eq!(histogram.p95(), Some(5_000));
}

fn sample() -> ExperienceSample {
    ExperienceSample {
        first_present_ms: Some(73),
        switch_present_ms: Some(19),
        delta_present_ms: Some(9),
        render_cpu_ms: Some(4),
        janky: Some(true),
        stream_gaps: Some(1),
    }
}

#[test]
fn experience_uses_visible_time_has_a_floor_and_never_bursts() {
    let mut experience = ExperienceAggregator::default();
    assert!(
        experience
            .presented(EXPERIENCE_INTERVAL_MS - 1, sample())
            .is_none()
    );
    let first = experience
        .presented(1, sample())
        .expect("15 visible minutes");
    assert_eq!(first.first_present_ms_bucket, Some(50));
    assert_eq!(first.sample_count_bucket, Some(2));

    let one = experience.presented(EXPERIENCE_INTERVAL_MS * 3, sample());
    assert!(one.is_some(), "one existing sample may produce one summary");
    assert!(
        experience.presented(0, sample()).is_none(),
        "elapsed backlog is capped rather than producing an event burst"
    );
}

#[test]
fn experience_final_flush_requires_unsent_samples() {
    let mut experience = ExperienceAggregator::default();
    assert!(experience.flush().is_none());
    experience.presented(1, ExperienceSample::default());
    assert!(experience.flush().is_some());
    assert!(experience.flush().is_none());
}

#[test]
fn experience_does_not_fabricate_unmeasured_jank_or_gaps() {
    let mut experience = ExperienceAggregator::default();
    experience.presented(1, ExperienceSample::default());
    let summary = experience.flush().expect("one presentation sample");
    assert_eq!(summary.jank_per_thousand_bucket, None);
    assert_eq!(summary.stream_gap_count_bucket, None);
}

type CapturedEvent = (String, Vec<String>, String);

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<CapturedEvent>>>);

impl<S: Subscriber> Layer<S> for Capture {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        struct Values(String);
        impl tracing::field::Visit for Values {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                use std::fmt::Write as _;
                let _ = write!(self.0, "{}={value:?};", field.name());
            }
        }
        let metadata = event.metadata();
        let fields = metadata
            .fields()
            .iter()
            .map(|field| field.name().to_string())
            .collect();
        let mut values = Values(String::new());
        event.record(&mut values);
        self.0
            .lock()
            .unwrap()
            .push((metadata.name().to_string(), fields, values.0));
    }
}

fn started() -> SessionStartedFact {
    SessionStartedFact {
        session_id: protocol_id("11111111-1111-4111-8111-111111111111"),
        mode: TerminalMode::Managed,
        profile_kind: TerminalProfileKind::Agent,
        provider_kind: TerminalProviderKind::Claude,
        elapsed_ms: 37,
    }
}

#[test]
fn disabled_tracing_emits_no_event() {
    let capture = Capture::default();
    let subscriber = tracing_subscriber::registry().with(capture.clone());
    tracing::subscriber::with_default(subscriber, || {
        let _ = started().commit(SessionTrace::new(false));
    });
    assert!(capture.0.lock().unwrap().is_empty());
}

#[test]
fn tracing_and_event_payload_have_independent_pinned_allowlists() {
    let capture = Capture::default();
    let subscriber = tracing_subscriber::registry().with(capture.clone());
    let payload =
        tracing::subscriber::with_default(subscriber, || started().commit(SessionTrace::new(true)));
    let events = capture.0.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].2.contains("terminal.session.started"));
    assert_eq!(
        events[0].1,
        [
            "event",
            "session_id",
            "mode",
            "profile_kind",
            "provider_kind",
            "elapsed_ms"
        ]
    );

    let context = EmitContext {
        session_id: EventSessionId::mint(),
        device_id: None,
        profile_id: None,
        actor: koru_events::Actor::user(),
    };
    let envelope = Draft::from(payload).build(&context, EventId::mint(), 1_756_000_000_000);
    validate(&envelope).unwrap();
    assert_eq!(
        envelope
            .attrs
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["mode", "profile_kind", "provider_kind"]
    );
    assert!(!envelope.attrs.contains_key("elapsed_ms"));
    assert!(!envelope.attrs.contains_key("session_id"));
}

#[test]
fn every_shared_fact_has_a_pinned_trace_allowlist() {
    let capture = Capture::default();
    let subscriber = tracing_subscriber::registry().with(capture.clone());
    tracing::subscriber::with_default(subscriber, || {
        let trace = SessionTrace::new(true);
        let session: SessionId = protocol_id("55555555-5555-4555-8555-555555555555");
        let attachment: AttachmentId = protocol_id("66666666-6666-4666-8666-666666666666");
        let _ = SurfaceOpenedFact {
            surface: koru_events::TerminalSurface::Terminal,
        }
        .commit(trace);
        let _ = SessionReattachedFact {
            session_id: session,
        }
        .commit(trace);
        let _ = SessionEndedFact {
            session_id: session,
            outcome: TerminalSessionOutcome::Failed,
            elapsed_ms: 12,
        }
        .commit(trace);
        let _ = InputSubmittedFact {
            session_id: session,
            input_kind: TerminalInputKind::Image,
        }
        .commit(trace);
        let _ = AttachmentDeliveredFact {
            session_id: session,
            attachment_id: attachment,
            delivery_mode: TerminalDeliveryMode::Unsupported,
            outcome: TerminalDeliveryOutcome::Unsupported,
        }
        .commit(trace);
    });
    let events = capture.0.lock().unwrap();
    let fields = |name: &str| {
        events
            .iter()
            .find(|event| event.2.contains(name))
            .unwrap_or_else(|| panic!("missing {name}"))
            .1
            .as_slice()
    };
    assert_eq!(fields("terminal.surface.opened"), ["event", "surface"]);
    assert_eq!(
        fields("terminal.session.reattached"),
        ["event", "session_id", "attached", "reason"]
    );
    assert_eq!(
        fields("terminal.session.ended"),
        ["event", "session_id", "exit_kind", "elapsed_ms"]
    );
    assert_eq!(
        fields("terminal.input.submitted"),
        ["event", "session_id", "input_kind"]
    );
    assert_eq!(
        fields("terminal.attachment.delivered"),
        [
            "event",
            "session_id",
            "attachment_id",
            "delivery_mode",
            "state"
        ]
    );
}

#[test]
fn additive_provider_tokens_collapse_to_the_reviewed_metrics_vocabulary() {
    use std::str::FromStr as _;
    let unknown = koru_session_protocol::ProviderKind::from_str("provider-opaque-123").unwrap();
    assert_eq!(
        metric_provider_kind(Some(&unknown)),
        TerminalProviderKind::Other
    );
    assert_eq!(metric_provider_kind(None), TerminalProviderKind::None);
}

#[test]
fn sentinel_content_cannot_enter_fact_logs_or_event_attrs() {
    const SENTINEL: &str = "SECRET prompt C:\\private\\title.txt https://example.invalid";
    let capture = Capture::default();
    let subscriber = tracing_subscriber::registry().with(capture.clone());
    let input = InputSubmittedFact {
        session_id: protocol_id("22222222-2222-4222-8222-222222222222"),
        input_kind: TerminalInputKind::Mixed,
    };
    let payload =
        tracing::subscriber::with_default(subscriber, || input.commit(SessionTrace::new(true)));
    let captured = format!("{:?}", capture.0.lock().unwrap());
    assert!(!captured.contains(SENTINEL));
    let draft = Draft::from(payload);
    let context = EmitContext::user_session();
    let envelope = draft.build(&context, EventId::mint(), 1_756_000_000_000);
    let attrs = serde_json::to_string(&envelope.attrs).unwrap();
    assert!(!attrs.contains(SENTINEL));
    assert_eq!(
        envelope.attrs.get("input_kind"),
        Some(&AttrValue::Token("mixed".into()))
    );
}

#[test]
fn all_lifecycle_facts_build_registered_payloads() {
    let trace = SessionTrace::new(false);
    let session: SessionId = protocol_id("33333333-3333-4333-8333-333333333333");
    let attachment: AttachmentId = protocol_id("44444444-4444-4444-8444-444444444444");
    let drafts: Vec<Draft> = vec![
        SurfaceOpenedFact {
            surface: koru_events::TerminalSurface::Terminal,
        }
        .commit(trace)
        .into(),
        started().commit(trace).into(),
        SessionReattachedFact {
            session_id: session,
        }
        .commit(trace)
        .into(),
        SessionEndedFact {
            session_id: session,
            outcome: TerminalSessionOutcome::Exited,
            elapsed_ms: 90_000,
        }
        .commit(trace)
        .into(),
        AttachmentDeliveredFact {
            session_id: session,
            attachment_id: attachment,
            delivery_mode: TerminalDeliveryMode::Native,
            outcome: TerminalDeliveryOutcome::Delivered,
        }
        .commit(trace)
        .into(),
    ];
    let context = EmitContext::user_session();
    for draft in drafts {
        validate(&draft.build(&context, EventId::mint(), 1_756_000_000_000)).unwrap();
    }
}
