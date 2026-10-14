// SPDX-License-Identifier: MPL-2.0

//! **Deleting a run**, through the orchestrator: the refusal that keeps
//! trashing reversible, and the empty — including the one rule in it that
//! can destroy work.
//!
//! The folder cases are on a real filesystem, under this process's own
//! temporary directory, because the failure they prevent is a real
//! `remove_dir_all` on somebody's repository. `koru-delegate-scope` holds
//! the rule and its own tests; these are the orchestrator asking it with
//! the scope the ledger recorded and the path the authority record kept —
//! which is where the two could be wired to each other wrongly.

use koru_delegate_core::{ArtifactDisposition, FolderOutcome, RunStatus};

use super::*;

/// A run that has ended, in the project scope — what a person would be
/// putting in the trash. Cancelled between turns, which is the one door a
/// test has to a terminal status without a provider.
fn finished(h: &mut Harness, n: u8) -> RunId {
    let run = h.running(n);
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(1) });
    h.orchestrator.cancel(run).unwrap();
    assert!(h.status(run).is_terminal(), "{:?}", h.status(run));
    run
}

/// **A run that is still working refuses to be trashed.**
///
/// Trashing is meant to be reversible, and a run still writing into a
/// folder is the one case where it is not: the agent keeps going, keeps
/// writing, and keeps registering results into a run the person believes
/// they put away. Cancel it first — a verb they already have.
#[test]
fn a_working_run_refuses_to_be_trashed_and_says_which_status_it_is_in() {
    let mut h = Harness::new("trash-working");
    let run = h.running(1);
    assert!(matches!(h.status(run), RunStatus::Running { .. }));

    let refusal = h
        .orchestrator
        .trash(run, ArtifactDisposition::Keep)
        .unwrap_err();
    assert_eq!(refusal.code(), "wrong_status");
    assert!(
        h.orchestrator.deletion_recorded(run).is_none(),
        "and nothing was recorded about a run that was refused"
    );

    // Cancelled between turns, it goes in.
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(1) });
    h.orchestrator.cancel(run).unwrap();
    assert!(h.orchestrator.trash(run, ArtifactDisposition::Keep).is_ok());
}

/// Trashing records what to do with the results and answers with the
/// document, which is the half the window performs. Restoring forgets the
/// decision — nothing was deleted, so there is nothing to put back.
#[test]
fn trashing_records_the_decision_and_names_the_document_restoring_forgets_it() {
    let mut h = Harness::new("trash-records");
    let run = finished(&mut h, 1);

    let place = h
        .orchestrator
        .trash(run, ArtifactDisposition::Delete)
        .unwrap();
    assert_eq!(place.header.run, run);
    assert_eq!(place.workspace, workspace());
    assert_eq!(
        Some(place.document),
        h.documents.lock().unwrap().by_run.get(&run).copied(),
        "the document the window has to move into the bin"
    );
    assert_eq!(
        h.orchestrator.deletion_recorded(run),
        Some(ArtifactDisposition::Delete)
    );

    let back = h.orchestrator.restore(run).unwrap();
    assert_eq!(back.document, place.document);
    assert_eq!(h.orchestrator.deletion_recorded(run), None);
}

/// **The trap.** Emptying the trash on a run scoped to a project root
/// leaves that project exactly where it was — it is the person's own
/// repository, and the run was a visitor in it. The trash says so with
/// `Kept` rather than saying nothing, because an operation that quietly
/// does less than it offered teaches somebody that it did more.
#[test]
fn emptying_the_trash_on_a_project_scoped_run_leaves_the_project_on_disk() {
    let mut h = Harness::new("purge-project");
    let run = finished(&mut h, 1);
    let project_dir = h.dir.root.join("project");
    let theirs = project_dir.join("their-work.md");
    std::fs::write(&theirs, b"a year of somebody's work").unwrap();
    h.orchestrator
        .trash(run, ArtifactDisposition::Keep)
        .unwrap();

    let purged = h.orchestrator.purge(run).unwrap();

    assert_eq!(purged.folder, FolderOutcome::Kept);
    assert!(project_dir.is_dir(), "their repository is still there");
    assert_eq!(
        std::fs::read(&theirs).unwrap(),
        b"a year of somebody's work",
        "and so is what was in it"
    );
    // The run itself is gone from everything this device holds.
    assert_eq!(
        h.orchestrator.header(run).unwrap_err().code(),
        "unknown_run"
    );
    assert_eq!(h.authority.lock().unwrap().forgotten, [run]);
}

