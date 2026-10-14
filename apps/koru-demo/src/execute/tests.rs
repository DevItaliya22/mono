// SPDX-License-Identifier: MPL-2.0

// The case-table tests are `cfg(windows)` (they compile registry chords
// into virtual-key codes); their helpers stay for them.
#![cfg_attr(not(windows), allow(dead_code))]
use super::*;
mod bootstrap;
mod reopen;
use crate::{
    inspect::{Rect, Role},
    supervise::{Limits, Mechanism},
};
use std::{
    sync::{Arc, Mutex},
    time::Instant,
};

struct Fake {
    requests: Arc<Mutex<Vec<Request>>>,
    matches: Vec<ElementFacts>,
    body: Vec<ElementFacts>,
    check: bool,
}
impl Mechanism for Fake {
    fn perform(&mut self, request: Request) -> Response {
        self.requests.lock().unwrap().push(request.clone());
        match request {
            Request::Matches { pattern, .. } if pattern.part == "root" => {
                Response::Found(self.body.clone())
            }
            Request::Matches { .. } | Request::Focused { .. } => {
                Response::Found(self.matches.clone())
            }
            Request::Check { .. } => Response::Checked(self.check),
            Request::Input(_) => Response::Input(input::Result::Complete { bounds: None }),
            _ => unreachable!(),
        }
    }
}
struct ClockFake(std::cell::Cell<Instant>);
impl Clock for ClockFake {
    fn now(&self) -> Instant {
        self.0.get()
    }
    fn rest(&self, duration: Duration) {
        self.0.set(self.0.get() + duration);
    }
}
fn facts() -> ElementFacts {
    ElementFacts {
        handle: Handle {
            generation: 1,
            slot: 1,
        },
        locator: Some("koru:w1/v1/s:body#root".into()),
        role: Role::Edit,
        enabled: true,
        offscreen: false,
        bounds: Some(Rect {
            left: 0,
            top: 0,
            right: 100,
            bottom: 30,
        }),
        process_id: Some(7),
    }
}
fn step(operation: Operation) -> Step {
    Step {
        id: "example",
        target: Target::Locator(
            crate::locator::LocatorPattern::parse(
                "koru:w*/v*/n:00000000000000000000000000000001#text",
            )
            .unwrap(),
        ),
        operation,
    }
}
fn run(
    matches: Vec<ElementFacts>,
    check: bool,
    operation: Operation,
) -> (StepResult, Vec<Request>) {
    run_with_body(matches, vec![facts()], check, operation)
}
fn run_with_body(
    matches: Vec<ElementFacts>,
    body: Vec<ElementFacts>,
    check: bool,
    operation: Operation,
) -> (StepResult, Vec<Request>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let shared = requests.clone();
    let mut supervisor = Supervisor::new(
        Arc::new(move |_| {
            Ok(Box::new(Fake {
                requests: shared.clone(),
                matches: matches.clone(),
                body: body.clone(),
                check,
            }))
        }),
        Limits::default(),
    );
    let cancel = CancelToken::new();
    let clock = ClockFake(std::cell::Cell::new(Instant::now()));
    let mut executor = Executor {
        supervisor: &mut supervisor,
        cancel: &cancel,
        root: Handle {
            generation: 1,
            slot: 0,
        },
        window_slot: 1,
        window_handle: 50,
        process_id: 7,
        patience: Patience {
            limit: Duration::from_millis(10),
            interval: Duration::from_millis(5),
        },
        clock: &clock,
    };
    let result = executor.step(&step(operation));
    let recorded = requests.lock().unwrap().clone();
    (result, recorded)
}

