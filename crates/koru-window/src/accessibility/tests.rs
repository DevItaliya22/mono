// SPDX-License-Identifier: MPL-2.0

//! The parts of the adapter's lifetime that can be asserted without a window.
//!
//! The adapter itself needs a live `ActiveEventLoop`, so what is pinned here is
//! the mailbox — the piece that is written from whichever thread the platform
//! chose and read from the one that answers a person — and the visibility rule
//! that decides whether an adapter may be installed at all.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use accesskit::{Action, ActionHandler, ActionRequest, ActivationHandler, DeactivationHandler};
use accesskit::{NodeId, TreeId};

use super::{Handlers, Inbox, MAX_QUEUED_ACTIONS, may_install};

fn request(node: u64) -> ActionRequest {
    ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: NodeId(node),
        data: None,
    }
}

fn handlers() -> (Arc<Inbox>, Arc<AtomicUsize>, Handlers) {
    let inbox = Arc::new(Inbox::default());
    let wakes = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&wakes);
    let handlers = Handlers {
        inbox: Arc::clone(&inbox),
        wake: Arc::new(move || {
            counter.fetch_add(1, Ordering::Relaxed);
        }),
    };
    (inbox, wakes, handlers)
}

#[test]
fn an_adapter_is_refused_on_a_window_that_is_already_visible() {
    // `Adapter::with_direct_handlers` panics in this case, so the check has to
    // happen before it is called rather than being caught after.
    assert!(!may_install(Some(true)));
    assert!(may_install(Some(false)));
    // A platform that will not answer is not a refusal: the adapter's own
    // check is a moment away and is the authority.
    assert!(may_install(None));
}

#[test]
fn activation_is_recorded_rather_than_answered_and_wakes_the_loop() {
    let (inbox, wakes, mut handlers) = handlers();
    // `None` is the contract: the tree belongs to the loop, and this runs on
    // whichever thread the platform chose.
    assert!(handlers.request_initial_tree().is_none());
    assert!(handlers.request_initial_tree().is_none());
    assert_eq!(wakes.load(Ordering::Relaxed), 2);

    let pending = inbox.drain();
    // Coalesced: two attaches are still one full snapshot.
    assert!(pending.initial_tree);
    assert!(pending.actions.is_empty());
    assert!(!inbox.drain().initial_tree);
}

#[test]
fn actions_arrive_in_order_and_a_drain_empties_the_queue() {
    let (inbox, wakes, mut handlers) = handlers();
    handlers.do_action(request(7));
    handlers.do_action(request(9));
    assert_eq!(wakes.load(Ordering::Relaxed), 2);

    let pending = inbox.drain();
    let targets: Vec<u64> = pending.actions.iter().map(|a| a.target_node.0).collect();
    assert_eq!(targets, vec![7, 9]);
    assert_eq!(pending.dropped_actions, 0);
    assert!(inbox.drain().is_empty());
}

#[test]
fn a_client_that_queues_faster_than_the_loop_drains_cannot_grow_this_process() {
    let (inbox, _wakes, mut handlers) = handlers();
    for node in 0..(MAX_QUEUED_ACTIONS as u64 + 5) {
        handlers.do_action(request(node));
    }
    let pending = inbox.drain();
    assert_eq!(pending.actions.len(), MAX_QUEUED_ACTIONS);
    assert_eq!(pending.dropped_actions, 5);
    // The oldest go, so what the loop resolves is the most recent intent.
    assert_eq!(pending.actions[0].target_node.0, 5);
}

#[test]
fn deactivation_is_recorded_and_coalesced() {
    let (inbox, wakes, mut handlers) = handlers();
    handlers.deactivate_accessibility();
    handlers.deactivate_accessibility();
    assert_eq!(wakes.load(Ordering::Relaxed), 2);

    let pending = inbox.drain();
    assert!(pending.deactivated);
    assert!(!inbox.drain().deactivated);
}
