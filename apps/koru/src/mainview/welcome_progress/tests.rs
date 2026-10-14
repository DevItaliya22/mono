// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn without_a_client_ticks_stay_in_memory_and_nothing_is_asked() {
    let mut progress = WelcomeProgress::default();
    assert!(progress.mark(vec!["learn"]));
    assert!(
        !progress.mark(vec!["learn"]),
        "a second tick changes nothing"
    );
    assert_eq!(progress.done().collect::<Vec<_>>(), ["learn"]);
    assert_eq!(progress.deadline(), None, "no client, no clock");
    assert!(!progress.maintain(Instant::now()));
    assert_eq!(
        progress.unwritten,
        ["learn"],
        "written once a client arrives"
    );
}
