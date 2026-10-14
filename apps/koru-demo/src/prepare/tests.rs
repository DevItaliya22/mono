// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::{
    fixtures::Environment,
    input::{Action, Result as InputResult},
    inspect::{Rect, Role},
    supervise::{Limits, Mechanism},
};
use std::{
    cell::Cell,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

struct FakeClock(Cell<Instant>);
impl Clock for FakeClock {
    fn now(&self) -> Instant {
        self.0.get()
    }
    fn rest(&self, duration: Duration) {
        self.0.set(self.0.get() + duration);
    }
}
fn manifest() -> Manifest {
    Manifest::new(
        Path::new("D:/isolated"),
        Environment {
            ui_language: "en".into(),
            locale: "en-US".into(),
            hour_cycle: "h12".into(),
            time_zone: "Eastern Standard Time".into(),
        },
    )
    .unwrap()
}
#[derive(Default)]
struct State {
    texts: Vec<String>,
    first: usize,
    focused: usize,
    shaped: Vec<bool>,
    inputs: Vec<Action>,
    snapshots: usize,
    cancel: Option<CancelToken>,
    wrong_selected_title: bool,
    changed_selection: bool,
    changed_title: bool,
    reordered: bool,
    duplicate: bool,
    foreign: bool,
    clipped_entry: bool,
}
impl State {
    fn new(name: &str) -> Self {
        let content = fixtures::document_text_with_count(name, 40).unwrap();
        let texts: Vec<_> = std::iter::once(fixtures::title(name).unwrap().to_owned())
            .chain(content.lines().map(str::to_owned))
            .collect();
        let mut state = Self {
            shaped: vec![false; texts.len()],
            texts,
            focused: 1,
            ..Self::default()
        };
        state.shape();
        state
    }
    fn visible_count(&self) -> usize {
        if self.texts.len() == 11 { 11 } else { 5 }
    }
    fn shape(&mut self) {
        for n in self.first..(self.first + self.visible_count()).min(self.texts.len()) {
            self.shaped[n] = true;
        }
    }
    fn facts(&self, slot: u64) -> ElementFacts {
        let (locator, role, bounds) = match slot {
            2 => ("koru:w1/v1/s:explore#tree".into(), Role::Tree, None),
            3 => (
                "koru:w1/v1/n:00000000000000000000000000000064#root".into(),
                Role::TreeItem,
                None,
            ),
            4 => (
                "koru:w1/v1/s:body#root".into(),
                Role::Group,
                Some(Rect {
                    left: 0,
                    top: 0,
                    right: 500,
                    bottom: (self.visible_count() * 20) as i32,
                }),
            ),
            slot => {
                let index = slot as usize - 10;
                let visible = (self.first..self.first + self.visible_count()).contains(&index);
                let bounds = visible.then(|| Rect {
                    left: 10,
                    top: ((index - self.first) * 20) as i32,
                    right: 450,
                    bottom: ((index - self.first) * 20 + 18) as i32,
                });
                (
                    format!("koru:w1/v1/n:{:032x}#text", index + 1),
                    Role::Edit,
                    bounds,
                )
            }
        };
        let bounds = if self.clipped_entry && slot == 11 {
            Some(Rect {
                left: 10,
                top: -100,
                right: 450,
                bottom: 10,
            })
        } else {
            bounds
        };
        ElementFacts {
            handle: Handle {
                generation: 1,
                slot,
            },
            locator: Some(locator),
            role,
            enabled: true,
            offscreen: false,
            bounds,
            process_id: Some(if self.foreign && slot == 3 { 99 } else { 42 }),
        }
    }
}
struct Fake(Arc<Mutex<State>>);
impl Mechanism for Fake {
    fn perform(&mut self, request: Request) -> Response {
        let mut state = self.0.lock().unwrap();
        match request {
            Request::Matches { pattern, .. } => Response::Found(
                [2, 3, 4]
                    .into_iter()
                    .chain(10..10 + state.texts.len() as u64)
                    .map(|slot| state.facts(slot))
                    .filter(|facts| pattern.matches(facts.locator.as_deref().unwrap()))
                    .collect(),
            ),
            Request::FixtureChildren { root, .. } if root.slot == 2 => {
                Response::Found(vec![state.facts(3)])
            }
            Request::FixtureChildren { root, .. } if root.slot == 4 => {
                let mut found: Vec<_> = (10..10 + state.texts.len() as u64)
                    .map(|slot| state.facts(slot))
                    .collect();
                if state.duplicate {
                    found[1] = found[0].clone();
                }
                if state.reordered && state.snapshots == state.texts.len() {
                    found.swap(1, 2);
                }
                Response::Found(found)
            }
            Request::Check { element, condition } => Response::Checked(match condition {
                Condition::Selected(true) => {
                    !(state.changed_selection && state.snapshots == state.texts.len())
                }
                Condition::Name(_) => {
                    !state.wrong_selected_title
                        && !(state.changed_title && state.snapshots == state.texts.len())
                }
                Condition::Focused => element.slot == state.focused as u64 + 10,
                _ => panic!("unexpected condition {condition:?}"),
            }),
            Request::Focused { .. } => {
                Response::Found(vec![state.facts(state.focused as u64 + 10)])
            }
            Request::Snapshot { element, .. } => {
                let index = element.slot as usize - 10;
                assert!(state.shaped[index], "unlaid-out text has no TextPattern");
                state.snapshots += 1;
                Response::Snapshot(
                    ContentSnapshot::bounded(
                        state.facts(element.slot),
                        state.texts[index].clone(),
                        4096,
                    )
                    .unwrap(),
                )
            }
            Request::Input(consequence) => {
                assert_eq!(consequence.process_id, 42);
                assert_eq!(consequence.root_window, 77);
                match &consequence.action {
                    Action::Focus => {
                        let index = consequence.element.slot as usize - 10;
                        assert!(
                            in_document_viewport(
                                state.facts(consequence.element.slot).bounds,
                                state.facts(4).bounds
                            )
                            .unwrap()
                        );
                        state.focused = index;
                    }
                    Action::Chord(keys) if keys.as_slice() == [0x22] => {
                        state.first = (state.first + 4).min(state.texts.len() - 2);
                        state.focused = state.first + 1;
                        state.shape();
                        if let Some(cancel) = &state.cancel {
                            cancel.cancel();
                        }
                    }
                    Action::Chord(keys) if keys.as_slice() == [0x21] => {
                        state.first = state.first.saturating_sub(4);
                        state.focused = state.first + 1;
                        state.shape();
                    }
                    _ => panic!(
                        "preparation emitted content-changing input: {:?}",
                        consequence.action
                    ),
                }
                state.inputs.push(consequence.action);
                Response::Input(InputResult::Complete { bounds: None })
            }
            _ => panic!("unexpected request {request:?}"),
        }
    }
}
fn execute(
    name: &str,
    state: Arc<Mutex<State>>,
    cancel: &CancelToken,
    manifest: &mut Manifest,
) -> Result<(), &'static str> {
    let mut supervisor = Supervisor::new(
        Arc::new(move |_| Ok(Box::new(Fake(state.clone())))),
        Limits::default(),
    );
    Reader {
        supervisor: &mut supervisor,
        cancel,
        root: Handle {
            generation: 1,
            slot: 1,
        },
        process_id: 42,
        window_slot: 1,
    }
    .capture(
        manifest,
        name,
        Some("04/12/2099"),
        Some("3:30 PM"),
        &Navigation {
            window_handle: 77,
            patience: Patience::default(),
            clock: &FakeClock(Cell::new(Instant::now())),
        },
    )
}
#[test]
fn progressive_layout_is_read_through_navigation_and_entry_is_restored() {
    let state = Arc::new(Mutex::new(State::new("document-edit")));
    let mut manifest = manifest();
    execute(
        "document-edit",
        state.clone(),
        &CancelToken::new(),
        &mut manifest,
    )
    .unwrap();
    let state = state.lock().unwrap();
    assert!(state.shaped.iter().all(|shaped| *shaped));
    assert_eq!(state.first, 0);
    assert_eq!(state.focused, 1);
    assert_eq!(state.snapshots, state.texts.len());
    assert!(
        state
            .inputs
            .iter()
            .any(|action| *action == Action::Chord(vec![0x22]))
    );
    assert!(
        state
            .inputs
            .iter()
            .any(|action| *action == Action::Chord(vec![0x21]))
    );
    let fixture = manifest.document_edit.unwrap();
    assert_eq!(fixture.document, format!("{:032x}", 100));
    assert_eq!(fixture.title, format!("{:032x}", 1));
    assert_ne!(fixture.document, fixture.title);
    assert_eq!(fixture.filler_count, 40);
}
#[test]
fn visible_task_fixture_requires_no_input_and_ignores_raw_offscreen_false() {
    let state = Arc::new(Mutex::new(State::new("task-reminder")));
    let mut manifest = manifest();
    execute(
        "task-reminder",
        state.clone(),
        &CancelToken::new(),
        &mut manifest,
    )
    .unwrap();
    assert_eq!(manifest.task_reminder.unwrap().subjects.len(), 10);
    assert!(state.lock().unwrap().inputs.is_empty());
}
#[test]
fn cancellation_after_first_page_stops_without_restore_or_manifest_changes() {
    let cancel = CancelToken::new();
    let mut state = State::new("document-edit");
    state.cancel = Some(cancel.clone());
    let state = Arc::new(Mutex::new(state));
    let mut manifest = manifest();
    let before = manifest.clone();
    assert_eq!(
        execute("document-edit", state.clone(), &cancel, &mut manifest),
        Err("cancelled")
    );
    assert_eq!(manifest, before);
    let state = state.lock().unwrap();
    assert_eq!(state.inputs, vec![Action::Focus, Action::Chord(vec![0x22])]);
}
#[test]
fn wrong_catalog_title_body_title_foreign_or_duplicate_ids_refuse_before_navigation() {
    for (variant, expected) in [
        (0, "fixture-selected-title-mismatch"),
        (1, "fixture-title-mismatch"),
        (2, "fixture-children-invalid"),
        (3, "fixture-duplicate-paragraph"),
    ] {
        let mut state = State::new("document-edit");
        match variant {
            0 => state.wrong_selected_title = true,
            1 => state.texts[0] = "Another document".into(),
            2 => state.foreign = true,
            3 => state.duplicate = true,
            _ => unreachable!(),
        }
        let state = Arc::new(Mutex::new(state));
        let mut manifest = manifest();
        let before = manifest.clone();
        assert_eq!(
            execute(
                "document-edit",
                state.clone(),
                &CancelToken::new(),
                &mut manifest
            ),
            Err(expected)
        );
        assert_eq!(manifest, before);
        assert!(state.lock().unwrap().inputs.is_empty());
    }
}
#[test]
fn changed_selection_title_or_paragraph_order_never_publishes_manifest() {
    for (variant, expected) in [
        (0, "fixture-selected-document-changed"),
        (1, "fixture-selected-title-changed"),
        (2, "fixture-paragraph-order-changed"),
    ] {
        let mut state = State::new("document-edit");
        match variant {
            0 => state.changed_selection = true,
            1 => state.changed_title = true,
            2 => state.reordered = true,
            _ => unreachable!(),
        }
        let state = Arc::new(Mutex::new(state));
        let mut manifest = manifest();
        let before = manifest.clone();
        assert_eq!(
            execute("document-edit", state, &CancelToken::new(), &mut manifest),
            Err(expected)
        );
        assert_eq!(manifest, before);
    }
}

#[test]
fn partly_visible_entry_with_center_outside_body_refuses_before_input() {
    let mut state = State::new("document-edit");
    state.clipped_entry = true;
    let state = Arc::new(Mutex::new(state));
    let mut manifest = manifest();
    let before = manifest.clone();
    assert_eq!(
        execute(
            "document-edit",
            state.clone(),
            &CancelToken::new(),
            &mut manifest
        ),
        Err("fixture-entry-not-visible")
    );
    assert_eq!(manifest, before);
    assert!(state.lock().unwrap().inputs.is_empty());
}
