// SPDX-License-Identifier: MPL-2.0

//! `POST /v1/attributes/query` — the attribute query service's wire bodies.
//!
//! One cross-workspace question: *which nodes carry these attributes?* The
//! answer comes from the store's derived read model, never from opening a
//! document, and every row carries `(workspace, document, node)` **together** —
//! a node id alone means nothing outside the document that minted it.
//!
//! The query travels in the body per ADR-103: a filter can name a tag or an
//! attribute key, and a URL is written to the access log.

use koru_store_core::ids::{DocumentId, WorkspaceId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `POST /v1/attributes/query`.
///
/// Every filter is optional and absent means "no constraint". Filters
/// compose with AND; the values inside `tags` compose with OR (any of).
/// The base set is nodes carrying **at least one attribute** — a bare
/// paragraph is never a row of an attribute query, even under `missing`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct AttributeQuery {
    /// Which workspaces to search. Empty means every workspace this store
    /// holds.
    #[serde(default)]
    pub workspaces: Vec<WorkspaceId>,
    /// Only nodes carrying this attribute key (`koru.due`, or a custom
    /// definition's key).
    #[serde(default)]
    pub has: Option<String>,
    /// Only nodes **not** carrying this key. Combined with the base rule this
    /// reads "has some attribute, but not this one".
    #[serde(default)]
    pub missing: Option<String>,
    /// Only nodes carrying any of these tags, each a `TagId` in storage form.
    /// Ids, never names — the tag table owns names and rename must not change
    /// what a saved query matches.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Only nodes whose `koru.priority` equals this token
    /// (`low | medium | high | urgent`).
    #[serde(default)]
    pub priority: Option<String>,
    /// Only nodes whose `koru.priority` is this token or more important —
    /// the semantic order `low < medium < high < urgent`, never the
    /// alphabetic one.
    #[serde(default)]
    pub min_priority: Option<String>,
    /// Which `koru.due` values match, **stated per temporal type**.
    #[serde(default)]
    pub due: Option<DueFilter>,
    /// `due`, `priority` or `modified`. Absent means `modified` — newest
    /// first, which is what a surface with no opinion wants.
    #[serde(default)]
    pub sort: Option<String>,
    /// Ask for **corpus-wide counts** beside the page. `tag` is the only
    /// token; absent means no groups, and the response's `groups` is empty.
    ///
    /// It is a separate question from paging, deliberately: a client can count
    /// a page itself, and what it cannot do is count the rows it did not
    /// fetch. So the counts come from their own statement over the same
    /// filter, and a group's count is the whole matching corpus rather than
    /// the page above it.
    #[serde(default)]
    pub group_by: Option<String>,
    /// Rows per page. Absent means the server default; the server also caps
    /// it, so a huge number is a full page rather than a full table.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Rows to skip. Page by increasing this until a short page comes back.
    #[serde(default)]
    pub offset: Option<usize>,
}

/// Which due values match, **one window per temporal type** ([ADR-183]).
///
/// Koru stores three temporal types and never one with a nullable timezone, so
/// a filter says which of them it is asking about. The windows compose with
/// **OR** — "everything due this week, however it was set" is genuinely two
/// questions asked of two kinds of value — and a row of a type this filter did
/// not name simply does not match.
///
/// What this deliberately does **not** do is fold one type onto the other. The
/// shape it replaced took a single bound and cut instants at the UTC day
/// boundary, which is wrong by a day for a third of the planet: 15:00Z on
/// September 2 is already September 3 in Tokyo. A caller that wants a
/// zone-exact window computes its own millisecond edges, because the caller is
/// the layer that knows which zone the person is asking in.
///
/// Naming neither window is refused: it is a question nobody asked, and
/// answering "everything" would be a guess.
///
/// [ADR-183]: ../../../docs/decisions/ADR-201-225.md#adr-206-timemodel--koru-has-three-temporal-types-not-one-with-a-nullable-timezone
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct DueFilter {
    /// Compared against dues stored as a **calendar date** — and against
    /// floating wall clocks, whose day needs no zone to read.
    #[serde(default)]
    pub dates: Option<DateWindow>,
    /// Compared against dues stored as a **zoned instant**.
    #[serde(default)]
    pub instants: Option<InstantWindow>,
}

