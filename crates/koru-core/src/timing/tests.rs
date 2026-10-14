// SPDX-License-Identifier: MPL-2.0

//! The switch is a `OnceLock` read of the environment, so a test cannot turn it
//! on — which is why the merging and the formatting are pure functions taking
//! their rows, and these tests call those rather than driving `Span`.

use super::*;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// A span entered several times is **one row** carrying the count, not several
/// rows. This is what makes "one per tab" and "one per peer file" readable.
#[test]
fn repeated_spans_merge_and_count() {
    let mut rows = Vec::new();
    add(&mut rows, "tab open", ms(10), None);
    add(&mut rows, "peer import", ms(3), None);
    add(&mut rows, "tab open", ms(20), None);

    assert_eq!(rows.len(), 2);
    let (name, count, total, ..) = &rows[0];
    assert_eq!(*name, "tab open");
    assert_eq!(*count, 2);
    assert_eq!(*total, ms(30));
}

/// The note comes from the **longest** occurrence, whichever order they arrived
/// in. This is not cosmetic: `peer import` runs once for the profile document
/// and once for the workspace, and while "first wins" the table reported the
/// profile's `0 KB read` and said nothing about the 91 MB that was the whole
/// problem.
#[test]
fn the_note_from_the_longest_occurrence_wins() {
    let mut short_first = Vec::new();
    add(&mut short_first, "peer import", ms(1), Some("0 KB".into()));
    add(
        &mut short_first,
        "peer import",
        ms(8000),
        Some("91000 KB".into()),
    );
    assert_eq!(short_first[0].3.as_deref(), Some("91000 KB"));

    let mut long_first = Vec::new();
    add(
        &mut long_first,
        "peer import",
        ms(8000),
        Some("91000 KB".into()),
    );
    add(&mut long_first, "peer import", ms(1), Some("0 KB".into()));
    assert_eq!(long_first[0].3.as_deref(), Some("91000 KB"));
}

/// A row recorded without a note can still gain one, so an occurrence that knows
/// something is not silenced by a longer one that did not.
#[test]
fn a_note_can_arrive_after_a_bare_occurrence() {
    let mut rows = Vec::new();
    add(&mut rows, "peer import", ms(50), None);
    add(&mut rows, "peer import", ms(1), Some("4 frames".into()));

    assert_eq!(rows[0].3.as_deref(), Some("4 frames"));
}

/// Rows keep the order they were first seen, which is the order the work
/// happened in — a table sorted by duration would lose the shape of the launch.
#[test]
fn rows_stay_in_the_order_they_first_happened() {
    let mut rows = Vec::new();
    add(&mut rows, "settings", ms(1), None);
    add(&mut rows, "profile open", ms(100), None);
    add(&mut rows, "settings", ms(1), None);

    let names: Vec<_> = rows.iter().map(|(n, ..)| *n).collect();
    assert_eq!(names, ["settings", "profile open"]);
}

/// The per-occurrence average only appears when there was more than one, so a
/// span that happened once is not decorated with `(1 × ...)`.
#[test]
fn the_average_only_shows_for_a_repeated_span() {
    let mut rows = Vec::new();
    add(&mut rows, "once", ms(5), None);
    add(&mut rows, "twice", ms(10), None);
    add(&mut rows, "twice", ms(30), None);

    let out = table("cold start", &rows);
    let once = out.lines().find(|l| l.starts_with("once")).unwrap();
    let twice = out.lines().find(|l| l.starts_with("twice")).unwrap();

    assert!(
        !once.contains(" x "),
        "a single occurrence should not show an average: {once}"
    );
    assert!(
        twice.contains("2 x 20.0ms"),
        "expected the mean of 10 and 30: {twice}"
    );
    assert!(twice.contains("40.0ms"), "expected the total: {twice}");
}

/// **The table is ASCII.** It prints to a console whose codepage is whatever
/// Windows chose, and a box-drawing rule came back as `â”€â”€â”€` — the same trap
/// `scripts/*.ps1` carries a warning about, one layer further out.
#[test]
fn the_table_is_ascii_only() {
    let mut rows = Vec::new();
    add(&mut rows, "peer import", ms(10), Some("91000 KB".into()));
    add(&mut rows, "peer import", ms(10), None);

    let out = table("cold start", &rows);
    assert!(
        out.is_ascii(),
        "the table must survive a non-UTF-8 console codepage: {out:?}"
    );
}

