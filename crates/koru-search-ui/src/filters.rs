// SPDX-License-Identifier: MPL-2.0

//! The `/key` filter grammar: what a typed attribute filter looks like in the
//! query box, and how it becomes [`AttrFilter`]s on the wire.
//!
//! **The surface parses this and the store does not.** `SearchQuery.attributes`
//! already carries a typed filter ([ADR-194]), so a second *text* grammar in
//! the store would make one filter expressible two ways on the wire. The store
//! still lifts `#tag` out of the query text, and only that; everything here is
//! stripped from [`SearchQuery::text`] before the request leaves.
//!
//! [`SearchQuery::text`]: koru_store_wire::search::SearchQuery::text
//!
//! # The grammar
//!
//! ```text
//! /<key>[:<op>][ <value>]
//! ```
//!
//! - **`key`** names an attribute by its *last* segment — `priority` for
//!   `koru.priority`, `effort` for `user.effort` — or by its label. That is the
//!   same choice `SearchQuery.tags` makes for tags: a person types the word
//!   they read. It also keeps the token inside [`koru_tagname::token_at`]'s
//!   character set, which has no `.` in it.
//! - **`op`** is optional and defaults to `is`. The spellings are the wire's
//!   own: `is`, `one_of`, `before`, `after`, `between`, `exists`, `missing`.
//! - **`value`** is one whitespace-delimited word, or a `"quoted phrase"`.
//!   `between` takes `a..b`; `one_of` takes `a,b`.
//!
//! Two rules keep this out of ordinary text, and both matter more than they
//! look:
//!
//! **A `/` only begins a filter at a word start**, which is
//! [`koru_tagname::token_at`]'s rule — so `10/28` and `and/or` are the text
//! they have always been.
//!
//! **A span is a filter only when its key names a definition in scope.** Type
//! `/tmp` and it stays text, because nothing in the workspace is called `tmp`;
//! type `/priority` and the word after it becomes the value. Without this the
//! box would swallow `/etc` out of a perfectly ordinary query, and would have
//! to guess what `hello / world` meant.
//!
//! # The temporal keys
//!
//! A `date`, a `datetime_zoned` or a `datetime_floating` key opens the
//! calendar rather than a list ([`FilterEditor::Temporal`], drawn by
//! [`crate::due_picker`]). What it writes back is still this grammar —
//! `/due:after 2026-09-10` — because the box is the model, and a filter you
//! can see is one you can edit and re-run ([ADR-197]).
//!
//! **A bound is written once and compared twice.** `koru.due` names `date` and
//! `datetime_zoned`, and those two do not share a column in the index: the
//! calendar day compares against ISO text, the instant against a number. So
//! [`split`] carries an epoch-millisecond copy of every temporal bound in
//! `AttrFilter::numeric_values`, computed **here** because this is the layer
//! that knows which zone the person is asking in. Leaving it out is not a
//! smaller answer — it is the same window silently answering for all-day
//! deadlines only.
//!
//! [ADR-194]: ../../../docs/decisions/ADR-201-225.md#adr-217-typedsearch--search-keeps-every-attribute-in-its-declared-type-and-a-filter-names-the-kinds-it-compares-against
//! [ADR-197]: ../../../docs/decisions/ADR-201-225.md#adr-220-duefilter--due-opens-the-calendar-then-asks-which-side-and-a-bound-that-names-two-kinds-is-written-twice

use std::ops::Range;

use koru_store_wire::attributes::AttributeDefinition;
use koru_store_wire::search::{AttrFilter, AttrOp};

/// What leads a typed filter. `#` leads a tag; this leads everything else.
pub(crate) const SIGIL: char = '/';

/// `koru_core::attributes::KEY_TASK`, as a wire key.
///
/// The marker every task carries, and the one definition this surface treats
/// specially — see [`FilterEditor::Presence`] for why. Not imported: this
/// crate links no `koru-core`, on purpose.
pub(crate) const TASK_MARKER_KEY: &str = "koru.task";

/// The wire keys that are only about tasks — the marker and its lifecycle —
/// which [`digest`] leaves out when the build hides tasks.
const TASK_KEYS: [&str; 2] = [TASK_MARKER_KEY, "koru.task_state"];

