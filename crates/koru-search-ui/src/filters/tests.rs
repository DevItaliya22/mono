// SPDX-License-Identifier: MPL-2.0

use super::*;

fn wire(
    key: &str,
    label: &str,
    value_type: &str,
    options: &[&str],
    filterable: bool,
) -> AttributeDefinition {
    AttributeDefinition {
        id: key.to_owned(),
        key: key.to_owned(),
        label: label.to_owned(),
        value_type: value_type.to_owned(),
        cardinality: "one".to_owned(),
        scope: "shared".to_owned(),
        system: key.starts_with("koru."),
        options: options.iter().map(|o| (*o).to_owned()).collect(),
        filterable,
        sortable: true,
        groupable: true,
    }
}

/// The system registry as the store answers it, plus one attribute a workspace
/// invented. **The custom one is the point of every test below it.**
fn scope() -> Vec<FilterDefinition> {
    let mut personal = wire("koru.plan", "Plan", "datetime_floating", &[], true);
    personal.scope = "personal".to_owned();
    digest(&[
        wire("koru.tag", "Tags", "multi_select", &[], true),
        wire("koru.color", "Color", "select", &["red", "teal"], true),
        wire("koru.due", "Due", "datetime_zoned", &[], true),
        wire(
            "koru.priority",
            "Priority",
            "select",
            &["low", "medium", "high", "urgent"],
            true,
        ),
        personal,
        wire("user.effort", "Effort", "number", &[], true),
        wire("user.notes", "Notes", "text", &[], true),
        wire("user.secret", "Secret", "text", &[], false),
    ])
}

fn keys(defs: &[FilterDefinition]) -> Vec<&str> {
    defs.iter().map(|d| d.key.as_str()).collect()
}

/// **The chooser is built from the definitions, so a workspace's own attribute
/// appears with no new code.** The refusals are structural too: a definition
/// that says it is not filterable, a personal one the index never sees, and a
/// select with nothing to select — which is what keeps `koru.tag` out without
/// naming it, since `#` is already its grammar.
#[test]
fn the_chooser_lists_custom_definitions_and_refuses_the_unanswerable() {
    assert_eq!(
        keys(&scope()),
        [
            "koru.color",
            "koru.due",
            "koru.priority",
            "user.effort",
            "user.notes"
        ]
    );
}

/// **`kinds` is the difference between a filter that works and one that
/// silently answers nothing.** The line worth pinning is the timed one: a due
/// stored as a plain calendar date is invisible to a window naming only
/// `datetime_zoned`.
#[test]
fn each_value_type_names_the_kinds_its_values_are_stored_under() {
    assert_eq!(kinds_for("text"), ["text"]);
    assert_eq!(kinds_for("select"), ["text"]);
    assert_eq!(kinds_for("multi_select"), ["text"]);
    assert_eq!(kinds_for("number"), ["number"]);
    assert_eq!(kinds_for("boolean"), ["bool"]);
    assert_eq!(kinds_for("date"), ["date"]);
    assert_eq!(kinds_for("datetime_zoned"), ["date", "datetime_zoned"]);
    assert_eq!(
        kinds_for("datetime_floating"),
        ["date", "datetime_floating"]
    );
    assert!(
        kinds_for("ref").is_empty(),
        "an unknown type is not guessed"
    );

    let defs = scope();
    let due = &defs[resolve(&defs, "due").expect("due is in scope")];
    assert_eq!(due.kinds, ["date", "datetime_zoned"]);
    let effort = &defs[resolve(&defs, "effort").expect("the custom one is in scope")];
    assert_eq!(effort.kinds, ["number"]);
}

