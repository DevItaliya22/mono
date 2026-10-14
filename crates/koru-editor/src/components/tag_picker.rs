// SPDX-License-Identifier: MPL-2.0

//! The tag chooser: filter the workspace's tags as you type, apply one to the
//! target block, or create one that does not exist yet.
//!
//! **In the chooser a ticked row toggles off**; in the `#tag` grammar choosing
//! only ever applies ([ADR-211](../../../../docs/decisions/ADR-201-225.md#adr-211-chipclick--an-attribute-chip-is-the-control-for-its-attribute-and-a-tag-comes-off-at-the-chip),
//! amended twice). The difference is whether the state a second choice reverses
//! is on screen: the chooser draws a checkmark beside every tag the block
//! carries, and a ticked row nothing can untick is a checkbox list with one
//! direction. The `#` form draws no checkmarks — its rows complete what is
//! being typed — and `#home` typed twice is one `#home`, not none.
//!
//! The chip's `×` is unchanged and is still the only removal that reaches a
//! block the chooser is not open on.
//!
//! **And in its field form it stays open across several choices** (ADR-188,
//! amended 2026-08-27). Tags are the one attribute a block carries many of, so
//! a visit here is usually more than one tag; choosing applies and leaves the
//! panel up, [`reopen_after_apply`](TagPicker::reopen_after_apply) ticks the
//! row, and the panel is dismissed the ordinary way — Escape, or a click
//! outside it. The other three attribute pickers set a single value and still
//! commit-and-close, which is why this is the tag picker's method and not
//! [`Outcome`]'s: a *stay open* outcome would have had to be answered by every
//! panel.
//!
//! Opened by `/tag` (and the menus) with its own search field, and by the
//! `#tag` grammar with none — there the query is the text after `#` in the
//! document, followed through [`super::Overlay::refresh`] exactly the way the
//! `:` emoji shortcode is.
//!
//! It lists **the tag table**, never strings found on nodes: a tag is an
//! entity (D5), the table maps its id to a name, and what committing writes
//! onto the node is the [`TagId`]. Creating goes the other way — the table
//! lives in the workspace system document, which no view can reach, so the
//! create row asks the host ([`Outcome::CreateTag`] →
//! `ViewState::wants_create_tag`) and the host applies the fresh id.

use std::collections::HashSet;

use winit::keyboard::{Key, ModifiersState, NamedKey};

use super::Outcome;
use super::command_menu::{MAX_ROWS, rows_top, visible_rows};
use super::text_field::TextField;
use crate::trigger::Trigger;
use koru_core::NodeId;
use koru_core::ids::TagId;
use koru_core::tags;
use koru_draw::text::TextEngine;
use koru_draw::theme;

/// One row's worth of the tag table, resolved for display.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TagChoice {
    pub id: TagId,
    /// The **whole path** — `fun/home`. A row draws it in two weights (see
    /// [`label`](Self::label)); a chip on a block draws only the leaf.
    pub name: String,
    /// The tag's accent token, for the row's swatch and the chip it predicts.
    pub color: Option<String>,
}

impl TagChoice {
    /// The row's label, split so the branch can be drawn quietly and the tag
    /// itself read at a glance: `("fun/", "home")` for `#fun/home`, and
    /// `("", "mvp")` for a root tag.
    ///
    /// A picker is the one surface that has to show *where* a tag is filed —
    /// two `home`s under different parents are two tags, and a leaf-only row
    /// would make them look like one. The chip on a block is the opposite case
    /// and shows the leaf alone.
    pub fn label(&self) -> (String, &str) {
        let leaf = tags::leaf(&self.name);
        match tags::parent(&self.name) {
            Some(path) => (format!("{path}/"), leaf),
            None => (String::new(), leaf),
        }
    }
}

