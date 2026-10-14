// SPDX-License-Identifier: MPL-2.0

//! What the numbers mean, asserted against a fabricated answer.
//!
//! **Nothing here dials a store.** A panel with no client starts no thread and
//! makes no request, which is the same guard `SearchSurface`'s `no-reader`
//! feature exists for — here it comes free, because the client arrives from
//! the draw and a test never draws with one.
//!
//! **The bucket rules are not asserted here any more, and that is the point.**
//! They used to be, against a copy of the rule that lived in this file and had
//! already drifted from `koru_core::tasks::bucket` (no thirty-day horizon). The
//! rule is the store's now, held equal to the domain function over a
//! five-thousand-task corpus by `bucket_placement_agrees_with_the_domain_function`
//! in `koru-projection`. What is left here is the adapter: which questions are
//! asked, and how an answer becomes the rail's five numbers.

use super::*;
use koru_store_wire::tasks::{TaskGroup, TaskQueryResponse};

fn day(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

fn answer(groups: Vec<TaskGroup>) -> TaskQueryResponse {
    TaskQueryResponse {
        rows: Vec::new(),
        definitions: Vec::new(),
        tags: Vec::new(),
        groups,
        projected: true,
        updating_workspaces: Vec::new(),
        limit: 1,
        next_after: None,
        restarted: false,
        covered: None,
    }
}

fn group(key: &str, count: usize, overdue: usize) -> TaskGroup {
    TaskGroup {
        workspace: koru_store_core::ids::WorkspaceId::new(),
        list: key.to_owned(),
        count,
        overdue,
        trashed: 0,
    }
}

/// **The five numbers come from five group rows, and nothing is folded here.**
/// The overdue split is the store's own, computed against the windows the
/// query carried rather than by a second pass over rows nobody fetched.
#[test]
fn the_rails_numbers_are_the_buckets_groups() {
    let counts = counts_of(&answer(vec![
        group("today", 5, 3),
        group("next", 12, 0),
        group("upcoming", 40, 0),
        group("later", 9, 0),
        group("completed", 122, 0),
    ]));
    assert!(counts.known);
    assert_eq!(counts.today, 5);
    assert_eq!(counts.today_overdue, 3);
    assert_eq!(counts.next, 12);
    assert_eq!(counts.upcoming, 40);
    assert_eq!(counts.later, 9);
    assert_eq!(counts.completed, 122);
}

/// **A corpus-wide count is never a floor.** It used to be: the page was fetched
/// at `MAX_LIMIT` and bucketed here, so a workspace with more than five hundred
/// tasks drew `500+` in every queue. `capped` stays in [`TaskCounts`] because
/// it is the honest thing to say the day a count comes off a page again.
#[test]
fn a_grouped_count_is_never_capped() {
    let counts = counts_of(&answer(vec![group("today", 4_000, 12)]));
    assert!(!counts.capped);
    assert_eq!(counts.today, 4_000);
}

/// A bucket nothing matches has **no row** rather than a zero — the store omits
/// it, and the rail draws all five either way.
#[test]
fn a_missing_group_is_a_zero_not_an_unknown() {
    let counts = counts_of(&answer(vec![group("today", 2, 0)]));
    assert!(counts.known, "an answer is an answer even with one group");
    assert_eq!((counts.next, counts.upcoming, counts.later), (0, 0, 0));
}

/// A bucket token a newer store knows and this build does not is counted
/// nowhere. A number in the wrong queue is worse than a number that is missing.
#[test]
fn an_unknown_bucket_token_is_not_folded_into_a_neighbour() {
    let counts = counts_of(&answer(vec![group("today", 3, 1), group("someday", 9, 0)]));
    assert_eq!(counts.today, 3);
    assert_eq!(
        counts.today + counts.next + counts.upcoming + counts.later + counts.completed,
        3
    );
}

/// The per-list table is keyed by stored id, and a list nothing matches has no
/// row — which the panel reads as a zero rather than as an unknown.
#[test]
fn the_list_groups_key_by_stored_id() {
    let groups = groups_of(&answer(vec![group("list-a", 14, 0), group("list-b", 6, 1)]));
    assert_eq!(groups.get("list-a"), Some(&(14, 0)));
    assert_eq!(groups.get("list-b"), Some(&(6, 1)));
    assert_eq!(groups.get("list-c"), None);
}

/// **Both statements ask for one row and read none of it.** The groups are
/// corpus-wide; a page beside them would be a page nobody looks at.
#[test]
fn neither_statement_fetches_a_page() {
    let workspace = koru_store_core::ids::WorkspaceId::new();
    let windows = crate::tasks_view::query::windows(day("2026-08-31"), 1_756_600_000_000);
    let buckets = bucket_query(workspace, &windows, day("2026-08-31"));
    assert_eq!(buckets.group_by.as_deref(), Some("bucket"));
    assert!(
        buckets.bucket_windows.is_some(),
        "a bucket needs the reader's windows, and the store refuses one without"
    );
    assert_eq!(buckets.limit, 1);
    let lists = list_query(workspace, day("2026-08-31"), 1_756_600_000_000);
    assert_eq!(lists.group_by.as_deref(), Some("list"));
    assert_eq!(lists.limit, 1);
}

/// The list statement states the reader's overdue windows, because overdue is a
/// fact about a display zone and the store carries no timezone database.
#[test]
fn the_list_statement_states_the_readers_overdue_windows() {
    let workspace = koru_store_core::ids::WorkspaceId::new();
    let q = list_query(workspace, day("2026-08-31"), 1_756_600_000_000);
    assert!(matches!(q.lifecycle, LifecycleFilter::Open));
    let overdue = q.overdue.expect("the reader defines overdue");
    assert_eq!(
        overdue.dates.and_then(|w| w.before).as_deref(),
        Some("2026-08-31")
    );
    assert_eq!(
        overdue.instants.and_then(|w| w.before_ms),
        Some(1_756_600_000_000)
    );
}

/// The bucket statement names **no lifecycle**, and that is not an omission:
/// grouping by bucket partitions by lifecycle, so the field would be ignored.
#[test]
fn the_bucket_statement_leaves_lifecycle_to_the_grouping() {
    let workspace = koru_store_core::ids::WorkspaceId::new();
    let windows = crate::tasks_view::query::windows(day("2026-08-31"), 1_756_600_000_000);
    let q = bucket_query(workspace, &windows, day("2026-08-31"));
    assert!(
        matches!(q.lifecycle, LifecycleFilter::Open),
        "the default is left alone rather than set to something the store drops"
    );
    assert!(
        q.bucket.is_none(),
        "this counts all five, it does not page one"
    );
}

/// A panel with no client makes no request and no thread — every unit test in
/// this crate builds one, so this is the guard that keeps `cargo test` off a
/// developer's own store.
#[test]
fn a_panel_with_no_client_dials_nothing() {
    let mut panel = TaskCountsPanel::default();
    let now = Instant::now();
    let workspace = Some(WorkspaceId::new());
    assert!(
        panel.maintain(workspace, true, now),
        "the workspace changed"
    );
    assert!(!panel.counts().known, "and there is nothing to draw yet");
    assert!(panel.receiver.is_none());
    assert!(panel.notice.is_none());
}

/// A hidden rail has no deadline, which is what makes "no traffic while the
/// sidebar shows documents" structural rather than a promise.
#[test]
fn a_hidden_rail_asks_for_nothing() {
    let mut panel = TaskCountsPanel::default();
    let now = Instant::now();
    let workspace = Some(WorkspaceId::new());
    panel.maintain(workspace, false, now);
    assert_eq!(panel.deadline(), None);
    panel.maintain(workspace, true, now);
    assert!(panel.deadline().is_some());
    panel.maintain(workspace, false, now);
    assert_eq!(panel.deadline(), None);
    // An invalidation from a task mutation while it is hidden is not a reason
    // to wake up either.
    panel.invalidate();
    assert_eq!(panel.deadline(), None);
}

// ----------------------------------------------------------- the wait (P)

fn covered(value: Option<bool>) -> TaskQueryResponse {
    TaskQueryResponse {
        covered: value,
        ..answer(Vec::new())
    }
}

/// **Both statements count.** A current second answer cannot certify the
/// first, and a store that cannot say for either cannot say for the rail.
#[test]
fn the_rail_is_covered_only_when_both_statements_are() {
    assert_eq!(
        both_covered(&covered(Some(true)), &covered(Some(true))),
        Some(true)
    );
    assert_eq!(
        both_covered(&covered(Some(true)), &covered(Some(false))),
        Some(false)
    );
    assert_eq!(
        both_covered(&covered(Some(false)), &covered(Some(true))),
        Some(false)
    );
    assert_eq!(both_covered(&covered(None), &covered(Some(true))), None);
    assert_eq!(both_covered(&covered(Some(true)), &covered(None)), None);
}

fn a_requirement() -> WriteRequirement {
    WriteRequirement {
        workspace: koru_store_core::ids::WorkspaceId::new(),
        document: koru_store_core::ids::DocumentId::new(),
        peer: 1,
        counter: 5,
    }
}

/// Buckets covered, lists not: the rail keeps asking, and stops when both say
/// yes.
#[test]
fn the_rail_re_asks_until_both_statements_include_the_write() {
    let t0 = std::time::Instant::now();
    let mut panel = TaskCountsPanel::default();
    let workspace = WorkspaceId::new();
    panel.maintain(Some(workspace), true, t0);
    panel.require_for_test(a_requirement(), t0);
    assert_eq!(
        panel.next_refresh_for_test(),
        Some(t0 + crate::tasks_view::query::first_ask_after_write())
    );
    let carried = panel.wanted_for_test().to_vec();

    let t1 = t0 + Duration::from_millis(600);
    panel.accept_carrying_for_test(&covered(Some(true)), &covered(Some(false)), &carried, t1);
    assert_eq!(
        panel.wanted_for_test().len(),
        1,
        "half an answer is no answer"
    );
    assert_eq!(
        panel.next_refresh_for_test(),
        Some(t1 + Duration::from_millis(crate::tasks_view::query::RETRY_MILLIS))
    );

    let t2 = t1 + Duration::from_millis(300);
    panel.accept_carrying_for_test(&covered(Some(true)), &covered(Some(true)), &carried, t2);
    assert!(panel.wanted_for_test().is_empty());
}

#[test]
fn a_workspace_change_clears_the_rails_wait() {
    let t0 = std::time::Instant::now();
    let mut panel = TaskCountsPanel::default();
    panel.maintain(Some(WorkspaceId::new()), true, t0);
    panel.require_for_test(a_requirement(), t0);
    panel.maintain(
        Some(WorkspaceId::new()),
        true,
        t0 + Duration::from_millis(10),
    );
    assert!(panel.wanted_for_test().is_empty());
}

/// **A write moves the number before the store counts it**, and the store's
/// answer takes over when the wait ends.
#[test]
fn an_expected_write_moves_the_numbers_until_the_wait_ends() {
    use crate::tasks_view::echo::CountDelta;
    let t0 = std::time::Instant::now();
    let mut panel = TaskCountsPanel::default();
    panel.maintain(Some(WorkspaceId::new()), true, t0);
    panel.accept_carrying_for_test(
        &answer(vec![group("today", 3, 1)]),
        &answer(Vec::new()),
        &[],
        t0,
    );
    assert_eq!(panel.counts().today, 3);
    assert!(panel.expect(CountDelta {
        today: 1,
        ..CountDelta::default()
    }));
    assert_eq!(panel.counts().today, 4, "one more, drawn now");
    assert!(!panel.expect(CountDelta::default()), "nothing to expect");

    panel.require_for_test(a_requirement(), t0);
    let carried = panel.wanted_for_test().to_vec();
    panel.accept_carrying_for_test(
        &answer(vec![group("today", 3, 1)]),
        &answer(Vec::new()),
        &carried,
        t0 + Duration::from_millis(600),
    );
    // Both statements said uncovered by default here (`covered: None` is
    // unknown, which ends the wait) — the answer is the answer again.
    assert_eq!(panel.pending_for_test(), 0);
    assert_eq!(panel.counts().today, 3);
}