/// Which control a chosen key opens, decided by the definition's **type** and
/// never by its key — so a workspace's own attribute gets the right one with
/// no new code.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum FilterEditor {
    /// `select`, `multi_select` and `boolean`: the definition's own tokens.
    Options,
    /// `text` and `number`: the query box itself. See the module note on why
    /// there is no second text field.
    Field,
    /// `date`, `datetime_zoned`, `datetime_floating`: the calendar
    /// ([`crate::due_picker`]), which is `koru_ui::date_grid` — the same
    /// widget and the same phrase grammar the editor's due picker wears, so
    /// `2w`, `+3d` and `sept 10` mean one thing in this app rather than two
    /// ([ADR-196]).
    ///
    /// [ADR-196]: ../../../docs/decisions/ADR-201-225.md#adr-219-dategrid--the-calendar-is-a-widget-in-koru-ui-what-a-day-means-stays-with-the-editor
    Temporal,
    /// **The key is the whole filter**: typing it asks for nodes that carry the
    /// attribute at all, and there is nothing after it to choose.
    ///
    /// The one definition that wears this is `koru.task`, and the reason is
    /// what its values *are*. They are `explicit`, `due` and `tasks_view` —
    /// how a node came to be a task, recorded so the app can tell a deadline
    /// from a typed `/task` afterwards. Offered as a value list they were three
    /// pieces of internal bookkeeping in somebody's search box, and the useful
    /// question — *is this a task* — was the one thing the key could not ask.
    ///
    /// It is a complete answer, not a near one: every door that makes a task
    /// writes this marker, `set_due` included, which is what
    /// [`koru_core::tasks::ensure_task`] means by normalizing a due-derived
    /// node in with [`TaskReason::Due`]. So *carries the marker* and *is a
    /// task* are the same set.
    Presence,
}

/// One attribute the box can filter on, digested once when the scope's
/// definitions arrive.
///
/// **Folded here, not per keystroke** — the same reason `TagCompletion` carries
/// its segments: a popover that reopens on every character must not fold a
/// table each time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FilterDefinition {
    /// The wire key, which is what an [`AttrFilter`] carries: `koru.priority`.
    pub(crate) key: String,
    /// The key's last segment, which is what the box spells: `priority`.
    pub(crate) segment: String,
    pub(crate) label: String,
    /// The stored kinds this definition's values compare against. **Never
    /// empty** — a definition whose type maps to no kind is not offered at
    /// all, because a filter naming no kinds matches nothing by design.
    pub(crate) kinds: Vec<String>,
    /// The legal tokens, for an [`FilterEditor::Options`] definition. Empty
    /// otherwise.
    pub(crate) options: Vec<String>,
    pub(crate) editor: FilterEditor,
    folded_segment: String,
    folded_label: String,
}

/// The stored `kind` tokens a definition's values are written under.
///
/// **This is the difference between a filter that works and one that silently
/// answers nothing**, because `AttrFilter.kinds` is required and an empty list
/// matches nothing ([ADR-194]).
///
/// The line that is not guessable: a **timed** attribute accepts a plain
/// calendar date as well as a timed value — `koru_core::attributes::validate`
/// says so, because "due August 30" and "due August 30, 5 pm" are one
/// attribute. So a filter on `koru.due` has to name `date` *and*
/// `datetime_zoned`, or it misses every all-day deadline in the workspace.
///
/// [ADR-194]: ../../../docs/decisions/ADR-201-225.md#adr-217-typedsearch--search-keeps-every-attribute-in-its-declared-type-and-a-filter-names-the-kinds-it-compares-against
pub(crate) fn kinds_for(value_type: &str) -> Vec<String> {
    let kinds: &[&str] = match value_type {
        // A select's token is stored as text; so is a multi-select's, one row
        // per value.
        "text" | "select" | "multi_select" => &["text"],
        "number" => &["number"],
        "boolean" => &["bool"],
        "date" => &["date"],
        "datetime_zoned" => &["date", "datetime_zoned"],
        "datetime_floating" => &["date", "datetime_floating"],
        // Including `ref` and `url`, which no definition declares today. An
        // unknown type is not guessed at: it is simply not offered.
        _ => &[],
    };
    kinds.iter().map(|k| (*k).to_owned()).collect()
}