/// A folder Koru made goes with the run (D37).
#[test]
fn emptying_the_trash_removes_a_temporary_folder() {
    let mut h = Harness::new("purge-temporary");
    let run = run_id(2);
    h.orchestrator
        .create(request(run, WorkspaceScope::Temporary), workspace())
        .unwrap();
    h.orchestrator.start(run, StartOrigin::Manual).unwrap();
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(1) });
    h.orchestrator.cancel(run).unwrap();
    let folder = h
        .authority
        .lock()
        .unwrap()
        .workspaces
        .get(&run)
        .cloned()
        .expect("the record kept where it worked");
    std::fs::write(folder.join("notes.md"), b"what the agent wrote").unwrap();
    h.orchestrator
        .trash(run, ArtifactDisposition::Keep)
        .unwrap();

    let purged = h.orchestrator.purge(run).unwrap();

    assert_eq!(purged.folder, FolderOutcome::Removed);
    assert!(!folder.exists(), "the folder Koru made is gone");
}

/// **The path is the one the run worked in, not one re-derived now.** The
/// record's path is what is removed; a scope resolved again would name a
/// different directory whenever a base has been repointed, and that is how
/// a delete removes the wrong thing.
#[test]
fn the_folder_removed_is_the_one_the_record_kept() {
    let mut h = Harness::new("purge-recorded-path");
    let run = run_id(2);
    h.orchestrator
        .create(request(run, WorkspaceScope::Temporary), workspace())
        .unwrap();
    h.orchestrator.start(run, StartOrigin::Manual).unwrap();
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(1) });
    h.orchestrator.cancel(run).unwrap();

    // Where the run actually worked, and where a fresh resolution would
    // now point instead.
    let worked_in = h.authority.lock().unwrap().workspaces[&run].clone();
    let elsewhere = h.dir.root.join("repointed");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("not-the-run.md"), b"somebody else's").unwrap();

    h.orchestrator
        .trash(run, ArtifactDisposition::Keep)
        .unwrap();
    assert_eq!(
        h.orchestrator.purge(run).unwrap().folder,
        FolderOutcome::Removed
    );

    assert!(!worked_in.exists());
    assert!(
        elsewhere.join("not-the-run.md").is_file(),
        "the folder a fresh resolution would have named was not touched"
    );
}

/// A run whose folder this device never resolved — one that never started —
/// is `Absent`, which is an answer rather than a failure.
#[test]
fn a_run_that_never_started_has_no_folder_to_remove() {
    let mut h = Harness::new("purge-never-started");
    let run = run_id(3);
    h.orchestrator
        .create(request(run, WorkspaceScope::Temporary), workspace())
        .unwrap();
    h.orchestrator.cancel(run).unwrap();
    h.orchestrator
        .trash(run, ArtifactDisposition::Keep)
        .unwrap();

    assert_eq!(
        h.orchestrator.purge(run).unwrap().folder,
        FolderOutcome::Absent
    );
}

/// **Keep leaves the results alone; Delete retires the families the run
/// made.** Two answers to the one question the trash asks, acted on at the
/// empty and never before it.
#[test]
fn the_results_go_only_when_the_person_said_they_should() {
    for (disposition, expected) in [
        (ArtifactDisposition::Keep, 0),
        (ArtifactDisposition::Delete, 2),
    ] {
        let mut h = Harness::new("purge-results");
        h.registrations.lock().unwrap().retires = 2;
        let run = finished(&mut h, 1);
        h.orchestrator.trash(run, disposition).unwrap();

        let purged = h.orchestrator.purge(run).unwrap();
        assert_eq!(purged.artifacts_retired, expected, "{disposition:?}");
        let retired = h.registrations.lock().unwrap().retired.clone();
        assert_eq!(
            retired.len(),
            usize::from(disposition.deletes()),
            "{disposition:?} — asked exactly when it should be"
        );
        if disposition.deletes() {
            assert_eq!(retired[0], (run, workspace()));
        }
    }
}

/// **A run nobody was asked about keeps what it produced.** Keep is the
/// default and the safe one; nothing infers *delete* from silence.
#[test]
fn a_run_with_no_decision_keeps_its_results() {
    let mut h = Harness::new("purge-no-decision");
    let run = finished(&mut h, 1);
    // Purged without ever going through the trash — a ledger from a build
    // that predates the question looks exactly like this.
    let purged = h.orchestrator.purge(run).unwrap();
    assert_eq!(purged.artifacts_retired, 0);
    assert!(h.registrations.lock().unwrap().retired.is_empty());
}

/// The empty asks about the status again rather than trusting that the
/// trash asked: it is the irreversible half.
#[test]
fn the_empty_refuses_a_run_that_is_somehow_still_working() {
    let mut h = Harness::new("purge-working");
    let run = h.running(1);
    assert_eq!(
        h.orchestrator.purge(run).unwrap_err().code(),
        "wrong_status"
    );
    assert!(
        h.authority.lock().unwrap().forgotten.is_empty(),
        "and nothing was forgotten"
    );
}
