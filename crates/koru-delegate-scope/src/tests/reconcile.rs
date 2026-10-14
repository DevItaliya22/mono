// SPDX-License-Identifier: MPL-2.0

//! A worker restart with a live claim: the registry is rebuilt from the
//! durable port, a claim whose attempt is live or unknown still holds its
//! folder, and one whose attempt ended is free. The folders are real —
//! reconciliation resolves them again — so this is the disk exception too.

use super::*;

fn recorded(run: u8, attempt: u8, scope: WorkspaceScope) -> DurableClaim {
    DurableClaim {
        run: run_id(run),
        attempt: attempt_id(attempt),
        scope,
    }
}

#[test]
fn a_live_claim_survives_the_restart_and_still_warns_and_skips() {
    let temp = TempDir::new("restart-live");
    let root = temp.dir("project");
    temp.dir("project/src");
    let roots = Roots::default().project(project_id(1), &root);
    let durable = FakeDurable {
        claims: Ok(vec![recorded(1, 11, root_scope(1))]),
    };

    let (mut claims, report) =
        ScopeClaims::reconcile(&durable, &roots, |_| Liveness::Live).unwrap();
    assert_eq!(report.kept, vec![(run_id(1), attempt_id(11))]);
    assert!(report.ended.is_empty());
    assert!(report.unresolvable.is_empty());
    assert_eq!(claims.len(), 1);

    let manual = claims.admit(
        run_id(2),
        attempt_id(22),
        &folder_scope(1, "src"),
        &roots,
        StartOrigin::Manual,
    );
    let Admission::Admitted {
        warning: Some(warning),
    } = manual
    else {
        panic!("a manual start proceeds after a warning: {manual:?}");
    };
    assert_eq!(warning.holder.run, run_id(1));
    assert_eq!(warning.holder.attempt, attempt_id(11));
    assert_eq!(
        warning.holder.overlap,
        Overlap::Nested {
            inner: Party::Candidate,
            outer: Party::Holder,
        }
    );

    let fired = claims.admit(
        run_id(3),
        attempt_id(33),
        &root_scope(1),
        &roots,
        scheduled(5),
    );
    let Admission::Skipped { schedule, busy } = fired else {
        panic!("a scheduled firing is skipped: {fired:?}");
    };
    assert_eq!(schedule, super::schedule(5));
    let holders: Vec<_> = busy.holders().map(|h| (h.run, h.attempt)).collect();
    assert_eq!(
        holders,
        vec![(run_id(1), attempt_id(11)), (run_id(2), attempt_id(22))],
        "the restart's holder and the person who proceeded are both recorded"
    );
}

#[test]
fn an_unknown_attempt_is_possibly_running_and_keeps_its_claim() {
    let temp = TempDir::new("restart-unknown");
    let root = temp.dir("project");
    let roots = Roots::default().project(project_id(1), &root);
    let durable = FakeDurable {
        claims: Ok(vec![recorded(1, 11, root_scope(1))]),
    };

    let (mut claims, report) =
        ScopeClaims::reconcile(&durable, &roots, |_| Liveness::Unknown).unwrap();
    assert_eq!(report.kept, vec![(run_id(1), attempt_id(11))]);
    assert!(matches!(
        claims.admit(
            run_id(2),
            attempt_id(22),
            &root_scope(1),
            &roots,
            scheduled(5)
        ),
        Admission::Skipped { .. }
    ));
}

#[test]
fn an_ended_attempt_is_dropped_and_its_folder_is_free() {
    let temp = TempDir::new("restart-ended");
    let root = temp.dir("project");
    let roots = Roots::default().project(project_id(1), &root);
    let durable = FakeDurable {
        claims: Ok(vec![
            recorded(1, 11, root_scope(1)),
            recorded(2, 22, root_scope(1)),
        ]),
    };

    let (mut claims, report) = ScopeClaims::reconcile(&durable, &roots, |attempt| {
        if attempt == attempt_id(11) {
            Liveness::Ended
        } else {
            Liveness::Live
        }
    })
    .unwrap();
    assert_eq!(report.ended, vec![(run_id(1), attempt_id(11))]);
    assert_eq!(report.kept, vec![(run_id(2), attempt_id(22))]);
    assert_eq!(claims.len(), 1);

    claims.release(attempt_id(22));
    assert_eq!(
        claims.admit(
            run_id(3),
            attempt_id(33),
            &root_scope(1),
            &roots,
            scheduled(5)
        ),
        Admission::Admitted { warning: None }
    );
}

#[test]
fn a_claim_whose_folder_is_gone_is_dropped_and_reported() {
    let temp = TempDir::new("restart-gone");
    let root = temp.dir("project");
    let roots = Roots::default()
        .project(project_id(1), &root)
        .temporary(run_id(3), &temp.path().join("never-created"));
    let durable = FakeDurable {
        claims: Ok(vec![
            recorded(1, 11, folder_scope(1, "removed")),
            recorded(2, 22, root_scope(9)),
            recorded(3, 33, WorkspaceScope::Temporary),
            recorded(4, 44, WorkspaceScope::None),
        ]),
    };

    let (claims, report) = ScopeClaims::reconcile(&durable, &roots, |_| Liveness::Unknown).unwrap();
    assert!(claims.is_empty());
    assert!(report.kept.is_empty());
    assert_eq!(
        report.unresolvable,
        vec![
            (run_id(1), attempt_id(11), ScopeRefusal::NotFound),
            (run_id(2), attempt_id(22), ScopeRefusal::UnknownProject),
            (run_id(3), attempt_id(33), ScopeRefusal::NotFound),
            (run_id(4), attempt_id(44), ScopeRefusal::NotAdmissible),
        ]
    );
}

#[test]
fn a_temporary_claim_is_reconciled_through_the_runs_own_directory() {
    let temp = TempDir::new("restart-temporary");
    let owned = temp.dir("run-1");
    let roots = Roots::default().temporary(run_id(1), &owned);
    let durable = FakeDurable {
        claims: Ok(vec![recorded(1, 11, WorkspaceScope::Temporary)]),
    };

    let (claims, report) = ScopeClaims::reconcile(&durable, &roots, |_| Liveness::Live).unwrap();
    assert_eq!(report.kept.len(), 1);
    assert_eq!(claims.claims().next().unwrap().scope, identity(&owned));
}

#[test]
fn a_durable_port_that_cannot_answer_fails_reconciliation_rather_than_starting_empty() {
    let roots = Roots::default();
    let durable = FakeDurable {
        claims: Err("runs.db unreadable"),
    };
    let result = ScopeClaims::reconcile(&durable, &roots, |_| Liveness::Live);
    assert_eq!(result.err(), Some("runs.db unreadable"));
}