impl FilterDefinition {
    /// Digest one wire definition, or refuse it.
    ///
    /// Three refusals, each of which would otherwise put a row in the chooser
    /// that cannot answer anything:
    ///
    /// - **not `filterable`** — the definition says so itself;
    /// - **`personal` scope** — those values live per profile and never appear
    ///   on a node, so the index has never seen one;
    /// - **a select with no options** — there is nothing to choose from. This
    ///   is what keeps `koru.tag` out without naming it: its options *are* the
    ///   tag table, and `#` is already its grammar.
    pub(crate) fn from_wire(def: &AttributeDefinition) -> Option<Self> {
        if !def.filterable || def.scope != "shared" {
            return None;
        }
        let kinds = kinds_for(&def.value_type);
        if kinds.is_empty() {
            return None;
        }
        // **A command-only key is a predicate, not a chooser.** `koru.task` and
        // `koru.task_state` are registered definitions so the read path can
        // filter and group by them (ADR-273), and the editor's own `/attr`
        // chooser already filters this same constant rather than the registry
        // growing a field. The search box is the third surface that has to
        // make the distinction, and it was the one that did not.
        //
        // `koru.task_state` keeps its list: `open` and `completed` are words a
        // person means. `koru.task`'s are not.
        //
        // Spelled as a literal, not imported: this crate deliberately links no
        // `koru-core` (see its `Cargo.toml`, where the dependency list is the
        // point), so a wire key it recognises is spelled the way `koru.tag` is
        // spelled in the suite two files over.
        if def.key == TASK_MARKER_KEY {
            return Some(Self::new(def, kinds, FilterEditor::Presence, Vec::new()));
        }
        let (editor, options) = match def.value_type.as_str() {
            "select" | "multi_select" => {
                if def.options.is_empty() {
                    return None;
                }
                (FilterEditor::Options, def.options.clone())
            }
            "boolean" => (
                FilterEditor::Options,
                vec!["false".to_owned(), "true".to_owned()],
            ),
            "text" | "number" => (FilterEditor::Field, Vec::new()),
            _ => (FilterEditor::Temporal, Vec::new()),
        };
        Some(Self::new(def, kinds, editor, options))
    }

    /// Assemble one, with the editor already decided. Split out because
    /// `from_wire` now has two ways of deciding it and they must agree about
    /// everything else.
    fn new(
        def: &AttributeDefinition,
        kinds: Vec<String>,
        editor: FilterEditor,
        options: Vec<String>,
    ) -> Self {
        let segment = def
            .key
            .rsplit('.')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(&def.key)
            .to_owned();
        Self {
            folded_segment: koru_tagname::fold_segment(&segment),
            folded_label: koru_tagname::fold_segment(&def.label),
            key: def.key.clone(),
            segment,
            label: def.label.clone(),
            kinds,
            options,
            editor,
        }
    }

    /// Whether a half-typed key names this one — the predicate the chooser
    /// narrows with. Matched against the segment *and* the label, because the
    /// row says `Priority` and the box spells `priority`.
    pub(crate) fn matches(&self, folded: &str) -> bool {
        folded.is_empty()
            || self.folded_segment.contains(folded)
            || self.folded_label.contains(folded)
    }
}

/// Digest a scope's definitions, in the order the chooser lists them.
pub(crate) fn digest(definitions: &[AttributeDefinition]) -> Vec<FilterDefinition> {
    let mut out: Vec<FilterDefinition> = Vec::new();
    for def in definitions {
        // **No task filters in a build that hides tasks** (koru-features):
        // *is a task* and its lifecycle are questions about the Tasks view.
        // `/due` still makes tasks, but finding them as tasks is that view's.
        if !koru_features::tasks() && TASK_KEYS.contains(&def.key.as_str()) {
            continue;
        }
        let Some(row) = FilterDefinition::from_wire(def) else {
            continue;
        };
        // One row per key. The response carries the system registry plus each
        // scoped workspace's own, and two workspaces may declare the same key.
        if out.iter().any(|existing| existing.key == row.key) {
            continue;
        }
        out.push(row);
    }
    out
}

/// Which definition a typed key names, as an index.
///
/// Exact key first, then a **unique** segment, then a **unique** label. An
/// ambiguous spelling resolves to nothing and stays ordinary text: two
/// definitions whose last segments fold alike cannot both be `/effort`, and
/// silently picking one of them is the worse answer.
pub(crate) fn resolve(defs: &[FilterDefinition], typed: &str) -> Option<usize> {
    if typed.is_empty() {
        return None;
    }
    if let Some(index) = defs.iter().position(|d| d.key == typed) {
        return Some(index);
    }
    let folded = koru_tagname::fold_segment(typed);
    let only = |matched: fn(&FilterDefinition, &str) -> bool| {
        let mut hits = defs
            .iter()
            .enumerate()
            .filter(|(_, def)| matched(def, &folded))
            .map(|(index, _)| index);
        let first = hits.next();
        first.filter(|_| hits.next().is_none())
    };
    only(|def, folded| def.folded_segment == folded)
        .or_else(|| only(|def, folded| def.folded_label == folded))
}