/// **`/task` asks whether something *is* a task, and offers nothing to choose.**
///
/// `koru.task` is a `select` whose tokens are `explicit`, `due` and
/// `tasks_view` — how a node came to be a task, recorded so a deadline can be
/// told from a typed `/task` afterwards. Offered as a value list they were
/// three pieces of internal bookkeeping in somebody's search box, and the one
/// useful question was the one the key could not ask.
///
/// It is a complete answer rather than a near one: every door that makes a task
/// writes this marker, `set_due` included, so *carries the marker* and *is a
/// task* are the same set.
#[test]
fn the_task_marker_is_a_presence_filter_and_offers_no_tokens() {
    let defs = digest(&[wire(
        "koru.task",
        "Task",
        "select",
        &["explicit", "due", "tasks_view"],
        true,
    )]);
    let def = &defs[0];
    assert_eq!(def.editor, FilterEditor::Presence);
    assert!(
        def.options.is_empty(),
        "the reasons are not offered: {:?}",
        def.options
    );

    // And what it puts on the wire is presence, with no value.
    let (text, filters) = split("ship it /task", &defs);
    assert_eq!(text, "ship it");
    assert_eq!(filters.len(), 1);
    assert_eq!(filters[0].key, "koru.task");
    assert_eq!(filters[0].op, AttrOp::Exists);
    assert!(filters[0].values.is_empty());
}

/// The **lifecycle** keeps its list, and that is the line: `open` and
/// `completed` are words a person means, where `tasks_view` is not.
#[test]
fn the_task_state_keeps_its_tokens() {
    let defs = digest(&[wire(
        "koru.task_state",
        "Task state",
        "select",
        &["open", "completed"],
        true,
    )]);
    assert_eq!(defs[0].editor, FilterEditor::Options);
    assert_eq!(defs[0].options, ["open", "completed"]);
}

/// A boolean has no declared options and still needs a list; it gets the only
/// two values it can hold.
#[test]
fn a_boolean_offers_its_two_tokens() {
    let defs = digest(&[wire("user.done", "Done", "boolean", &[], true)]);
    assert_eq!(defs[0].options, ["false", "true"]);
    assert_eq!(defs[0].editor, FilterEditor::Options);
    assert_eq!(defs[0].kinds, ["bool"]);
}

/// The editor follows the **type**, which is the whole reason nothing here is
/// per-attribute code.
#[test]
fn the_editor_follows_the_type() {
    let defs = scope();
    let editor = |typed: &str| defs[resolve(&defs, typed).expect(typed)].editor;
    assert_eq!(editor("priority"), FilterEditor::Options);
    assert_eq!(editor("color"), FilterEditor::Options);
    assert_eq!(editor("effort"), FilterEditor::Field);
    assert_eq!(editor("notes"), FilterEditor::Field);
    assert_eq!(editor("due"), FilterEditor::Temporal);
}

/// A key is named by its last segment or by its label, and an ambiguous
/// spelling names nothing rather than one of the two arbitrarily.
#[test]
fn a_key_resolves_by_segment_label_or_not_at_all() {
    let defs = scope();
    assert_eq!(
        defs[resolve(&defs, "priority").unwrap()].key,
        "koru.priority"
    );
    assert_eq!(
        defs[resolve(&defs, "Priority").unwrap()].key,
        "koru.priority"
    );
    assert_eq!(
        defs[resolve(&defs, "koru.priority").unwrap()].key,
        "koru.priority"
    );
    assert_eq!(defs[resolve(&defs, "effort").unwrap()].key, "user.effort");
    assert_eq!(resolve(&defs, "tmp"), None);
    assert_eq!(resolve(&defs, ""), None);

    let clash = digest(&[
        wire("a.effort", "Effort A", "number", &[], true),
        wire("b.effort", "Effort B", "number", &[], true),
    ]);
    assert_eq!(
        resolve(&clash, "effort"),
        None,
        "two definitions cannot both be /effort, and picking one is worse"
    );
}

/// **A committed filter leaves the text.** This is the whole contract with the
/// store: `attributes` carries it and `text` no longer does.
#[test]
fn a_committed_filter_is_stripped_from_the_text() {
    let defs = scope();
    let (text, filters) = split("Hello /priority high #fun", &defs);
    assert_eq!(text, "Hello #fun", "the tag still travels inside the text");
    assert_eq!(filters.len(), 1);
    assert_eq!(filters[0].key, "koru.priority");
    assert_eq!(filters[0].op, AttrOp::Is);
    assert_eq!(filters[0].values, ["high"]);
    assert_eq!(filters[0].kinds, ["text"]);
}