#[test]
fn ambiguous_targets_never_receive_input() {
    let (result, requests) = run(
        vec![facts(), facts()],
        true,
        Operation::Input(Action::Click),
    );
    assert_eq!(result.failure, Some("ambiguous-target"));
    assert!(!requests.iter().any(|r| matches!(r, Request::Input(_))));
}
#[test]
fn assertion_timeout_retains_target_facts_without_expected_or_actual_text() {
    let (result, _) = run(
        vec![facts()],
        false,
        Operation::Expect(input::Condition::Text("private content".into())),
    );
    assert_eq!(result.failure, Some("state-timeout"));
    assert!(result.target.is_some());
    assert!(!format!("{result:?}").contains("private content"));
}
#[test]
fn offscreen_text_is_read_but_never_clicked() {
    let mut target = facts();
    target.offscreen = true;
    assert!(
        run(
            vec![target.clone()],
            true,
            Operation::Expect(input::Condition::Text("x".into()))
        )
        .0
        .failure
        .is_none()
    );
    let (result, requests) = run(vec![target], true, Operation::Input(Action::Click));
    assert_eq!(result.failure, Some("state-timeout"));
    assert!(!requests.iter().any(|r| matches!(r, Request::Input(_))));
}
#[test]
fn navigation_waits_for_geometry_progress_before_sending_another_page_down() {
    let mut target = facts();
    target.bounds = Some(Rect {
        left: 0,
        top: 40,
        right: 100,
        bottom: 70,
    });
    let (result, requests) = run(vec![target], false, Operation::ReachByPageDown);
    assert_eq!(result.failure, Some("state-timeout"));
    assert_eq!(
        requests
            .iter()
            .filter(|r| matches!(r, Request::Input(_)))
            .count(),
        1
    );
}
#[test]
fn focus_navigation_waits_for_focus_acknowledgement_and_does_not_double_click() {
    let (result, requests) = run(vec![facts()], false, Operation::ReachByPageDown);
    assert_eq!(result.failure, Some("state-timeout"));
    assert_eq!(
        requests
            .iter()
            .filter(|r| matches!(r, Request::Input(_)))
            .count(),
        1
    );
}

#[test]
fn false_expected_boolean_reports_actual_false_when_the_assertion_matches() {
    let (result, _) = run(
        vec![facts()],
        true,
        Operation::Expect(input::Condition::Enabled(false)),
    );
    assert_eq!(result.expected.boolean, Some(false));
    assert_eq!(result.actual.boolean, Some(false));
    assert_eq!(result.actual.matched, Some(true));
}