/// One `/key` span in the query text, already resolved to a definition.
///
/// Only resolved spans exist: an unrecognised `/word` is text, so it is neither
/// stripped nor turned into a filter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Span {
    /// The whole span, sigil through value — what is removed from the text.
    pub(crate) range: Range<usize>,
    /// Index into the definitions this was parsed against.
    pub(crate) def: usize,
    op: Option<Range<usize>>,
    /// The value's byte range, present only when a whitespace run separated it
    /// from the key. That is what makes `/priority` a key still being chosen
    /// and `/priority ` a value being typed.
    value: Option<Range<usize>>,
}

fn run(text: &str, mut allowed: impl FnMut(char) -> bool) -> usize {
    text.char_indices()
        .take_while(|(_, ch)| allowed(*ch))
        .map(|(at, ch)| at + ch.len_utf8())
        .last()
        .unwrap_or(0)
}

/// What a key may be spelled with. Deliberately **not** [`koru_tagname`]'s set,
/// which admits `/`: a second slash inside a key would make `/home/user` parse
/// as one, where the token rule already says it is text.
fn is_key_char(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '-' | '_')
}

fn op_takes_value(op: &str) -> bool {
    !matches!(op, "exists" | "missing")
}

/// The value's text, with a surrounding pair of quotes taken off.
fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .map(|rest| rest.strip_suffix('"').unwrap_or(rest))
        .unwrap_or(value)
}

fn value_end(text: &str, start: usize) -> usize {
    let rest = &text[start..];
    if let Some(body) = rest.strip_prefix('"') {
        return match body.find('"') {
            Some(at) => start + 1 + at + 1,
            None => text.len(),
        };
    }
    start + run(rest, |ch| !ch.is_whitespace())
}

fn span_at(text: &str, start: usize, defs: &[FilterDefinition]) -> Option<Span> {
    let key_start = start + SIGIL.len_utf8();
    let key_end = key_start + run(&text[key_start..], is_key_char);
    let def = resolve(defs, &text[key_start..key_end])?;
    let mut end = key_end;
    let mut op = None;
    if text[end..].starts_with(':') {
        let op_start = end + 1;
        let op_end = op_start
            + run(&text[op_start..], |ch| {
                ch.is_ascii_alphabetic() || ch == '_'
            });
        op = Some(op_start..op_end);
        end = op_end;
    }
    let mut value = None;
    let takes = op
        .clone()
        .is_none_or(|range| op_takes_value(&text[range.clone()]));
    if takes {
        let spaces = run(&text[end..], |ch| ch == ' ' || ch == '\t');
        if spaces > 0 {
            let value_start = end + spaces;
            let value_stop = value_end(text, value_start);
            value = Some(value_start..value_stop);
            end = value_stop;
        }
    }
    Some(Span {
        range: start..end,
        def,
        op,
        value,
    })
}

/// Every `/key` span in `text`, left to right.
pub(crate) fn spans(text: &str, defs: &[FilterDefinition]) -> Vec<Span> {
    let mut out = Vec::new();
    let mut at = 0usize;
    let mut after_whitespace = true;
    while at < text.len() {
        let ch = text[at..].chars().next().expect("at is a char boundary");
        if after_whitespace
            && ch == SIGIL
            && let Some(span) = span_at(text, at, defs)
        {
            at = span.range.end;
            out.push(span);
            after_whitespace = false;
            continue;
        }
        after_whitespace = ch.is_whitespace();
        at += ch.len_utf8();
    }
    out
}