/// **A `/` that does not begin a word is text**, and so is one whose key names
/// nothing in scope. Both rules exist so an ordinary query is not quietly
/// eaten.
#[test]
fn only_a_word_leading_slash_naming_a_definition_is_a_filter() {
    let defs = scope();
    for query in ["due 10/28", "and/or", "/tmp/scratch", "a /nonesuch b"] {
        let (text, filters) = split(query, &defs);
        assert_eq!(text, query, "{query} is text");
        assert!(filters.is_empty(), "{query} is text");
    }
}

/// Two filters compose, and so do a filter and a tag.
#[test]
fn filters_and_text_compose() {
    let defs = scope();
    let (text, filters) = split("/priority urgent quarterly /color teal", &defs);
    assert_eq!(text, "quarterly");
    assert_eq!(filters.len(), 2);
    assert_eq!(filters[0].values, ["urgent"]);
    assert_eq!(filters[1].key, "koru.color");
    assert_eq!(filters[1].values, ["teal"]);
}

/// A number takes its comparison on the front of the value, which is how a
/// person writes one. The wire's ranges are half-open, so `>` is `>=`.
#[test]
fn a_number_reads_its_comparison_from_the_value() {
    let defs = scope();
    let filter = |query: &str| split(query, &defs).1.into_iter().next().expect(query);
    assert_eq!(filter("/effort 3").op, AttrOp::Is);
    assert_eq!(filter("/effort >3").op, AttrOp::After);
    assert_eq!(filter("/effort >=3").op, AttrOp::After);
    assert_eq!(filter("/effort <3").op, AttrOp::Before);
    assert_eq!(filter("/effort >3").values, ["3"]);

    // And a select does not: `>` is a character somebody may be searching for.
    assert_eq!(split("/priority >high", &defs).1.len(), 0);
}

/// The explicit spellings, including the two that take no value at all.
#[test]
fn the_wire_operators_can_be_named() {
    let defs = scope();
    let filters = |query: &str| split(query, &defs).1;
    assert_eq!(filters("/due:before 2026-09-10")[0].op, AttrOp::Before);
    assert_eq!(
        filters("/due:before 2026-09-10")[0].kinds,
        ["date", "datetime_zoned"]
    );
    assert_eq!(filters("/due:exists")[0].op, AttrOp::Exists);
    assert!(filters("/due:exists")[0].values.is_empty());
    assert_eq!(filters("/effort:between 1..5")[0].values, ["1", "5"]);
    assert_eq!(
        filters("/priority:one_of high,urgent")[0].values,
        ["high", "urgent"]
    );
    assert!(
        filters("/due:soonish 2026-09-10").is_empty(),
        "a spelling the wire does not have is not guessed at"
    );
}

/// **Half-typed is not "matches nothing".** The span is still removed from the
/// text — it is not a search term — but no filter is sent, so the page does not
/// blank while a hand is moving.
#[test]
fn an_incomplete_filter_sends_nothing_and_leaves_no_text() {
    let defs = scope();
    for query in ["/priority", "/priority ", "/priority h", "/effort", "/due:"] {
        let (text, filters) = split(query, &defs);
        assert!(text.is_empty(), "{query} left {text:?} behind");
        assert!(filters.is_empty(), "{query} sent a filter");
    }
}

/// A phrase with a space in it needs quotes, and they do not reach the wire.
#[test]
fn a_quoted_value_holds_its_spaces() {
    let defs = scope();
    let (text, filters) = split("/notes \"two words\" tail", &defs);
    assert_eq!(text, "tail");
    assert_eq!(filters[0].values, ["two words"]);
}