pub struct TagPicker {
    pub trigger: Trigger,
    /// The nodes a choice applies to — one for `/tag` and `#tag`, several for
    /// a multi-select apply.
    pub nodes: Vec<NodeId>,
    /// The table snapshot this picker filters, in listing order (name, id).
    pub table: Vec<TagChoice>,
    /// Tags the (first) target node already carries: drawn checked, so the
    /// list says what is already on the block.
    ///
    /// What the checkmark is made of — and, in the chooser, what decides
    /// whether choosing a row applies or removes ([`choose`](Self::choose)).
    ///
    /// It is the **first** target node's tags, which is why a multi-node apply
    /// never toggles: the tick would be one block's answer spent on all of
    /// them ([ADR-188], amended twice).
    ///
    /// [ADR-188]: ../../../../docs/decisions/ADR-201-225.md#adr-211-chipclick--an-attribute-chip-is-the-control-for-its-attribute-and-a-tag-comes-off-at-the-chip
    pub applied: HashSet<TagId>,
    /// The panel's own search box — `Some` for `/tag` and the menus. `None`
    /// for `#tag`, whose query is document text and arrives via `refilter`.
    pub query: Option<TextField>,
    /// The document-driven query, in `#tag` mode.
    pub filter: String,
    /// Indices into [`table`](Self::table).
    pub hits: Vec<usize>,
    pub sel: usize,
    /// First visible row, when the list is longer than the window.
    pub first: usize,
}

impl TagPicker {
    /// The `/tag` form: a search field, everything listed.
    pub fn opened(
        engine: &mut TextEngine,
        trigger: Trigger,
        nodes: Vec<NodeId>,
        table: Vec<TagChoice>,
        applied: HashSet<TagId>,
    ) -> Self {
        let hits = (0..table.len()).collect();
        Self {
            trigger,
            nodes,
            table,
            applied,
            query: Some(TextField::new(engine, theme::role::ui())),
            filter: String::new(),
            hits,
            sel: 0,
            first: 0,
        }
    }

    /// The `#tag` form. The shortcode contract: **no match never opens** — a
    /// `#` in prose must not sprout a panel unless the query already looks
    /// like a tag, which here means it matches the table or is at least two
    /// characters of an intended new one.
    pub fn for_hash(
        trigger: Trigger,
        nodes: Vec<NodeId>,
        table: Vec<TagChoice>,
        applied: HashSet<TagId>,
        q: &str,
    ) -> Option<Self> {
        let hits = filter_table(&table, q);
        if hits.is_empty() && q.chars().count() < 2 {
            return None;
        }
        Some(Self {
            trigger,
            nodes,
            table,
            applied,
            query: None,
            filter: q.to_owned(),
            hits,
            sel: 0,
            first: 0,
        })
    }

    /// Re-filter as the `#` query behind the panel changes. Always answers a
    /// panel — with no table match the create row is what is on offer — and
    /// dismissal on a dead trigger is `refresh`'s, upstream.
    pub fn refilter(&self, trigger: Trigger, q: &str) -> Self {
        let hits = filter_table(&self.table, q);
        let sel = self.sel.min(hits.len().saturating_sub(0));
        Self {
            trigger,
            nodes: self.nodes.clone(),
            table: self.table.clone(),
            applied: self.applied.clone(),
            query: None,
            filter: q.to_owned(),
            hits,
            sel: sel.min(self.row_count_for(q).saturating_sub(1)),
            first: 0,
        }
    }

    pub fn has_search(&self) -> bool {
        self.query.is_some()
    }

    /// The live query, whichever mode holds it.
    pub fn query_text(&self) -> String {
        match &self.query {
            Some(field) => field.value(),
            None => self.filter.clone(),
        }
    }

    /// The name the create row offers, when the query names no existing tag.
    pub fn offer_create(&self) -> Option<String> {
        offer_create_for(&self.table, &self.query_text())
    }

    fn row_count_for(&self, q: &str) -> usize {
        filter_table(&self.table, q).len() + usize::from(offer_create_for(&self.table, q).is_some())
    }

    /// Hits plus the create row.
    pub fn row_count(&self) -> usize {
        self.hits.len() + usize::from(self.offer_create().is_some())
    }

    pub fn first_visible(&self) -> usize {
        self.first.min(self.max_first())
    }

    fn max_first(&self) -> usize {
        self.row_count().saturating_sub(MAX_ROWS)
    }

    fn scroll_selection_into_view(&mut self) {
        let rows = visible_rows(self.row_count());
        if self.sel < self.first {
            self.first = self.sel;
        } else if self.sel >= self.first + rows {
            self.first = self.sel + 1 - rows;
        }
        self.first = self.first.min(self.max_first());
    }

    pub fn height(&self) -> f32 {
        let rows = visible_rows(self.row_count()) as f32 * theme::ROW_H;
        if self.has_search() {
            theme::PANEL_PAD + rows_top() + rows + 8.0
        } else {
            theme::PANEL_PAD * 2.0 + rows
        }
    }