/// Turn one span into a wire filter, or answer `None` when it is incomplete.
///
/// **Incomplete is not "matches nothing".** A half-typed `/priority h` is a
/// person mid-gesture, and answering it with a filter that cannot match would
/// blank the page under their hands. The span is still removed from the text —
/// it is not a search term either.
fn filter_of(text: &str, span: &Span, defs: &[FilterDefinition]) -> Option<AttrFilter> {
    let def = defs.get(span.def)?;
    let op_token = span
        .op
        .clone()
        .map(|range| &text[range])
        .unwrap_or_default();
    let raw = span
        .value
        .clone()
        .map(|range| unquote(&text[range]))
        .unwrap_or_default();
    let (op, raw) = match op_token {
        "" => implicit(def, raw),
        "is" => (AttrOp::Is, raw),
        "one_of" => (AttrOp::OneOf, raw),
        "before" => (AttrOp::Before, raw),
        "after" => (AttrOp::After, raw),
        "between" => (AttrOp::Between, raw),
        "exists" => (AttrOp::Exists, raw),
        "missing" => (AttrOp::Missing, raw),
        // A spelling the wire does not have. Not a filter, and not a guess.
        _ => return None,
    };
    let values: Vec<String> = match op {
        AttrOp::Exists | AttrOp::Missing => Vec::new(),
        AttrOp::Between => {
            let (after, before) = raw.split_once("..")?;
            if after.is_empty() || before.is_empty() {
                return None;
            }
            vec![after.to_owned(), before.to_owned()]
        }
        AttrOp::OneOf => {
            let values: Vec<String> = raw
                .split(',')
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect();
            if values.is_empty() {
                return None;
            }
            values
        }
        _ => {
            if raw.is_empty() {
                return None;
            }
            vec![raw.to_owned()]
        }
    };
    // **A select applies only on one of its own tokens.** Half of `high` is
    // `h`, which no node carries — filtering on it would empty the page while
    // somebody is still typing toward a value that exists.
    if !def.options.is_empty() && values.iter().any(|value| !def.options.contains(value)) {
        return None;
    }
    Some(AttrFilter {
        key: def.key.clone(),
        numeric_values: instant_bounds(def, op, &values),
        op,
        values,
        kinds: def.kinds.clone(),
    })
}

/// Whether a stored kind is compared through the index's **number** column.
///
/// The store's own split, spelled here because a filter has to know which of
/// its named kinds will not read the text bound it is carrying.
fn is_numeric_kind(kind: &str) -> bool {
    matches!(kind, "number" | "datetime_zoned")
}

/// The operands again in epoch milliseconds, for a definition that names a
/// numeric kind beside a textual one.
///
/// **All or nothing.** A bound that will not parse leaves the list empty, so
/// the numeric half falls back to `values` and simply declines to compare —
/// which is what a malformed operand should do. A *partly* converted list
/// would pair a lower bound with the wrong upper one, which is worse than not
/// answering.
///
/// Empty for a definition whose kinds all compare the same way: `user.effort`
/// over `["number"]` needs no second copy, and `/priority high` has no bound at
/// all.
fn instant_bounds(def: &FilterDefinition, op: AttrOp, values: &[String]) -> Vec<String> {
    let mixed = def.kinds.iter().any(|kind| is_numeric_kind(kind))
        && def.kinds.iter().any(|kind| !is_numeric_kind(kind));
    if !mixed || values.is_empty() || matches!(op, AttrOp::Exists | AttrOp::Missing) {
        return Vec::new();
    }
    let converted: Option<Vec<String>> = values.iter().map(|value| instant_bound(value)).collect();
    converted.unwrap_or_default()
}

/// One ISO bound as epoch milliseconds **in this machine's zone**.
///
/// `2026-09-10` is midnight local; `2026-09-10T17:30` is that wall clock local.
/// A day that does not exist locally — the hour a spring-forward skips —
/// resolves to the first instant that does, because refusing a bound would
/// drop the whole window rather than move its edge by an hour.
fn instant_bound(raw: &str) -> Option<String> {
    use chrono::{NaiveDate, NaiveDateTime, TimeZone};
    let raw = raw.trim();
    let wall = ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"]
        .iter()
        .find_map(|format| NaiveDateTime::parse_from_str(raw, format).ok())
        .or_else(|| {
            NaiveDate::parse_from_str(raw, "%Y-%m-%d")
                .ok()
                .map(|day| day.and_time(chrono::NaiveTime::MIN))
        })?;
    let instant = match chrono::Local.from_local_datetime(&wall) {
        chrono::LocalResult::Single(at) => at,
        chrono::LocalResult::Ambiguous(first, _) => first,
        // Skipped by a DST jump: take the first instant on or after it.
        chrono::LocalResult::None => chrono::Local
            .from_local_datetime(&(wall + chrono::Duration::hours(1)))
            .earliest()?,
    };
    Some(instant.timestamp_millis().to_string())
}

