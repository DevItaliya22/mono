// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn creation_stays_visible_until_its_snapshot_rows_are_published() {
    for initial_state in [
        OptimisticState::Pending,
        OptimisticState::Succeeded,
        OptimisticState::Failed {
            reason: Failure::Unavailable,
        },
    ] {
        let current = target();
        let mut controller = Controller::default();
        controller.target = Some(current.clone());
        controller.context = Some(context());
        controller.creation = Some(Creation::loading(request(1)));
        controller.confirm_creation(draft());
        let create = controller.pending.pop_front().expect("queued creation");
        if matches!(initial_state, OptimisticState::Failed { .. }) {
            controller.retain_failure(create, Failure::Unavailable);
        } else {
            controller.optimistic[0].presentation.state = initial_state;
        }
        let id = controller.optimistic[0].presentation.reminder_id.clone();
        let row = GlobalItem {
            target: current.entity.to_storage(),
            title: "new reminder".into(),
            source_location: String::new(),
            reminder_id: id.clone(),
            revision: RuleRevisionId::new().to_storage(),
            occurrence: Some("new-occurrence".into()),
            sort_at_ms: Some(1),
            draft: draft(),
            state: GlobalItemState::ComingUp,
            snoozed: false,
            rule_paused: false,
        };
        let mut snapshot = loaded(Default::default());
        snapshot.global_updating = true;
        snapshot.present_ids.insert(id.clone());
        snapshot.global_rows.push(row.clone());

        // Store truth already contains the rule, but the document projection
        // is still updating. Repeated partial reads cannot leave a blank row.
        for _ in 0..2 {
            assert!(controller.apply_stamped(StampedAnswer {
                target: current.clone(),
                generation: controller.generation,
                load_epoch: Some(controller.load_epoch),
                answer: Answer::Loaded(snapshot.clone()),
            }));
            assert!(controller.global().rows.is_empty());
            let visible = controller
                .optimistic_creations()
                .filter(|record| !matches!(record.state, OptimisticState::Failed { .. }))
                .map(|record| record.reminder_id.clone())
                .collect::<Vec<_>>();
            assert_eq!(visible, vec![id.clone()], "partial snapshot lost the pill");
            assert!(controller.failed_creations.is_empty());
        }

        // The complete snapshot replaces the optimistic record in the same
        // transition, so exactly one visible reminder survives the handoff.
        snapshot.global_updating = false;
        for _ in 0..2 {
            assert!(controller.apply_stamped(StampedAnswer {
                target: current.clone(),
                generation: controller.generation,
                load_epoch: Some(controller.load_epoch),
                answer: Answer::Loaded(snapshot.clone()),
            }));
            assert!(controller.optimistic_creations().next().is_none());
            assert_eq!(controller.global().rows, vec![row.clone()]);
        }
    }
}