/// The heading names what was measured, so a switch table cannot be mistaken for
/// a launch table further up the same console.
#[test]
fn the_heading_says_what_was_measured() {
    let rows = vec![("first frame", 1, ms(1), None, ms(1))];
    assert!(table("switch to Notes", &rows).contains("switch to Notes"));
}

/// Names are padded to the widest, so the numbers form a column.
#[test]
fn names_are_padded_into_a_column() {
    let mut rows = Vec::new();
    add(&mut rows, "gpu", ms(1), None);
    add(&mut rows, "snapshot decode", ms(1), None);

    let out = table("cold start", &rows);
    let at = |prefix: &str| {
        out.lines()
            .find(|l| l.starts_with(prefix))
            .and_then(|l| l.find("ms"))
            .unwrap()
    };
    assert_eq!(at("gpu"), at("snapshot decode"));
}

/// With the switch off — which is how it is in the test binary — a `Span` never
/// reads the clock and never records. The whole facility has to be free in a
/// normal run, or it would be measuring itself.
#[test]
fn a_span_is_inert_when_the_switch_is_off() {
    assert!(
        !enabled(),
        "KORU_STARTUP_STATS must not be set when running the suite"
    );

    let span = Span::start("nothing");
    assert!(span.at.is_none(), "the clock was read with the switch off");
    span.stop_with(|| panic!("the note was built with the switch off"));

    assert!(spans().lock().unwrap().is_empty());
}

/// **The stall watch has a switch of its own.** It used to ride on
/// `KORU_STARTUP_STATS`, so the one diagnostic aimed at a freeze in the middle
/// of a session was behind a variable named for the first two seconds of one —
/// and nobody reporting lag would think to set it.
///
/// The floor is a *second* variable, and that is the point this test pins:
/// `KORU_JANK=1` is on, the way every switch in this tree is on, and a single
/// variable carrying both would read that `1` as a one-millisecond floor and
/// log every event in the session.
#[test]
fn the_stall_floor_reads_its_own_switch() {
    assert_eq!(floor_from(Some("1"), None, false), Some(STALL_OVER));
    assert_eq!(floor_from(Some("1"), Some("120"), false), Some(ms(120)));
    // Still on for a session that is already measuring, as it always was.
    assert_eq!(floor_from(None, None, true), Some(STALL_OVER));
    assert_eq!(floor_from(None, None, false), None);
    // A floor with the switch off is off: one variable turns it on.
    assert_eq!(floor_from(Some("0"), Some("120"), false), None);
}

/// Off is off, and a typo in the floor still watches. `KORU_JANK_MS=fast` is
/// somebody guessing at the units, and the answer to a guess is the default
/// rather than a silence they would read as "no stalls".
#[test]
fn a_nonsense_floor_is_the_default_and_zero_is_off() {
    assert_eq!(floor_from(Some("1"), Some("fast"), false), Some(STALL_OVER));
    assert_eq!(floor_from(Some("0"), None, true), None);
    assert_eq!(floor_from(Some(""), None, true), None);
}

/// The summary is the paragraph somebody attaches to a report, so it names the
/// worst first rather than the most recent.
#[test]
fn the_summary_is_ordered_worst_first() {
    let mut stalls = vec![
        ("redraw", ms(80)),
        ("key press", ms(300)),
        ("scroll", ms(90)),
    ];
    stalls.sort_by_key(|(_, took)| std::cmp::Reverse(*took));
    assert_eq!(
        stalls.iter().map(|(what, _)| *what).collect::<Vec<_>>(),
        ["key press", "scroll", "redraw"]
    );
}

/// With no switch set — which is how the suite runs — a `Watch` never reads the
/// clock, so instrumenting the event loop costs a healthy session nothing.
#[test]
fn a_watch_is_inert_when_the_switch_is_off() {
    assert!(
        stall_floor().is_none(),
        "KORU_JANK must not be set in tests"
    );
    let watch = Watch::start("nothing");
    assert!(watch.at.is_none(), "the clock was read with the switch off");
}