/// The operator a value carries on its own front, for a definition that
/// compares as a number.
///
/// `>3` and `>=3` are [`AttrOp::After`], `<3` is [`AttrOp::Before`]. The wire's
/// ranges are half-open, so `After` is `>=` — a strict `>` is not expressible
/// and is not pretended to be.
fn implicit<'a>(def: &FilterDefinition, raw: &'a str) -> (AttrOp, &'a str) {
    // **A presence key means presence, with no value and no operator.** It is
    // the whole point of the editor: `/task` on its own is *is this a task*,
    // and the default `is` would have asked for a value nobody can type.
    if def.editor == FilterEditor::Presence {
        return (AttrOp::Exists, "");
    }
    if def.kinds.iter().any(|kind| kind == "number") {
        for (sigil, op) in [
            (">=", AttrOp::After),
            (">", AttrOp::After),
            ("<", AttrOp::Before),
        ] {
            if let Some(rest) = raw.strip_prefix(sigil) {
                return (op, rest.trim_start());
            }
        }
    }
    (AttrOp::Is, raw)
}

/// Split a query box's text into what the store searches for and what it
/// filters by.
///
/// The returned text has every recognised span removed and its whitespace
/// collapsed, so `Hello /priority high #fun` reaches the store as
/// `Hello #fun` — where the store's own parser still lifts the tag.
pub(crate) fn split(text: &str, defs: &[FilterDefinition]) -> (String, Vec<AttrFilter>) {
    let spans = spans(text, defs);
    if spans.is_empty() {
        return (text.to_owned(), Vec::new());
    }
    let mut kept = String::with_capacity(text.len());
    let mut filters = Vec::new();
    let mut last = 0;
    for span in &spans {
        kept.push_str(&text[last..span.range.start]);
        last = span.range.end;
        if let Some(filter) = filter_of(text, span, defs) {
            filters.push(filter);
        }
    }
    kept.push_str(&text[last..]);
    let kept = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    (kept, filters)
}

/// Where the caret is, as far as the chooser is concerned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Context {
    /// Choosing which attribute to filter on.
    Key {
        /// The `/key` token, sigil included — what a chosen row replaces.
        token: Range<usize>,
        typed: String,
    },
    /// A key is chosen and the caret is in its value.
    Value {
        def: usize,
        /// The value's range, which may be empty when the value has not been
        /// started. Replacing it writes the chosen token.
        token: Range<usize>,
        typed: String,
    },
}

/// What the caret is in the middle of, or `None` when it is in ordinary text.
///
/// The key half is [`koru_tagname::token_at`]'s answer, so the box cannot
/// decide a `/` starts a word when the token rule says it does not.
pub(crate) fn context_at(text: &str, caret: usize, defs: &[FilterDefinition]) -> Option<Context> {
    if let Some(token) = koru_tagname::token_at(text, caret, SIGIL) {
        let typed = text[token.start + SIGIL.len_utf8()..token.end].to_owned();
        return Some(Context::Key { token, typed });
    }
    spans(text, defs).into_iter().find_map(|span| {
        let value = span.value?;
        (caret >= value.start && caret <= value.end).then(|| Context::Value {
            def: span.def,
            typed: unquote(&text[value.clone()]).to_owned(),
            token: value,
        })
    })
}

/// What a picker needs in order to rewrite the filter it was opened on.
///
/// [`Context::Value`] carries only the *value's* range, which is enough for a
/// list of tokens replacing one word. A temporal picker writes the operator as
/// well — `/due:after 2026-09-10` — so it replaces the **whole span**, and it
/// has to know which side was already named before it decides whether to ask.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SpanEdit {
    /// Index into the definitions this was parsed against.
    pub(crate) def: usize,
    /// The whole span, sigil through value — what the commit replaces.
    pub(crate) range: Range<usize>,
    /// The operator already spelled after the colon, when it is one this
    /// grammar knows. `None` is what makes plain `/due` ask which side.
    pub(crate) op: Option<AttrOp>,
}

/// The [`SpanEdit`] for the span whose *value* contains `caret` — the same
/// rule [`context_at`]'s value arm applies, so a picker is open exactly when
/// the chooser would have been.
pub(crate) fn span_edit_at(
    text: &str,
    caret: usize,
    defs: &[FilterDefinition],
) -> Option<SpanEdit> {
    spans(text, defs).into_iter().find_map(|span| {
        let value = span.value.clone()?;
        if caret < value.start || caret > value.end {
            return None;
        }
        let op = span
            .op
            .clone()
            .map(|range| &text[range])
            .and_then(|token| match token {
                "before" => Some(AttrOp::Before),
                "after" => Some(AttrOp::After),
                "between" => Some(AttrOp::Between),
                _ => None,
            });
        Some(SpanEdit {
            def: span.def,
            range: span.range,
            op,
        })
    })
}

#[cfg(test)]
mod tests;
