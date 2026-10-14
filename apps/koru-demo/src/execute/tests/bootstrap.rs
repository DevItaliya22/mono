// SPDX-License-Identifier: MPL-2.0

use super::*;

#[derive(Clone, Copy)]
enum Scenario {
    Normal,
    NoCandidate,
    Duplicate,
    ForeignProcess,
    ForeignView,
    Clipped,
    Disabled,
    Moving,
    Refused,
    NoAcknowledgement,
    WrongFocus,
    WrongFocusVisible,
    LostAfterPage,
    LostBeforePage,
}
struct Restored {
    scenario: Scenario,
    focused: Option<u64>,
    reached: bool,
    scans: usize,
    inputs: Arc<Mutex<Vec<Action>>>,
}
fn paragraph(slot: u64) -> ElementFacts {
    let mut item = facts();
    item.handle.slot = slot;
    item.locator = Some(format!("koru:w1/v1/n:{slot:032x}#text"));
    item
}
impl Mechanism for Restored {
    fn perform(&mut self, request: Request) -> Response {
        match request {
            Request::Matches { pattern, .. } if pattern.part == "root" => {
                let mut body = facts();
                body.handle.slot = 0;
                body.role = Role::Group;
                Response::Found(vec![body])
            }
            Request::Matches { .. } => {
                let mut subject = paragraph(1);
                if !self.reached {
                    subject.bounds = None;
                }
                Response::Found(vec![subject])
            }
            Request::Focused { root } => {
                assert_eq!(root.slot, 0, "focus must be scoped to the exact body");
                let found = self.focused.map(paragraph).into_iter().collect();
                if matches!(self.scenario, Scenario::LostBeforePage) {
                    self.focused = None;
                }
                Response::Found(found)
            }
            Request::Check {
                element,
                condition: input::Condition::Focused,
            } => Response::Checked(self.focused == Some(element.slot)),
            Request::MatchesPart { root, part } => {
                assert_eq!(
                    root.slot, 0,
                    "bootstrap discovery stays inside the exact body"
                );
                assert_eq!(part, "text");
                self.scans += 1;
                let mut item = paragraph(2);
                match self.scenario {
                    Scenario::NoCandidate => return Response::Found(vec![]),
                    Scenario::Duplicate => return Response::Found(vec![item.clone(), item]),
                    Scenario::ForeignProcess => item.process_id = Some(8),
                    Scenario::ForeignView => {
                        item.locator = Some(format!("koru:w1/v2/n:{:032x}#text", 2))
                    }
                    Scenario::Clipped => {
                        item.bounds = Some(Rect {
                            left: 0,
                            top: 20,
                            right: 100,
                            bottom: 50,
                        })
                    }
                    Scenario::Disabled => item.enabled = false,
                    Scenario::Moving => {
                        item.bounds = Some(Rect {
                            left: 0,
                            top: (self.scans % 2) as i32,
                            right: 100,
                            bottom: 20,
                        });
                    }
                    _ => {}
                }
                Response::Found(vec![item])
            }
            Request::Input(Consequence {
                element,
                action,
                root_window,
                process_id,
            }) => {
                assert_eq!((root_window, process_id), (50, 7));
                self.inputs.lock().unwrap().push(action.clone());
                match action {
                    Action::Focus => {
                        assert_eq!(
                            element.slot, 2,
                            "only the discovered concrete paragraph is focused"
                        );
                        match self.scenario {
                            Scenario::Refused => {
                                return Response::Input(input::Result::Refused("stale-bounds"));
                            }
                            Scenario::NoAcknowledgement => {}
                            Scenario::WrongFocus => self.focused = Some(3),
                            Scenario::WrongFocusVisible => {
                                self.focused = Some(3);
                                self.reached = true;
                            }
                            _ => self.focused = Some(2),
                        }
                    }
                    Action::Chord(keys) => {
                        assert_eq!(keys, vec![0x21]);
                        assert_eq!(element.slot, 2);
                        assert_eq!(self.focused, Some(2));
                        if matches!(self.scenario, Scenario::LostAfterPage) {
                            self.focused = None;
                        } else {
                            self.reached = true;
                            self.focused = Some(1);
                        }
                    }
                    _ => panic!("bootstrap cannot edit a document"),
                }
                Response::Input(input::Result::Complete { bounds: None })
            }
            _ => unreachable!(),
        }
    }
}
fn restored(scenario: Scenario) -> (StepResult, Vec<Action>) {
    let inputs = Arc::new(Mutex::new(Vec::new()));
    let shared = inputs.clone();
    let mut supervisor = Supervisor::new(
        Arc::new(move |_| {
            Ok(Box::new(Restored {
                scenario,
                focused: matches!(scenario, Scenario::LostBeforePage).then_some(2),
                reached: false,
                scans: 0,
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
            slot: 99,
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
    let result = executor.step(&step(Operation::ReachByPageUp));
    let inputs = inputs.lock().unwrap().clone();
    (result, inputs)
}
#[test]
fn restored_scroll_with_no_editor_focus_bootstraps_one_visible_paragraph_before_paging() {
    let (result, inputs) = restored(Scenario::Normal);
    assert_eq!(result.failure, None);
    assert_eq!(inputs, vec![Action::Focus, Action::Chord(vec![0x21])]);
    assert!(
        result.elapsed >= Duration::from_millis(25),
        "bootstrap and paging must settle between polls"
    );
}
#[test]
fn missing_unsafe_or_moving_bootstrap_candidates_never_receive_input() {
    for scenario in [
        Scenario::NoCandidate,
        Scenario::Duplicate,
        Scenario::ForeignProcess,
        Scenario::ForeignView,
        Scenario::Clipped,
        Scenario::Disabled,
        Scenario::Moving,
    ] {
        let (result, inputs) = restored(scenario);
        assert!(result.failure.is_some());
        assert!(inputs.is_empty());
    }
}
#[test]
fn bootstrap_focus_refusal_missing_ack_and_changed_focus_never_trigger_another_attempt() {
    for scenario in [
        Scenario::Refused,
        Scenario::NoAcknowledgement,
        Scenario::WrongFocus,
        Scenario::WrongFocusVisible,
    ] {
        let (result, inputs) = restored(scenario);
        assert!(result.failure.is_some());
        if matches!(scenario, Scenario::Refused) {
            assert_eq!(result.failure, Some("stale-bounds"));
        }
        assert_eq!(inputs, vec![Action::Focus]);
    }
    let (result, inputs) = restored(Scenario::LostAfterPage);
    assert_eq!(result.failure, Some("document-focus-unavailable"));
    assert_eq!(inputs, vec![Action::Focus, Action::Chord(vec![0x21])]);
}

#[test]
fn preexisting_focus_lost_before_the_first_page_is_never_bootstrapped_again() {
    let (result, inputs) = restored(Scenario::LostBeforePage);
    assert_eq!(result.failure, Some("document-focus-unavailable"));
    assert!(
        inputs.is_empty(),
        "observed focus loss is terminal even before any page was sent"
    );
}