    fn refilter_from_field(&mut self) {
        let q = self.query_text();
        self.hits = filter_table(&self.table, &q);
        self.sel = self.sel.min(self.row_count().saturating_sub(1));
        self.first = 0;
    }

    /// The picker was **left open** after applying `tag` ([ADR-188], amended
    /// 2026-08-27): tick that row, and empty the search box so the next tag can
    /// be typed straight away.
    ///
    /// Two things this deliberately does. It updates
    /// [`applied`](Self::applied) rather than waiting for the host to push a
    /// fresh set, because the checkmark is the only feedback a stay-open picker
    /// gives and it has to land on the same frame as the edit. And it clears
    /// the query: after `mv` has narrowed the list to one row and that row has
    /// been chosen, leaving `mv` in the box means the second tag of the visit
    /// starts with a select-all-and-delete. Nothing is lost — the box is what
    /// was typed to find the tag that is now applied.
    ///
    /// Only ever called on the field form. The `#tag` grammar commits and
    /// closes, because its query *is* the document text behind the panel.
    ///
    /// [ADR-188]: ../../../../docs/decisions/ADR-201-225.md#adr-211-chipclick--an-attribute-chip-is-the-control-for-its-attribute-and-a-tag-comes-off-at-the-chip
    pub fn reopen_after_apply(&mut self, engine: &mut TextEngine, tag: TagId) {
        self.applied.insert(tag);
        self.reopen(engine);
    }

    /// The other half: the picker was left open after a **ticked row was
    /// chosen again**, so untick it. Same reasoning as
    /// [`reopen_after_apply`](Self::reopen_after_apply) in every other
    /// respect, and the box is emptied for the same reason — the query is
    /// what was typed to find the row that has now been acted on.
    pub fn reopen_after_remove(&mut self, engine: &mut TextEngine, tag: TagId) {
        self.applied.remove(&tag);
        self.reopen(engine);
    }

    fn reopen(&mut self, engine: &mut TextEngine) {
        if self.query.is_some() {
            self.query = Some(TextField::new(engine, theme::role::ui()));
        }
        self.refilter_from_field();
        self.sel = 0;
    }

    pub fn key(
        &mut self,
        engine: &mut TextEngine,
        key: &Key,
        text: Option<&str>,
        mods: ModifiersState,
    ) -> Outcome {
        let named = match key {
            Key::Named(n) => Some(*n),
            _ => None,
        };
        let rows = self.row_count().max(1);
        match named {
            Some(NamedKey::ArrowDown) => {
                self.sel = (self.sel + 1) % rows;
                self.scroll_selection_into_view();
                Outcome::Consumed
            }
            Some(NamedKey::ArrowUp) => {
                self.sel = (self.sel + rows - 1) % rows;
                self.scroll_selection_into_view();
                Outcome::Consumed
            }
            Some(NamedKey::Enter) | Some(NamedKey::Tab) => self.choose(self.sel),
            _ => {
                let Some(field) = &mut self.query else {
                    // `#` mode: printable keys keep flowing into the document,
                    // and `refresh` re-filters from what lands there.
                    return Outcome::Ignored;
                };
                if named == Some(NamedKey::Backspace) && field.is_empty() {
                    return Outcome::Close;
                }
                field.key(engine, key, text, mods);
                self.refilter_from_field();
                Outcome::Consumed
            }
        }
    }

    /// Which row a panel-local point is over, in list indices.
    pub fn item_at(&self, local_y: f32) -> Option<usize> {
        let top = if self.has_search() { rows_top() } else { 0.0 };
        let row = ((local_y - top) / theme::ROW_H).floor();
        if row < 0.0 || row as usize >= visible_rows(self.row_count()) {
            return None;
        }
        let i = self.first_visible() + row as usize;
        (i < self.row_count()).then_some(i)
    }

    pub fn click(&self, local_y: f32) -> Outcome {
        match self.item_at(local_y) {
            Some(row) => self.choose(row),
            None => Outcome::Consumed,
        }
    }

    pub fn scroll_rows(&mut self, delta: isize) {
        if self.row_count() == 0 {
            return;
        }
        self.first = (self.first as isize + delta).clamp(0, self.max_first() as isize) as usize;
    }

    pub fn scrolls(&self) -> bool {
        self.row_count() > MAX_ROWS
    }

