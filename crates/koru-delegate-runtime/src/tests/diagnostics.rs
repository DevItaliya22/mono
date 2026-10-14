// SPDX-License-Identifier: MPL-2.0

//! `KORU_DELEGATE_DEBUG`: news, not activity — and nothing at all when it
//! is off.
//!
//! The host reads the variable once at startup into [`Options::debug`]
//! (`crates/koru-delegate-host/src/lib.rs`), so what is asserted here is the
//! flag's two states rather than the environment. Two properties, and the
//! second is the one that matters:
//!
//! - **Off costs nothing.** Not a line, not a formatted string: the
//!   `format_args!` a call site builds is lazy, and `trace` returns on a
//!   bool before it is ever rendered. So the assertion is *zero events on
//!   this target*, which is the observable version of "free when off".
//! - **On reports news.** A line per consequential thing — a status
//!   transition, an appended step, a usage fact, a failure — and **never a
//!   line per provider event**. The assertion is a ratio, the way
//!   PRINCIPLES asks: the same turn driven with ten times the message
//!   deltas produces exactly the same number of lines. A per-event trace
//!   could not pass that, and no count of call sites is involved.

use std::sync::{Arc, Mutex};

use koru_session_protocol::ToolCallId;

use super::*;

/// A `Harness` whose orchestrator has the debug flag on, over the same
/// ports and the same temporary directory the plain one builds.
fn debugging(label: &str) -> Harness {
    let Harness {
        orchestrator,
        schedule_store,
        schedule_clock,
        ledger,
        documents,
        sessions,
        capture,
        bus,
        registrations,
        authority,
        sleeps,
        preambles,
        dir,
    } = Harness::new(label);
    let orchestrator = Orchestrator::new(
        orchestrator.into_ports(),
        Options {
            debug: true,
            reask_attempts: 3,
            ..Options::default()
        },
    );
    Harness {
        orchestrator,
        schedule_store,
        schedule_clock,
        ledger,
        documents,
        sessions,
        capture,
        bus,
        registrations,
        authority,
        sleeps,
        preambles,
        dir,
    }
}

/// Create a run, start it, drive one turn with `deltas` message deltas and
/// a tool call, and complete it. Answers the trace lines it produced.
fn one_turn(mut h: Harness, deltas: usize) -> Vec<String> {
    let lines = Recorder::default();
    let collected = Arc::clone(&lines.0);
    tracing::subscriber::with_default(lines, || {
        let run = h.running(1);
        let t = turn(1);
        h.agent(run, AgentEvent::TurnStarted { turn_id: t });
        for _ in 0..deltas {
            h.agent(
                run,
                AgentEvent::MessageDelta {
                    turn_id: t,
                    text: "a delta".into(),
                },
            );
        }
        let call = ToolCallId::mint();
        h.agent(
            run,
            AgentEvent::ToolStarted {
                turn_id: t,
                tool_call_id: call,
                label: "a tool".into(),
                input: None,
            },
        );
        h.agent(
            run,
            AgentEvent::ToolCompleted {
                turn_id: t,
                tool_call_id: call,
                succeeded: true,
                tool: Some(koru_session_protocol::ToolKind::CommandExecution),
                failure: None,
                detail: None,
                output: None,
            },
        );
        h.agent(run, AgentEvent::TurnCompleted { turn_id: t });
    });
    let collected = collected.lock().unwrap();
    collected.clone()
}

#[test]
fn the_flag_off_costs_a_branch_and_produces_nothing() {
    let lines = one_turn(Harness::new("debug_off"), 12);
    assert!(lines.is_empty(), "a run with the flag off traced {lines:?}");
}

/// The whole point: a line per consequential event, and the count does not
/// move when the provider says ten times as much.
#[test]
fn the_flag_on_reports_news_and_never_a_line_per_provider_event() {
    let quiet = one_turn(debugging("debug_quiet"), 1);
    let chatty = one_turn(debugging("debug_chatty"), 40);
    assert!(!quiet.is_empty(), "the flag on traced nothing");
    assert_eq!(
        quiet, chatty,
        "the trace grew with the provider's stream rather than with the run"
    );
    // And it names what happened, in the closed vocabulary the call sites
    // use — a status transition and a recorded step, both of which are
    // durable facts rather than passing traffic.
    assert!(quiet.iter().any(|line| line == "status"), "{quiet:?}");
    assert!(quiet.iter().any(|line| line == "step"), "{quiet:?}");
}

// ---------------------------------------------------------- the recorder
//
// A `tracing::Subscriber` of about forty lines rather than a dev-dependency
// on `tracing-subscriber`: all it has to do is keep the `what` field of
// every event on this crate's target, and `with_default` scopes it to this
// thread so tests beside it are unaffected.

#[derive(Default)]
struct Recorder(Arc<Mutex<Vec<String>>>);

#[derive(Default)]
struct What(Option<String>);

impl tracing::field::Visit for What {
    fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "what" {
            self.0 = Some(value.to_owned());
        }
    }
}

impl tracing::Subscriber for Recorder {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.target() == "koru::delegate"
    }

    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut what = What::default();
        event.record(&mut what);
        self.0
            .lock()
            .unwrap()
            .push(what.0.unwrap_or_else(|| "<unnamed>".to_owned()));
    }

    fn enter(&self, _: &tracing::span::Id) {}

    fn exit(&self, _: &tracing::span::Id) {}
}
