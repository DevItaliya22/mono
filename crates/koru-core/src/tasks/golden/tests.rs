// SPDX-License-Identifier: MPL-2.0

//! **The reference run**: every golden case, executed against the commands
//! themselves.
//!
//! This is what makes the table truthful. `golden.rs` is data, and data can
//! say anything; the only thing that makes a row an expectation rather than a
//! wish is a runner that fails when the domain disagrees with it. An adapter's
//! run then compares its *host* against a table this suite has already proved
//! the domain produces — so a red case in `koru-editor` or `koru-mcp-server`
//! means the wrapper drifted, never that the row was fiction.

use super::*;

/// The whole table, in one test, reporting **every** disagreement rather than
/// the first: a change to a command usually moves several rows, and finding
/// them one `cargo test` at a time is the slow way to learn that.
#[test]
fn the_golden_cases_are_what_the_commands_do() {
    let mut bad: Vec<String> = Vec::new();
    for case in CASES {
        let mut fx = Fixture::build(case);
        let target = fx.node(case.command.target());
        let got = case.command.apply_to(&mut fx.ws, target);
        if let Err(complaint) = verify(case, &fx, &got) {
            bad.push(complaint);
        }
    }
    assert!(
        bad.is_empty(),
        "{} of {} golden cases disagree with the commands:\n\n{}",
        bad.len(),
        CASES.len(),
        bad.join("\n\n")
    );
}

/// An id is how a failure in another crate names a row, so two rows may not
/// share one.
#[test]
fn every_case_id_is_unique_and_says_which_command_it_is() {
    let mut seen: Vec<&str> = Vec::new();
    for case in CASES {
        assert!(
            !seen.contains(&case.id),
            "two golden cases are called {:?}",
            case.id
        );
        assert!(
            !case.what.is_empty(),
            "{} says nothing about what it holds down",
            case.id
        );
        seen.push(case.id);
    }
}

/// The coverage the roadmap fixed for this table. Asserted rather than
/// reviewed, because a row deleted during a refactor is invisible otherwise.
#[test]
fn the_table_covers_every_command_and_both_refusals() {
    let names: Vec<&str> = CASES.iter().map(|c| c.command.name()).collect();
    for command in [
        "ensure_task",
        "set_due",
        "clear_due",
        "complete_task",
        "reopen_task",
        "remove_from_tasks",
        "reclassify",
        "reclassify_moved",
    ] {
        assert!(names.contains(&command), "no golden case runs {command}");
    }

    let refusals: Vec<Refusal> = CASES
        .iter()
        .filter_map(|c| match c.expect {
            Expect::Refused(why) => Some(why),
            _ => None,
        })
        .collect();
    assert!(refusals.contains(&Refusal::Task(TaskRefusal::KindNotCapable)));
    assert!(refusals.contains(&Refusal::Task(TaskRefusal::NotATask)));
    assert!(
        refusals.contains(&Refusal::Attribute("koru.due")),
        "the value-validation refusal is a different guard and needs its own row"
    );

    // Both shapes of "nothing to do": a command's guarded no-op, and the
    // classification hook answering `None`.
    assert!(
        CASES.iter().any(|c| matches!(
            c.expect,
            Expect::Applied(Applied {
                effect: TaskEffect::Unchanged,
                ..
            })
        )),
        "no golden case exercises a guarded no-op"
    );
    assert!(CASES.iter().any(|c| matches!(c.expect, Expect::Nothing)));
}

/// Every case declares what happened to every node it built. Checked here as
/// well as inside [`verify`], so a case with no `after` at all is caught even
/// if its command refuses.
#[test]
fn a_case_says_what_happened_to_every_node_it_declared() {
    for case in CASES {
        let Expect::Applied(applied) = case.expect else {
            continue;
        };
        for spec in case.nodes {
            assert!(
                applied.after.iter().any(|a| a.label == spec.label),
                "{}: `after` never says what happened to {:?}",
                case.id,
                spec.label
            );
        }
    }
}

/// The causation id is minted by the command, per run — which is what lets a
/// wrapper carry it as `correlation_id` and group a cascade the same way on
/// both hosts. Two runs of one case must not share one.
#[test]
fn a_command_mints_its_own_causation_id() {
    let case = case("complete.cascades_over_checkables_and_nested_tasks")
        .expect("the cascade case is in the table");
    let mut ids = Vec::new();
    for _ in 0..2 {
        let mut fx = Fixture::build(case);
        let target = fx.node(case.command.target());
        let outcome = case
            .command
            .apply_to(&mut fx.ws, target)
            .expect("the cascade case applies")
            .expect("and answers an outcome");
        ids.push(outcome.causation);
    }
    assert_ne!(ids[0], ids[1], "two runs shared one causation id");
}

/// A label the case never declared reads back as `""` rather than panicking,
/// so a wrapper that invents an intent about some other node fails as a
/// mismatch a reader can see.
#[test]
fn an_unknown_node_has_no_label() {
    let case = case("ensure.explicit").expect("the first case is in the table");
    let mut fx = Fixture::build(case);
    let stranger = fx
        .ws
        .insert(
            crate::schema::Parent::Root,
            crate::node::Node::paragraph("nobody named me").unwrap(),
        )
        .unwrap();
    assert_eq!(fx.label(stranger), "");
}