/// A half-open calendar window, `[after, before)`. Each bound is strictly
/// `YYYY-MM-DD`; absent means unbounded on that side.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct DateWindow {
    #[serde(default)]
    pub after: Option<String>,
    #[serde(default)]
    pub before: Option<String>,
}

/// A half-open instant window, `[after_ms, before_ms)`, in epoch milliseconds
/// UTC. Absent means unbounded on that side.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema)]
pub struct InstantWindow {
    #[serde(default)]
    pub after_ms: Option<i64>,
    #[serde(default)]
    pub before_ms: Option<i64>,
}

/// What the query answers with.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AttributeQueryResponse {
    /// The matching nodes, one row each, in the requested order.
    pub rows: Vec<AttributeRow>,
    /// Every attribute definition in play: the system registry always, plus
    /// the scoped workspaces' custom definitions — so a filter UI builds its
    /// controls from types rather than guessing.
    pub definitions: Vec<AttributeDefinition>,
    /// The scoped workspaces' tag tables (live tags only, merge tombstones
    /// resolved away), so a `koru.tag` value — a `TagId` — resolves to a name
    /// and a chip color without another request.
    pub tags: Vec<AttributeTag>,
    /// Corpus-wide counts per tag, when the query asked for them with
    /// `group_by`. Empty otherwise.
    ///
    /// **Names are not repeated here**: a group carries the `TagId` and the
    /// count, and `tags` above is where that id becomes a word and a color.
    /// Two spellings of one name in one response is a drift waiting to happen.
    #[serde(default)]
    pub groups: Vec<AttributeGroup>,
    /// Whether every workspace in scope has reached the read model at all.
    /// `false` is "still indexing" — the projector is debounced, so an
    /// attribute set a moment ago may not be a row yet.
    pub projected: bool,
    /// The workspaces the answer may be behind on, by the same rule search
    /// uses: named rather than waited for.
    #[serde(default)]
    pub updating_workspaces: Vec<WorkspaceId>,
    /// The page size actually applied, after the server's cap.
    pub limit: usize,
    /// The offset actually applied.
    pub offset: usize,
}

/// One matching node. The identity is `(workspace, document, node)`, carried
/// together — never a node id on its own.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AttributeRow {
    pub workspace: WorkspaceId,
    pub document: DocumentId,
    /// The node id, meaningful only inside `document`.
    pub node: String,
    /// The node's kind token — `paragraph`, `task`, `image`.
    pub node_kind: String,
    /// The node's text, truncated to a bounded number of characters —
    /// characters, never bytes, so no language is cut mid-glyph.
    pub excerpt: String,
    /// The source document's title, from the workspace listing.
    pub document_title: String,
    /// Whether the source document sits in the trash, **inherited down the
    /// document tree** — a child of a binned parent carries no stamp of its
    /// own. Trashed rows are flagged, never hidden: restore must be able to
    /// find them.
    pub trashed: bool,
    /// Every attribute on the node — not only the ones the filter named, so a
    /// row can be drawn without a second request.
    pub attributes: Vec<AttributeRowValue>,
}