/// The chooser's context: `/pri` is a key being chosen, `/priority ` is a value
/// being typed, and ordinary text is neither.
#[test]
fn the_caret_says_which_half_is_being_typed() {
    let defs = scope();
    let at = |text: &str| context_at(text, text.len(), &defs);

    assert_eq!(
        at("/pri"),
        Some(Context::Key {
            token: 0..4,
            typed: "pri".to_owned()
        })
    );
    assert_eq!(
        at("/"),
        Some(Context::Key {
            token: 0..1,
            typed: String::new()
        }),
        "one keystroke is enough, the way it is for a tag"
    );
    assert_eq!(
        at("/priority "),
        Some(Context::Value {
            def: resolve(&defs, "priority").unwrap(),
            token: 10..10,
            typed: String::new()
        }),
        "the space moved the caret into the value"
    );
    assert_eq!(
        at("/priority hi"),
        Some(Context::Value {
            def: resolve(&defs, "priority").unwrap(),
            token: 10..12,
            typed: "hi".to_owned()
        })
    );
    assert_eq!(at("10/28"), None);
    assert_eq!(at("/priority high "), None, "past the value it is closed");
    assert_eq!(
        at("/due:"),
        None,
        "the colon invites an operator, not a value: nothing is offered"
    );
}

/// A narrowing key matches its segment or its label, and an empty token offers
/// everything.
#[test]
fn a_typed_key_narrows_by_segment_or_label() {
    let defs = scope();
    let matching = |typed: &str| {
        let folded = koru_tagname::fold_segment(typed);
        defs.iter()
            .filter(|d| d.matches(&folded))
            .map(|d| d.key.as_str())
            .collect::<Vec<_>>()
    };
    assert_eq!(matching("").len(), 5, "a bare slash offers them all");
    assert_eq!(matching("pri"), ["koru.priority"]);
    assert_eq!(matching("Effort"), ["user.effort"]);
    assert_eq!(matching("zzz").len(), 0);
}

// ------------------------------------------------ the two halves of a bound

/// Midnight of an ISO day in this machine's zone, written out here rather than
/// read back from the code under test.
fn local_midnight_ms(iso: &str) -> i64 {
    use chrono::TimeZone;
    let day = chrono::NaiveDate::parse_from_str(iso, "%Y-%m-%d").unwrap();
    chrono::Local
        .from_local_datetime(&day.and_time(chrono::NaiveTime::MIN))
        .single()
        .expect("no DST jump at midnight in September")
        .timestamp_millis()
}

/// **A due bound is written once and compared twice.** `koru.due` names `date`
/// and `datetime_zoned`, and those two do not share a column: the calendar day
/// compares as ISO text and the instant as a number. So one filter carries a
/// bound for each, and the one for the instant is computed here because this is
/// the layer that knows the zone.
///
/// Without it the instant half drops out in the store — an ISO date does not
/// parse as a number — and the window answers only for all-day deadlines while
/// looking exactly as though it answered for both.
#[test]
fn a_due_bound_carries_a_form_for_each_column() {
    let defs = scope();
    let (_, filters) = split("/due:before 2026-09-10", &defs);
    assert_eq!(filters.len(), 1);
    assert_eq!(filters[0].kinds, ["date", "datetime_zoned"]);
    assert_eq!(filters[0].values, ["2026-09-10"]);
    assert_eq!(
        filters[0].numeric_values,
        [local_midnight_ms("2026-09-10").to_string()],
        "midnight local, not the UTC one"
    );
}

/// A `between` carries both edges in both forms, in the same order — the
/// operands are positional, so a half-converted list would pair a lower bound
/// with the wrong upper one.
#[test]
fn a_between_carries_both_edges_in_both_forms() {
    let defs = scope();
    let (_, filters) = split("/due:between 2026-09-10..2026-09-11", &defs);
    assert_eq!(filters[0].values, ["2026-09-10", "2026-09-11"]);
    let ms: Vec<i64> = filters[0]
        .numeric_values
        .iter()
        .map(|value| value.parse().unwrap())
        .collect();
    assert_eq!(ms.len(), 2);
    assert_eq!(ms[1] - ms[0], 24 * 60 * 60 * 1000);
}

