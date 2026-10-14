// SPDX-License-Identifier: MPL-2.0

//! What a reader gets back — and what a reader on a device that is not
//! executing the run gets back, which is the same history and no verb
//! (D15, UC5).

use koru_delegate_core::{ContextState, RunStatus};

use super::*;

#[test]
fn the_view_names_the_live_session_only_while_this_device_drives_the_run() {
    let mut h = Harness::new("view-session");
    let run = h.running(1);
    let view = h.orchestrator.view(run, None, 0, 10).unwrap();
    assert_eq!(
        view.session,
        Some(session(1)),
        "this device is driving the attempt"
    );

    // The run ends: there is no session to hand over any more.
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    h.registrations.lock().unwrap().next = vec![artifact(1)];
    h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(1) });
    assert_eq!(h.status(run), RunStatus::Completed);
    let view = h.orchestrator.view(run, None, 0, 10).unwrap();
    assert_eq!(view.session, None);
}

#[test]
fn the_view_carries_what_each_pin_points_at_now_and_says_nothing_when_it_cannot_tell() {
    let mut h = Harness::new("view-pins");
    let run = h.running(2);
    // Nothing resolved: the surface says nothing about the source rather
    // than saying it is gone.
    assert!(
        h.orchestrator
            .view(run, None, 0, 10)
            .unwrap()
            .context
            .is_empty()
    );

    h.capture.lock().unwrap().states = vec![ContextState::InTrash];
    let view = h.orchestrator.view(run, None, 0, 10).unwrap();
    assert_eq!(view.header.context.len(), 1);
    assert_eq!(view.context, vec![ContextState::InTrash]);
    // The pin itself did not move.
    assert_eq!(
        view.header.context[0].document_version().map(|(_, v)| v),
        Some(koru_delegate_core::DocumentVersionId::from_uuid(v4(0xf1)))
    );
}

#[test]
fn a_device_that_did_not_create_the_run_reads_it_by_workspace_and_holds_no_session() {
    // One device creates and drives the run; a second reads the Run
    // document that reached it, with no creation record of its own.
    let mut executing = Harness::new("view-here");
    let run = executing.running(3);
    executing.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    let synced = executing.documents.lock().unwrap().snapshots.clone();
    let steps_there = executing.steps(run).len();

    let mut reading = Harness::new("view-elsewhere");
    reading.documents.lock().unwrap().snapshots = synced;

    // Without the workspace there is nothing to find it by.
    assert!(matches!(
        reading
            .orchestrator
            .view(run, None, 0, usize::MAX)
            .unwrap_err(),
        RunError::UnknownRun
    ));

    let view = reading
        .orchestrator
        .view(run, Some(workspace()), 0, usize::MAX)
        .unwrap();
    assert_eq!(view.header.run, run);
    assert_eq!(view.timeline.items.len(), steps_there);
    assert!(matches!(view.header.status, RunStatus::Running { .. }));
    assert_eq!(
        view.header.executing_device(),
        Some(DeviceId::from_uuid(v4(0xe1))),
        "the reader can name the device running the work (D16)"
    );
    assert_eq!(
        view.session, None,
        "no session here, so the surface offers no handover (D15)"
    );
}

#[test]
fn a_reader_takes_in_what_arrived_and_a_driver_has_nothing_to_take_in() {
    let mut h = Harness::new("absorb");
    let run = h.running(4);
    assert!(
        !h.orchestrator.absorb(run).unwrap(),
        "this device writes the document; there is nothing to take in"
    );
}

/// **A run that has finished still says where it worked.**
///
/// The Overview's *Working folder* section read only *A folder Koru made for
/// this run*, with no path — which is what the maintainers saw. The cause is here,
/// not in the drawing: `end_attempt` takes the attempt the moment a run ends,
/// and the view read the folder off that attempt, so the folder was present
/// exactly while nobody needed it and absent for every run a person comes
/// back to judge. The durable authority record answers instead, and it is the
/// path *as this device resolved it when the run started* — the same one a
/// delete reads, never re-derived.
#[test]
fn a_finished_run_still_says_which_folder_it_worked_in() {
    let mut h = Harness::new("view-folder");
    let run = h.running(5);
    let while_running = h
        .orchestrator
        .view(run, None, 0, 10)
        .unwrap()
        .run_workspace
        .expect("a live attempt knows its folder");

    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    h.registrations.lock().unwrap().next = vec![artifact(1)];
    h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(1) });
    assert_eq!(h.status(run), RunStatus::Completed);
    assert!(
        h.orchestrator
            .runs
            .get(&run)
            .is_some_and(|state| state.attempt.is_none()),
        "the attempt is gone, which is why this test exists"
    );

    assert_eq!(
        h.orchestrator.view(run, None, 0, 10).unwrap().run_workspace,
        Some(while_running),
        "the folder the run worked in outlives the attempt that used it"
    );
}

/// ...and a run this device has no record of having launched says nothing,
/// rather than refusing the read. A folder is one line of an Overview; a run
/// whose folder cannot be named is still a run worth reading.
#[test]
fn a_run_with_no_authority_record_says_nothing_about_a_folder() {
    let mut h = Harness::new("view-folder-absent");
    let run = h.running(6);
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    h.registrations.lock().unwrap().next = vec![artifact(1)];
    h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(1) });

    h.authority.lock().unwrap().workspaces.remove(&run);
    let view = h.orchestrator.view(run, None, 0, 10).unwrap();
    assert_eq!(view.run_workspace, None);
    assert_eq!(view.header.run, run, "and the run still reads");
}