/// One attribute on one row, as the projection carries it: the value's shape
/// in `kind` and its canonical string in `value`.
///
/// `kind` is one of `date`, `datetime_floating`, `datetime_zoned`, `number`,
/// `bool`, `ref`, `url`, `text` — **three temporal tokens, and no `datetime`
/// that might be either** ([ADR-183]). `value` is the canonical string for
/// that kind: a calendar date as `YYYY-MM-DD`, a floating wall clock as
/// `YYYY-MM-DDTHH:MM:SS`, a zoned instant as decimal epoch-ms, a tag as its
/// `TagId`.
///
/// [ADR-183]: ../../../docs/decisions/ADR-201-225.md#adr-206-timemodel--koru-has-three-temporal-types-not-one-with-a-nullable-timezone
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AttributeRowValue {
    /// The attr record's own id, stable across edits to its value.
    pub id: String,
    /// The attribute key the value is stored under — `koru.due`, `koru.tag`,
    /// or a custom definition's key.
    pub role: String,
    pub kind: String,
    pub value: String,
    /// The IANA zone a `datetime_zoned` value was meant in, and absent for
    /// every other kind.
    ///
    /// It travels with the instant rather than being dropped as presentation:
    /// a consumer handed an epoch-ms number alone would be holding the
    /// ambiguous shape the three types exist to abolish. Which zone a reader
    /// *displays* it in is that consumer's own, separate decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
}

/// One attribute definition, flattened to tokens. The same shape for the
/// compiled-in system registry and a workspace's custom records; `system`
/// says which this is.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AttributeDefinition {
    /// Stable identity: the namespaced key itself for a system attribute, a
    /// generated UUID for a custom one. Rename never changes it.
    pub id: String,
    /// The key values are stored under.
    pub key: String,
    /// Display only, freely renameable.
    pub label: String,
    /// `text`, `number`, `boolean`, `date`, `datetime_zoned`,
    /// `datetime_floating`, `select`, `multi_select`.
    ///
    /// **The definition declares which temporal type the key takes.** A shared
    /// key such as `koru.due` is `datetime_zoned` and refuses a floating
    /// value; a personal one such as `koru.plan` is `datetime_floating` and
    /// refuses a zoned one ([ADR-183]).
    pub value_type: String,
    /// `one | many`.
    pub cardinality: String,
    /// `shared | personal`. Personal values never appear in this response's
    /// rows — they live per profile, not on nodes.
    pub scope: String,
    pub system: bool,
    /// The legal tokens for a select; empty otherwise, and empty for
    /// `koru.tag`, whose options are the tag table.
    #[serde(default)]
    pub options: Vec<String>,
    pub filterable: bool,
    pub sortable: bool,
    pub groupable: bool,
}

/// One tag's share of the whole matching corpus.
///
/// **Keyed by `(workspace, tag)`**, because a `TagId` belongs to one
/// workspace's table and the `tags` array is per table too. A multi-workspace
/// query gets a group per table rather than one merged heading.
///
/// **The id is the one a live table can name.** A tag merged into another
/// re-points no node ([ADR-176]), so nodes go on carrying the merged-away id;
/// the store folds those counts into the surviving tag before answering, so a
/// merge shows as one group rather than splitting into a named half and a
/// nameless one. The consequence worth knowing: the id here is not necessarily
/// an id any row's `koru.tag` value spells.
///
/// [ADR-176]: ../../../docs/decisions/ADR-176-200.md#adr-176-consenton--telemetry-consent-defaults-on-and-a-discoverable-opt-out-is-what-buys-it
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AttributeGroup {
    /// Which workspace's tag table names this id.
    pub workspace: WorkspaceId,
    /// The `TagId` in storage form. Look it up in `tags` for a name and color.
    pub tag: String,
    /// Distinct nodes carrying it across the whole matching corpus — not the
    /// page. A node with two tags counts once in each group, so the counts sum
    /// to more than `rows.len()` even on a complete page.
    pub count: usize,
    /// How many of `count` sit in a binned document. Binned rows are counted
    /// and flagged rather than dropped, exactly as they are in `rows` — a
    /// heading that hid them would disagree with the list underneath it.
    pub trashed: usize,
}

/// One live tag from a scoped workspace's tag table.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AttributeTag {
    /// The `TagId`, which is what node attributes store.
    pub id: String,
    pub name: String,
    /// An accent color token, or absent for the default chip.
    #[serde(default)]
    pub color: Option<String>,
    /// Which workspace's table this row is from — tags are workspace-scoped,
    /// and a multi-workspace query returns each table's rows.
    pub workspace: WorkspaceId,
}