#[test]
fn page_up_cannot_finish_outside_the_viewport_even_when_raw_offscreen_is_false() {
    let mut target = facts();
    target.bounds = Some(Rect {
        left: 0,
        top: -40,
        right: 100,
        bottom: -10,
    });
    let (result, requests) = run(vec![target], true, Operation::ReachByPageUp);
    assert_eq!(result.failure, Some("state-timeout"));
    let keys: Vec<_> = requests
        .iter()
        .filter_map(|request| match request {
            Request::Input(Consequence {
                action: Action::Chord(keys),
                ..
            }) => Some(keys.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(keys, vec![vec![0x21]]);
}

#[test]
fn a_missing_later_poll_clears_an_earlier_observed_property() {
    struct Vanishing {
        lookups: usize,
    }
    impl Mechanism for Vanishing {
        fn perform(&mut self, request: Request) -> Response {
            match request {
                Request::Matches { .. } => {
                    self.lookups += 1;
                    Response::Found(if self.lookups == 1 {
                        vec![facts()]
                    } else {
                        vec![]
                    })
                }
                Request::Check { .. } => Response::Checked(false),
                _ => unreachable!(),
            }
        }
    }
    let mut supervisor = Supervisor::new(
        Arc::new(|_| Ok(Box::new(Vanishing { lookups: 0 }))),
        Limits::default(),
    );
    let cancel = CancelToken::new();
    let clock = ClockFake(std::cell::Cell::new(Instant::now()));
    let mut executor = Executor {
        supervisor: &mut supervisor,
        cancel: &cancel,
        root: Handle {
            generation: 1,
            slot: 0,
        },
        window_slot: 1,
        window_handle: 50,
        process_id: 7,
        patience: Patience {
            limit: Duration::from_millis(10),
            interval: Duration::from_millis(5),
        },
        clock: &clock,
    };
    let result = executor.step(&step(Operation::Expect(input::Condition::Enabled(true))));
    assert_eq!(result.failure, Some("state-timeout"));
    assert_eq!(result.selection, Some(Selection::NotFound));
    assert_eq!(result.actual, Observed::default());
    assert!(result.target.is_none());
}

#[test]
fn distant_target_without_bounds_is_reached_across_multiple_observed_pages() {
    struct Paging {
        pages: usize,
        focused: bool,
        keys: Arc<Mutex<Vec<u16>>>,
    }
    impl Mechanism for Paging {
        fn perform(&mut self, request: Request) -> Response {
            match request {
                Request::Matches { pattern, .. } => {
                    let mut item = facts();
                    if pattern.part == "text" {
                        item.locator =
                            Some("koru:w1/v1/n:00000000000000000000000000000001#text".into());
                        item.offscreen = false;
                        item.bounds = if self.pages < 3 { None } else { facts().bounds };
                    }
                    Response::Found(vec![item])
                }
                Request::Focused { .. } => {
                    let mut item = facts();
                    item.locator = Some(format!("koru:w1/v1/n:{:032x}#text", self.pages + 10));
                    Response::Found(vec![item])
                }
                Request::Check { .. } => Response::Checked(self.focused),
                Request::Input(Consequence { action, .. }) => {
                    match action {
                        Action::Chord(keys) => {
                            self.pages += 1;
                            self.keys.lock().unwrap().push(keys[0]);
                        }
                        Action::Focus => self.focused = true,
                        _ => unreachable!(),
                    }
                    Response::Input(input::Result::Complete { bounds: None })
                }
                _ => unreachable!(),
            }
        }
    }
    for (operation, key) in [
        (Operation::ReachByPageDown, 0x22),
        (Operation::ReachByPageUp, 0x21),
    ] {
        let keys = Arc::new(Mutex::new(Vec::new()));
        let shared = keys.clone();
        let mut supervisor = Supervisor::new(
            Arc::new(move |_| {
                Ok(Box::new(Paging {
                    pages: 0,
                    focused: false,
                    keys: shared.clone(),
                }))
            }),
            Limits::default(),
        );
        let cancel = CancelToken::new();
        let clock = ClockFake(std::cell::Cell::new(Instant::now()));
        let mut executor = Executor {
            supervisor: &mut supervisor,
            cancel: &cancel,
            root: Handle {
                generation: 1,
                slot: 0,
            },
            window_slot: 1,
            window_handle: 50,
            process_id: 7,
            patience: Patience {
                limit: Duration::from_millis(50),
                interval: Duration::from_millis(5),
            },
            clock: &clock,
        };
        let step = Step {
            id: "reach",
            target: Target::Locator(
                crate::locator::LocatorPattern::parse(
                    "koru:w*/v*/n:00000000000000000000000000000001#text",
                )
                .unwrap(),
            ),
            operation,
        };
        let result = executor.step(&step);
        assert_eq!(result.failure, None);
        assert_eq!(*keys.lock().unwrap(), vec![key, key, key]);
        assert_eq!(result.actual.boolean, Some(true));
    }
}

// Compiles registry chords into Windows key codes; a Mac registry is ⌘-based.
#[cfg(windows)]
#[test]
fn opening_a_different_catalog_document_refuses_captured_paragraphs_before_mutation() {
    // Two documents can have the same visible title. Capture may have seen B;
    // opening catalog A must not authorize B's captured paragraph identities.
    struct DifferentDocument {
        inputs: Arc<Mutex<Vec<Action>>>,
    }
    impl Mechanism for DifferentDocument {
        fn perform(&mut self, request: Request) -> Response {
            match request {
                Request::Matches { pattern, .. } => {
                    let is_navigation = matches!(&pattern.object,
                        crate::locator::Object::Surface(name) if name == "explore-mode");
                    let is_catalog = matches!(&pattern.object,
                        crate::locator::Object::Model(id) if id == "0000000000000000000000000000000a")
                        && pattern.part == "root";
                    if is_navigation || is_catalog {
                        let mut item = facts();
                        item.role = if is_catalog {
                            Role::TreeItem
                        } else {
                            Role::Button
                        };
                        item.locator = Some(pattern.to_string());
                        Response::Found(vec![item])
                    } else {
                        Response::Found(vec![])
                    }
                }
                Request::Input(input) => {
                    self.inputs.lock().unwrap().push(input.action);
                    Response::Input(input::Result::Complete { bounds: None })
                }
                _ => panic!("no captured paragraph exists in the opened document"),
            }
        }
    }
    for name in crate::cases::names() {
        let fixture = crate::cases::Fixture {
            document: crate::locator::LocatorPattern::parse(
                "koru:w*/v*/n:0000000000000000000000000000000a#root",
            )
            .unwrap(),
            subject: crate::locator::LocatorPattern::parse(
                "koru:w*/v*/n:0000000000000000000000000000000b#text",
            )
            .unwrap(),
            entry: Some(
                crate::locator::LocatorPattern::parse(
                    "koru:w*/v*/n:0000000000000000000000000000000c#text",
                )
                .unwrap(),
            ),
            reminder_date: Some("04/12/2099".into()),
            reminder_time: Some("3:30 PM".into()),
        };
        let case = crate::cases::compile(name, &fixture).unwrap();
        let inputs = Arc::new(Mutex::new(Vec::new()));
        let shared = inputs.clone();
        let mut supervisor = Supervisor::new(
            Arc::new(move |_| {
                Ok(Box::new(DifferentDocument {
                    inputs: shared.clone(),
                }))
            }),
            Limits::default(),
        );
        let cancel = CancelToken::new();
        let clock = ClockFake(std::cell::Cell::new(Instant::now()));
        let mut executor = Executor {
            supervisor: &mut supervisor,
            cancel: &cancel,
            root: Handle {
                generation: 1,
                slot: 0,
            },
            window_slot: 1,
            window_handle: 50,
            process_id: 7,
            patience: Patience {
                limit: Duration::from_millis(10),
                interval: Duration::from_millis(5),
            },
            clock: &clock,
        };
        let results = executor.run(&case);
        assert_eq!(
            results.last().unwrap().failure,
            Some("state-timeout"),
            "{name}"
        );
        assert_eq!(results.last().unwrap().selection, Some(Selection::NotFound));
        assert_eq!(
            *inputs.lock().unwrap(),
            vec![Action::Click, Action::Click],
            "only document navigation can occur; no text, task conversion, reminder or cleanup input"
        );
    }
}

#[test]
fn geometric_visibility_ignores_raw_offscreen_and_preserves_that_fact() {
    let mut target = facts();
    target.bounds = Some(Rect {
        left: 0,
        top: 40,
        right: 100,
        bottom: 70,
    });
    let (result, requests) = run(vec![target], true, Operation::InDocumentViewport(false));
    assert_eq!(result.failure, None);
    assert_eq!(result.actual.boolean, Some(false));
    assert_eq!(result.expected.kind, "in-document-viewport");
    assert!(!result.target.unwrap().offscreen);
    assert!(
        !requests
            .iter()
            .any(|request| matches!(request, Request::Check { .. } | Request::Input(_)))
    );
}
#[test]
fn a_near_viewport_layout_band_does_not_receive_a_center_click() {
    let mut target = facts();
    target.bounds = Some(Rect {
        left: 0,
        top: 20,
        right: 100,
        bottom: 50,
    });
    let (result, requests) = run(vec![target], false, Operation::ReachByPageDown);
    assert_eq!(result.failure, Some("state-timeout"));
    assert!(requests.iter().any(|request| matches!(
        request,
        Request::Input(Consequence {
            action: Action::Chord(_),
            ..
        })
    )));
    assert!(!requests.iter().any(|request| matches!(
        request,
        Request::Input(Consequence {
            action: Action::Focus,
            ..
        })
    )));
}
#[test]
fn invalid_or_missing_document_viewport_stops_before_focus_or_input() {
    let mut invalid = facts();
    invalid.bounds = None;
    for body in [vec![], vec![invalid]] {
        let (result, requests) =
            run_with_body(vec![facts()], body, true, Operation::ReachByPageDown);
        assert!(matches!(
            result.failure,
            Some("document-body-unavailable" | "document-viewport-unavailable")
        ));
        assert!(
            !requests
                .iter()
                .any(|request| matches!(request, Request::Check { .. } | Request::Input(_)))
        );
    }
}

/// Each subject lookup advances one animation observation, rather than making
/// a page key synchronously finish scrolling as the simpler paging fake does.
fn animated_navigation(
    frames: &[(i32, i32)],
    initially_focused: bool,
    refusal: Option<&'static str>,
) -> (StepResult, Vec<(usize, Action)>) {
    struct Animation {
        frames: Vec<(i32, i32)>,
        poll: usize,
        focused: bool,
        refusal: Option<&'static str>,
        inputs: Arc<Mutex<Vec<(usize, Action)>>>,
    }
    impl Mechanism for Animation {
        fn perform(&mut self, request: Request) -> Response {
            match request {
                Request::Matches { pattern, .. } => {
                    if pattern.part == "text" {
                        self.poll += 1;
                    }
                    let (subject_top, body_top) =
                        self.frames[self.poll.saturating_sub(1).min(self.frames.len() - 1)];
                    let mut item = facts();
                    if pattern.part == "text" {
                        item.locator = Some(pattern.to_string());
                        item.bounds = Some(Rect {
                            left: 0,
                            top: subject_top,
                            right: 100,
                            bottom: subject_top + 10,
                        });
                    } else {
                        item.bounds = Some(Rect {
                            left: 0,
                            top: body_top,
                            right: 100,
                            bottom: body_top + 30,
                        });
                    }
                    Response::Found(vec![item])
                }
                Request::Focused { .. } => Response::Found(vec![facts()]),
                Request::Check { .. } => Response::Checked(self.focused),
                Request::Input(Consequence { action, .. }) => {
                    self.inputs
                        .lock()
                        .unwrap()
                        .push((self.poll, action.clone()));
                    if matches!(action, Action::Focus) {
                        if let Some(code) = self.refusal {
                            return Response::Input(input::Result::Refused(code));
                        }
                        self.focused = true;
                    }
                    Response::Input(input::Result::Complete { bounds: None })
                }
                _ => unreachable!(),
            }
        }
    }
    let inputs = Arc::new(Mutex::new(Vec::new()));
    let shared = inputs.clone();
    let frames = frames.to_vec();
    let mut supervisor = Supervisor::new(
        Arc::new(move |_| {
            Ok(Box::new(Animation {
                frames: frames.clone(),
                poll: 0,
                focused: initially_focused,
                refusal,
                inputs: shared.clone(),
            }))
        }),
        Limits::default(),
    );
    let cancel = CancelToken::new();
    let clock = ClockFake(std::cell::Cell::new(Instant::now()));
    let mut executor = Executor {
        supervisor: &mut supervisor,
        cancel: &cancel,
        root: Handle {
            generation: 1,
            slot: 0,
        },
        window_slot: 1,
        window_handle: 50,
        process_id: 7,
        patience: Patience {
            limit: Duration::from_millis(100),
            interval: Duration::from_millis(5),
        },
        clock: &clock,
    };
    let result = executor.step(&step(Operation::ReachByPageDown));
    let inputs = inputs.lock().unwrap().clone();
    (result, inputs)
}

#[test]
fn animated_scroll_waits_for_subject_and_body_to_settle_without_input_bursts() {
    let frames = [
        (80, 0),
        (80, 0),
        (60, 0),
        (20, 0),
        (15, 0),
        (15, 2),
        (10, 2),
        (10, 2),
    ];
    let (result, inputs) = animated_navigation(&frames, false, None);
    assert_eq!(result.failure, None);
    assert_eq!(
        inputs,
        vec![(2, Action::Chord(vec![0x22])), (8, Action::Focus)]
    );
    // Completion also waits for fresh equal observations after the focus input.
    assert_eq!(result.elapsed, Duration::from_millis(45));
}

#[test]
fn focused_subject_still_waits_for_animation_to_settle_before_completion() {
    let frames = [
        (80, 0),
        (80, 0),
        (60, 0),
        (20, 0),
        (15, 0),
        (15, 2),
        (10, 2),
        (10, 2),
    ];
    let (result, inputs) = animated_navigation(&frames, true, None);
    assert_eq!(result.failure, None);
    assert_eq!(inputs, vec![(2, Action::Chord(vec![0x22]))]);
    assert_eq!(result.elapsed, Duration::from_millis(35));
}

#[test]
fn continuously_moving_visible_subject_times_out_without_a_focus_click() {
    let frames: Vec<_> = (0..30).map(|i| (10 + i % 2, 0)).collect();
    let (result, inputs) = animated_navigation(&frames, false, None);
    assert_eq!(result.failure, Some("state-timeout"));
    assert!(inputs.is_empty());
}

#[test]
fn native_stale_bounds_after_observed_settling_is_not_retried() {
    let (result, inputs) = animated_navigation(&[(10, 0), (10, 0)], false, Some("stale-bounds"));
    assert_eq!(result.failure, Some("stale-bounds"));
    assert_eq!(inputs, vec![(2, Action::Focus)]);
}