/// A bound carrying a clock keeps it in both forms — the ISO datetime still
/// orders correctly against a `date` row, and the instant is that wall clock
/// resolved locally.
#[test]
fn a_timed_bound_keeps_its_clock() {
    use chrono::TimeZone;
    let defs = scope();
    let (_, filters) = split("/due:after 2026-09-10T17:30", &defs);
    assert_eq!(filters[0].values, ["2026-09-10T17:30"]);
    let expected = chrono::Local
        .from_local_datetime(
            &chrono::NaiveDate::from_ymd_opt(2026, 9, 10)
                .unwrap()
                .and_hms_opt(17, 30, 0)
                .unwrap(),
        )
        .single()
        .unwrap()
        .timestamp_millis();
    assert_eq!(filters[0].numeric_values, [expected.to_string()]);
}

/// **A definition whose kinds all compare the same way carries no second
/// copy.** A number is already a number, and a select's token is only ever
/// text; writing the operands twice there would be a list nothing reads.
#[test]
fn a_single_column_definition_needs_no_second_copy() {
    let defs = scope();
    for text in ["/effort >3", "/priority high", "/due:exists"] {
        let (_, filters) = split(text, &defs);
        assert!(
            filters[0].numeric_values.is_empty(),
            "{text} names kinds that share a column"
        );
    }
}

/// **All or nothing.** A bound that will not parse leaves the numeric half
/// empty rather than partly filled, so the instant kind declines to compare
/// instead of comparing against the wrong edge.
#[test]
fn an_unparseable_bound_converts_nothing() {
    let defs = scope();
    let (_, filters) = split("/due:before soon", &defs);
    assert_eq!(filters[0].values, ["soon"]);
    assert!(filters[0].numeric_values.is_empty());
}

/// What the picker needs in order to rewrite a filter: which definition, the
/// **whole** span rather than only its value, and the side already spelled —
/// which is what makes `/due:after` skip the question and plain `/due` ask it.
#[test]
fn a_span_edit_names_the_whole_filter_and_the_side_already_chosen() {
    let defs = scope();
    let due = resolve(&defs, "due").unwrap();

    let text = "quarterly /due ";
    let edit = span_edit_at(text, text.len(), &defs).unwrap();
    assert_eq!(edit.def, due);
    assert_eq!(&text[edit.range.clone()], "/due ");
    assert_eq!(edit.op, None, "plain /due asks which side");

    let text = "/due:after ";
    let edit = span_edit_at(text, text.len(), &defs).unwrap();
    assert_eq!(&text[edit.range], "/due:after ");
    assert_eq!(edit.op, Some(AttrOp::After));

    let text = "/due:before 2026-09-10";
    let edit = span_edit_at(text, text.len(), &defs).unwrap();
    assert_eq!(&text[edit.range], text, "the value is replaced with it");
    assert_eq!(edit.op, Some(AttrOp::Before));

    assert_eq!(
        span_edit_at("/due", 4, &defs),
        None,
        "a key still being chosen is not a span to rewrite"
    );
    assert_eq!(span_edit_at("hello", 5, &defs), None);
}

/// **No task filters in the simplified product** (koru-features): the marker
/// and its lifecycle are not offered, and every other definition still is.
#[test]
fn a_build_without_tasks_offers_no_task_filter() {
    let defs = || {
        digest(&[
            wire("koru.task", "Task", "select", &["explicit", "due"], true),
            wire(
                "koru.task_state",
                "State",
                "select",
                &["open", "completed"],
                true,
            ),
            wire("koru.due", "Due", "datetime_zoned", &[], true),
        ])
    };
    koru_features::testing::with(koru_features::Features::ALL_OFF, || {
        assert_eq!(keys(&defs()), ["koru.due"]);
    });
    assert_eq!(keys(&defs()), ["koru.task", "koru.task_state", "koru.due"]);
}
