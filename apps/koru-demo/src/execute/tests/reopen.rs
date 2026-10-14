// SPDX-License-Identifier: MPL-2.0

//! Reopening from Explore does not promise a paragraph holds keyboard focus.
// The case-table tests are `cfg(windows)` (they compile registry chords
// into virtual-key codes); their helpers stay for them.
#![cfg_attr(not(windows), allow(dead_code))]
use super::*;
use crate::cases;
#[cfg(windows)]
use crate::locator::LocatorPattern;

struct Reopened {
    focused: Option<u64>,
    reached: bool,
    entry_matches: bool,
    inputs: Arc<Mutex<Vec<(u64, Action)>>>,
}
impl Mechanism for Reopened {
    fn perform(&mut self, request: Request) -> Response {
        match request {
            Request::Matches { pattern, .. } => {
                let mut item = facts();
                item.locator = Some(pattern.to_string());
                item.handle.slot = match &pattern.object {
                    crate::locator::Object::Model(id) => u64::from_str_radix(id, 16).unwrap(),
                    _ => 0,
                };
                if item.handle.slot == 1 && !self.reached {
                    item.bounds = None;
                }
                if item.handle.slot == 2 {
                    item.role = Role::TreeItem;
                }
                Response::Found(vec![item])
            }
            Request::Focused { .. } => Response::Found(
                self.focused
                    .map(|slot| {
                        let mut item = facts();
                        item.handle.slot = slot;
                        item.locator = Some(format!("koru:w1/v1/n:{slot:032x}#text"));
                        item
                    })
                    .into_iter()
                    .collect(),
            ),
            Request::Check { element, condition } => Response::Checked(match condition {
                input::Condition::Focused => self.focused == Some(element.slot),
                input::Condition::Text(text) if element.slot == 3 => {
                    assert_eq!(text, cases::DOCUMENT_ENTRY);
                    self.entry_matches
                }
                input::Condition::Text(text) if element.slot == 1 => {
                    assert!(self.reached);
                    assert_eq!(text, cases::DOCUMENT_AFTER);
                    true
                }
                _ => panic!("unexpected reopen assertion"),
            }),
            Request::Input(Consequence {
                element, action, ..
            }) => {
                self.inputs
                    .lock()
                    .unwrap()
                    .push((element.slot, action.clone()));
                match action {
                    Action::Click => {
                        assert_eq!(element.slot, 2);
                        self.focused = None;
                        self.reached = false;
                    }
                    Action::Focus => {
                        assert_eq!(element.slot, 3, "focus the exact captured entry");
                        self.focused = Some(3);
                    }
                    Action::Chord(keys) => {
                        assert_eq!(keys, vec![0x22]);
                        assert_eq!(element.slot, 3);
                        assert_eq!(self.focused, Some(3), "never page from an unfocused body");
                        assert!(
                            self.entry_matches,
                            "entry identity/content proof precedes paging"
                        );
                        self.reached = true;
                        self.focused = Some(1);
                    }
                    _ => panic!("reopen retention must not rewrite the saved document"),
                }
                Response::Input(input::Result::Complete { bounds: None })
            }
            _ => unreachable!(),
        }
    }
}

// Compiles registry chords into Windows key codes; a Mac registry is ⌘-based.
#[cfg(windows)]
#[test]
fn reopened_cold_document_reestablishes_exact_entry_focus_before_paging_or_reading() {
    for entry_matches in [true, false] {
        let locator =
            |id, part| LocatorPattern::parse(&format!("koru:w*/v*/n:{id:032x}#{part}")).unwrap();
        let fixture = cases::Fixture {
            subject: locator(1, "text"),
            document: locator(2, "root"),
            entry: Some(locator(3, "text")),
            reminder_date: None,
            reminder_time: None,
        };
        let mut case = cases::compile("document-edit", &fixture).unwrap();
        let reopened = case
            .steps
            .iter()
            .position(|step| step.id == "reopen-document")
            .unwrap();
        case.steps.drain(..reopened);
        let inputs = Arc::new(Mutex::new(Vec::new()));
        let shared = inputs.clone();
        let mut supervisor = Supervisor::new(
            Arc::new(move |_| {
                Ok(Box::new(Reopened {
                    focused: None,
                    reached: false,
                    entry_matches,
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
                limit: Duration::from_millis(50),
                interval: Duration::from_millis(5),
            },
            clock: &clock,
        };
        let results = executor.run(&case);
        if entry_matches {
            assert_eq!(results.last().unwrap().failure, None);
            assert_eq!(results.last().unwrap().step, "reopened-edit-retained");
            assert_eq!(
                *inputs.lock().unwrap(),
                vec![
                    (2, Action::Click),
                    (3, Action::Focus),
                    (3, Action::Chord(vec![0x22])),
                ]
            );
        } else {
            assert_eq!(results.last().unwrap().step, "reopened-entry");
            assert_eq!(results.last().unwrap().failure, Some("state-timeout"));
            assert_eq!(
                *inputs.lock().unwrap(),
                vec![(2, Action::Click), (3, Action::Focus)]
            );
        }
    }
}