    /// Apply the row's tag — or, on a **ticked row of the chooser**, take it
    /// off again ([ADR-188], amended twice; see [`toggles_off`](Self::toggles_off)
    /// for the two forms that stay additive).
    ///
    /// A tag this does *not* toggle still commits [`Outcome::AddTag`] rather
    /// than closing quietly, and that is load-bearing: the commit is also what
    /// takes the typed `#home` back out of the row. `add_tag_to_all` is
    /// guarded, so the document sees no edit and there is no empty undo step.
    ///
    /// [ADR-188]: ../../../../docs/decisions/ADR-201-225.md#adr-211-chipclick--an-attribute-chip-is-the-control-for-its-attribute-and-a-tag-comes-off-at-the-chip
    fn choose(&self, i: usize) -> Outcome {
        if let Some(&t) = self.hits.get(i) {
            let tag = self.table[t].id;
            if self.toggles_off(tag) {
                return Outcome::RemoveTag(self.nodes.clone(), tag);
            }
            return Outcome::AddTag(self.nodes.clone(), tag);
        }
        if i == self.hits.len()
            && let Some(name) = self.offer_create()
        {
            return Outcome::CreateTag(self.nodes.clone(), name);
        }
        Outcome::Close
    }

    /// Whether choosing `tag` means *take it off*: only in the chooser, only
    /// on one node, and only when the row is already ticked.
    ///
    /// The two exclusions are the whole of the first amendment's argument,
    /// kept where it still holds. **`#tag` in the text** ([`query`](Self::query)
    /// is `None`) draws no checkmarks, so a second `#home` would remove
    /// something the panel never said was there. **A multi-node apply** ticks
    /// rows from the first node alone, so removing would mean opposite things
    /// to the blocks in the selection depending on what each already carried.
    fn toggles_off(&self, tag: TagId) -> bool {
        self.has_search() && self.nodes.len() == 1 && self.applied.contains(&tag)
    }
}

/// **Segment-aware** filter over the table's names, answering indices in table
/// order. An empty query keeps everything.
///
/// The rule itself is [`tags::matches_query`], which lives in `koru-tagname`
/// because the **search box completes a half-typed `#tag` with the same one**
/// and has no node model to reach it through. Two lists that narrow differently
/// for `fun/h` would be two products; this is the reason that crate exists.
///
/// The query is folded once here and each name once per keystroke, which is
/// what a table this size affords. A caller holding a table across keystrokes
/// — the search surface does — folds its names when they arrive instead.
fn filter_table(table: &[TagChoice], q: &str) -> Vec<usize> {
    let query = tags::fold_segments(q);
    table
        .iter()
        .enumerate()
        .filter(|(_, tag)| tags::matches_query(&query, &tags::fold_segments(&tag.name)))
        .map(|(i, _)| i)
        .collect()
}

