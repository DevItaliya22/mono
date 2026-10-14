// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn partial_purge_keeps_processing_and_reports_changes_in_every_language() {
    for lang in Lang::ALL {
        let mut visited = Vec::new();
        let report = purge_report([1, 2, 3, 4], lang, |id| {
            visited.push(id);
            match id {
                1 => Err("first refusal"),
                2 => Ok(true),
                3 => Err("later refusal"),
                _ => Ok(false),
            }
        })
        .unwrap();
        assert_eq!(visited, [1, 2, 3, 4]);
        assert_eq!(report.title, t(lang, Key::TrashDeleteFailedTitle));
        assert_eq!(report.body, t(lang, Key::TrashDeletePartial));
        assert_eq!(report.details.as_deref(), Some("first refusal"));
    }
}

#[test]
fn already_absent_items_do_not_turn_a_failed_purge_into_partial_success() {
    let report = purge_report([false, true], Lang::En, |fail| {
        if fail { Err("refused") } else { Ok(false) }
    })
    .unwrap();
    assert_eq!(report.body, t(Lang::En, Key::TrashDeleteFailedBody));
}

#[test]
fn successful_and_empty_purges_have_no_error_report() {
    assert!(purge_report([true, false], Lang::En, Ok::<_, &str>).is_none());
    assert!(purge_report([], Lang::En, Ok::<bool, &str>).is_none());
}