/// The name the create row offers, or `None` when the query names a tag that
/// already exists — **or is not a name a tag can have**.
///
/// This is the entry surface, so this is where the guardrails run
/// ([`TagName::parse`]): trimmed segments, collapsed `//`, no leading or
/// trailing `/`, at most [`tags::MAX_DEPTH`] deep. A refusal is a quiet row
/// that does not appear rather than an error — the picker is still open, the
/// text is still in the document, and one more keystroke may well fix it.
fn offer_create_for(table: &[TagChoice], q: &str) -> Option<String> {
    let name = tags::TagName::parse(q).ok()?;
    let key = name.fold_key();
    let exists = table.iter().any(|t| tags::fold_key(&t.name) == key);
    (!exists).then(|| name.into_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Vec<TagChoice> {
        [("launch", Some("red")), ("marketing", None), ("mvp", None)]
            .into_iter()
            .map(|(name, color)| TagChoice {
                id: TagId::new(),
                name: name.to_owned(),
                color: color.map(str::to_owned),
            })
            .collect()
    }

    fn nodes() -> Vec<NodeId> {
        vec![NodeId::new()]
    }

    /// **In the chooser a ticked row toggles off** ([ADR-188], amended twice).
    /// An unticked row adds; the row the block already carries removes, which
    /// is the only way the checkmark the panel is drawing can be reversed
    /// without leaving it.
    #[test]
    fn typing_filters_and_a_ticked_row_toggles_off() {
        let mut engine = TextEngine::new();
        let t = table();
        let applied: HashSet<TagId> = [t[2].id].into_iter().collect();
        let mut p = TagPicker::opened(
            &mut engine,
            Trigger::at_start(),
            nodes(),
            t.clone(),
            applied,
        );
        assert_eq!(p.hits.len(), 3);
        for ch in ["m", "v"] {
            p.key(
                &mut engine,
                &Key::Character((*ch).into()),
                Some(ch),
                ModifiersState::empty(),
            );
        }
        assert_eq!(p.hits.len(), 1, "mv narrows to mvp");
        // mvp is ticked, and the panel is saying so: choosing it takes it off.
        assert!(matches!(p.choose(0), Outcome::RemoveTag(_, id) if id == t[2].id));

        // An unticked tag adds.
        let fresh = TagPicker::opened(
            &mut engine,
            Trigger::at_start(),
            nodes(),
            t.clone(),
            HashSet::new(),
        );
        assert!(matches!(fresh.choose(0), Outcome::AddTag(_, id) if id == t[0].id));
    }

    /// **`#tag` in the text never removes**, however many times it is typed.
    /// Its rows carry no checkmark — they complete what is being typed — so a
    /// removal would undo something the panel never said was there. `#mvp`
    /// twice is one `#mvp`, not none, and the guarded `add_tag_to_all` turns
    /// the second into no edit at all.
    #[test]
    fn the_hash_grammar_stays_additive() {
        let t = table();
        let applied: HashSet<TagId> = [t[2].id].into_iter().collect();
        let p = TagPicker::for_hash(Trigger::at_start(), nodes(), t.clone(), applied, "mvp")
            .expect("mvp matches the table");
        assert_eq!(p.hits.len(), 1);
        assert!(matches!(p.choose(0), Outcome::AddTag(_, id) if id == t[2].id));
    }

    /// **A multi-node apply never removes either.** The checkmarks are the
    /// *first* node's, so a toggle would spend one block's answer on all of
    /// them — taking the tag off the blocks that had it and off the ones that
    /// never did.
    #[test]
    fn a_multi_node_apply_stays_additive() {
        let mut engine = TextEngine::new();
        let t = table();
        let applied: HashSet<TagId> = [t[2].id].into_iter().collect();
        let p = TagPicker::opened(
            &mut engine,
            Trigger::at_start(),
            vec![NodeId::new(), NodeId::new()],
            t.clone(),
            applied,
        );
        let row = p.hits.iter().position(|&i| t[i].id == t[2].id).unwrap();
        assert!(matches!(p.choose(row), Outcome::AddTag(_, id) if id == t[2].id));
    }

    /// **Unticking leaves the panel up, the same as ticking does.** Removing
    /// is part of the same visit — the whole reason the chooser stays open is
    /// that a block carries several tags — so the row unticks live and the box
    /// empties for the next one.
    #[test]
    fn reopening_after_a_removal_unticks_the_row() {
        let mut engine = TextEngine::new();
        let t = table();
        let applied: HashSet<TagId> = [t[0].id, t[2].id].into_iter().collect();
        let mut p = TagPicker::opened(
            &mut engine,
            Trigger::at_start(),
            nodes(),
            t.clone(),
            applied,
        );
        for ch in ["m", "v"] {
            p.key(
                &mut engine,
                &Key::Character((*ch).into()),
                Some(ch),
                ModifiersState::empty(),
            );
        }
        assert!(matches!(p.choose(0), Outcome::RemoveTag(_, id) if id == t[2].id));

        p.reopen_after_remove(&mut engine, t[2].id);
        assert!(!p.applied.contains(&t[2].id), "the row is unticked");
        assert!(p.applied.contains(&t[0].id), "the block's other tag stays");
        assert_eq!(p.query_text(), "", "and the box is empty again");
        assert_eq!(p.hits.len(), 3, "so the whole table is on offer");

        // And the row that was just unticked now adds, rather than removing
        // again — the toggle reads the state the panel is drawing.
        let row = p.hits.iter().position(|&i| t[i].id == t[2].id).unwrap();
        assert!(matches!(p.choose(row), Outcome::AddTag(_, id) if id == t[2].id));
    }

    /// **After an apply the field form is re-armed rather than rebuilt**: the
    /// row ticks, the box empties, and the whole table is listed again — which
    /// is what makes the second tag of a visit one keystroke rather than a
    /// select-all-and-delete. The table and the target nodes are untouched.
    #[test]
    fn reopening_after_an_apply_ticks_the_row_and_empties_the_box() {
        let mut engine = TextEngine::new();
        let t = table();
        let nodes = nodes();
        let mut p = TagPicker::opened(
            &mut engine,
            Trigger::at_start(),
            nodes.clone(),
            t.clone(),
            HashSet::new(),
        );
        for ch in ["m", "v"] {
            p.key(
                &mut engine,
                &Key::Character((*ch).into()),
                Some(ch),
                ModifiersState::empty(),
            );
        }
        assert_eq!(p.hits.len(), 1, "mv narrows to mvp");
        assert!(matches!(p.choose(0), Outcome::AddTag(_, id) if id == t[2].id));

        p.reopen_after_apply(&mut engine, t[2].id);
        assert!(p.applied.contains(&t[2].id), "the row is ticked");
        assert_eq!(p.query_text(), "", "and the box is empty again");
        assert_eq!(p.hits.len(), 3, "so the whole table is on offer");
        assert_eq!((p.sel, p.first), (0, 0));
        assert_eq!(p.nodes, nodes, "still the same target");

        // A second choice through the same panel, on a different, unticked
        // row: it adds, and the first tag keeps its check.
        assert!(matches!(p.choose(0), Outcome::AddTag(_, id) if id == t[0].id));
        p.reopen_after_apply(&mut engine, t[0].id);
        assert_eq!(p.applied.len(), 2);
    }

    #[test]
    fn an_unknown_query_offers_creation_and_an_exact_match_does_not() {
        let mut engine = TextEngine::new();
        let mut p = TagPicker::opened(
            &mut engine,
            Trigger::at_start(),
            nodes(),
            table(),
            HashSet::new(),
        );
        for ch in "roadmap".split("").filter(|s| !s.is_empty()) {
            p.key(
                &mut engine,
                &Key::Character(ch.into()),
                Some(ch),
                ModifiersState::empty(),
            );
        }
        assert!(p.hits.is_empty());
        assert_eq!(p.offer_create().as_deref(), Some("roadmap"));
        assert!(matches!(
            p.choose(0),
            Outcome::CreateTag(_, name) if name == "roadmap"
        ));

        // Case-insensitively exact: nothing to create, the row is the tag's.
        let mut exact = TagPicker::opened(
            &mut engine,
            Trigger::at_start(),
            nodes(),
            table(),
            HashSet::new(),
        );
        for ch in "MVP".split("").filter(|s| !s.is_empty()) {
            exact.key(
                &mut engine,
                &Key::Character(ch.into()),
                Some(ch),
                ModifiersState::empty(),
            );
        }
        assert_eq!(exact.hits.len(), 1);
        assert_eq!(exact.offer_create(), None);
    }

    /// The shortcode contract, applied to `#`: one character that matches
    /// opens; one that matches nothing does not; two of anything do, so a new
    /// tag can still be born from prose.
    #[test]
    fn hash_mode_never_opens_on_a_dead_query() {
        let t = table();
        assert!(
            TagPicker::for_hash(Trigger::at_start(), nodes(), t.clone(), HashSet::new(), "m")
                .is_some(),
            "a matching first character opens"
        );
        assert!(
            TagPicker::for_hash(Trigger::at_start(), nodes(), t.clone(), HashSet::new(), "z")
                .is_none(),
            "one character matching nothing stays quiet"
        );
        assert!(
            TagPicker::for_hash(
                Trigger::at_start(),
                nodes(),
                t.clone(),
                HashSet::new(),
                "zz"
            )
            .is_some(),
            "two characters are an intent, and the create row is the offer"
        );
        let p = TagPicker::for_hash(Trigger::at_start(), nodes(), t, HashSet::new(), "zz").unwrap();
        assert_eq!(p.hits.len(), 0);
        assert_eq!(p.offer_create().as_deref(), Some("zz"));
    }

    #[test]
    fn refilter_follows_the_document_query() {
        let t = table();
        let p = TagPicker::for_hash(Trigger::at_start(), nodes(), t, HashSet::new(), "m").unwrap();
        assert_eq!(p.hits.len(), 2, "marketing and mvp");
        let narrowed = p.refilter(Trigger::at_start(), "mar");
        assert_eq!(narrowed.hits.len(), 1);
        assert_eq!(narrowed.filter, "mar");
        // Backspace to nothing lists the whole table rather than closing —
        // the trigger is still armed, and dismissal is refresh's business.
        let all = narrowed.refilter(Trigger::at_start(), "");
        assert_eq!(all.hits.len(), 3);
    }

    // ------------------------------------------------- folding and paths

    /// The bug this closes: `eq_ignore_ascii_case` offered *Create `#STRASSE`*
    /// beside an existing `#straße`, and the model then held two tags a user
    /// and the search index both consider one. The offer now folds the way
    /// `koru_core::tags` does — full Unicode case **and** accents, so it never
    /// invites a duplicate the index would have merged anyway.
    #[test]
    fn the_create_offer_folds_the_way_the_table_and_the_index_do() {
        let table: Vec<TagChoice> = ["straße", "café", "home", "İstanbul"]
            .into_iter()
            .map(|name| TagChoice {
                id: TagId::new(),
                name: name.to_owned(),
                color: None,
            })
            .collect();
        for existing in [
            "STRASSE", "Strasse", "CAFÉ", "Café", "cafe", "CAFE", "HOME", "istanbul", "Istanbul",
        ] {
            assert_eq!(
                offer_create_for(&table, existing),
                None,
                "{existing:?} already exists"
            );
        }
        // Folding accents is not folding words.
        assert_eq!(offer_create_for(&table, "cave").as_deref(), Some("cave"));
    }

    /// The guardrails live here, at the surface where somebody types — never in
    /// `SystemDoc::create_tag`, which must read back anything already written.
    #[test]
    fn the_create_offer_applies_the_path_guardrails() {
        let table = table();
        assert_eq!(
            offer_create_for(&table, "  fun / home ").as_deref(),
            Some("fun/home"),
            "segments are trimmed"
        );
        assert_eq!(
            offer_create_for(&table, "fun//home").as_deref(),
            Some("fun/home"),
            "an interior double separator collapses"
        );
        for refused in ["", "   ", "/fun", "fun/", "//", "a/b/c/d/e/f"] {
            assert_eq!(offer_create_for(&table, refused), None, "{refused:?}");
        }
        assert!(offer_create_for(&table, "a/b/c/d/e").is_some(), "the cap");
    }

    /// Typing `fun` finds both children; typing `fun/h` narrows to one.
    #[test]
    fn filtering_is_segment_aware() {
        let table: Vec<TagChoice> = ["fun", "fun/home", "Fun/Outside", "funhouse", "work/home"]
            .into_iter()
            .map(|name| TagChoice {
                id: TagId::new(),
                name: name.to_owned(),
                color: None,
            })
            .collect();
        let names = |q: &str| -> Vec<&str> {
            filter_table(&table, q)
                .into_iter()
                .map(|i| table[i].name.as_str())
                .collect()
        };

        assert_eq!(
            names("fun"),
            ["fun", "fun/home", "Fun/Outside", "funhouse"],
            "a bare segment matches any segment containing it"
        );
        assert_eq!(names("fun/h"), ["fun/home"], "a path anchors the run");
        assert_eq!(
            names("Fun/"),
            ["fun", "fun/home", "Fun/Outside", "funhouse"],
            "a trailing separator folds away mid-typing"
        );
        assert_eq!(
            names("home"),
            ["fun/home", "work/home"],
            "a leaf is findable without knowing where it is filed"
        );
        assert_eq!(names("FUN/OUT"), ["Fun/Outside"], "and it all folds case");
        assert_eq!(names("n/h"), Vec::<&str>::new(), "not a raw substring");
        assert_eq!(names("").len(), table.len());
    }

    /// The row draws the branch quietly and the tag's own name plainly.
    #[test]
    fn a_row_label_splits_the_branch_from_the_leaf() {
        let child = TagChoice {
            id: TagId::new(),
            name: "fun/home/attic".to_owned(),
            color: None,
        };
        assert_eq!(child.label(), ("fun/home/".to_owned(), "attic"));
        let root = TagChoice {
            id: TagId::new(),
            name: "mvp".to_owned(),
            color: None,
        };
        assert_eq!(root.label(), (String::new(), "mvp"));
    }

    #[test]
    fn rows_hit_test_below_the_search_box() {
        let mut engine = TextEngine::new();
        let p = TagPicker::opened(
            &mut engine,
            Trigger::at_start(),
            nodes(),
            table(),
            HashSet::new(),
        );
        assert_eq!(p.item_at(rows_top() - 1.0), None, "in the search box");
        assert_eq!(p.item_at(rows_top() + 0.5 * theme::ROW_H), Some(0));
        let past = rows_top() + (p.row_count() as f32 + 0.5) * theme::ROW_H;
        assert_eq!(p.item_at(past), None);
    }
}
