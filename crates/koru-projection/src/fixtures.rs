// SPDX-License-Identifier: MPL-2.0

//! **A workspace corpus at the size the product has to survive**, built the
//! same way twice.
//!
//! Roadmap rev6 §6 names the shape a task surface is judged against — three
//! workspaces, 250 documents, 5,000 tasks, 50,000 rows that are not tasks, and
//! **one workspace left cold** — and every plan that measures anything about
//! the tasks query needs that corpus rather than the three-row documents a unit
//! test builds. This is that corpus, as a builder: R-1's freshness, pagination
//! and no-accumulating-work assertions run on it, and P-Base measures on the
//! same one, so a number and an assertion are talking about the same fixture.
//!
//! Compiled in rather than test-only, for [`koru_store_core::fixtures`]' reason:
//! a `dev-dependency` cannot cross crates, and the daemon's suites need it too.
//!
//! ## Deterministic, and that is the whole of its identity
//!
//! **Nothing here reads a clock or a random number generator.** Every id, title,
//! due date, priority, list membership and lifecycle comes from
//! [`TaskCorpus::seed`] through one mixing function, so two builds of the same
//! `TaskCorpus` are the same corpus down to the byte — which is what makes a
//! failure reproducible and a measurement comparable with yesterday's. Ids are
//! UUID-shaped strings minted from the seed rather than `Uuid::new_v4`, for the
//! same reason: a fixture whose identity moves every run cannot be paged
//! through twice and compared.
//!
//! It is a *fixture* identity and nothing else. Real ids are client-minted v4
//! UUIDs ([ADR-39]); these are v4-shaped so every parser and every SQL join
//! behaves exactly as it does on real content, and they are never written
//! anywhere a person's workspace can see.
//!
//! ## The plan is data; writing it is one function
//!
//! [`TaskCorpus::plan`] answers a [`Corpus`] — every workspace, document and
//! task as plain fields, with no database in sight. [`Corpus::write_into`]
//! writes the warm workspaces into a [`Projections`], and
//! [`Corpus::sweep_in`] writes one named workspace, which is what a cold
//! workspace converging *is*: the sweep landed and its documents arrived.
//!
//! So a test asserts against the plan rather than against a second copy of the
//! generator's arithmetic — "the walk returned exactly the tasks the plan says
//! are open and live" is a real assertion, where re-deriving the expected count
//! from the same formula would be the generator agreeing with itself.
//!
//! ## The trap: a projection seeded behind the store's back does not survive
//!
//! This writes `projection.db` directly, which is right for measuring the query
//! and **wrong for seeding a running daemon**. The projector retires a
//! workspace the authoritative store does not have —
//! `Projections::remove_workspace`, from `begin_sweep`'s
//! `WorkspaceNotFound` arm — and every tasks query asks for a sweep of every
//! workspace in its scope. So a corpus written into a live store's root is
//! deleted by the first query that names it, which looks exactly like the query
//! returning nothing. Drive this against a `Projections` you own; drive the
//! daemon through its own doors.
//!
//! [ADR-39]: ../../../docs/decisions/ADR-026-050.md
//! [`koru_store_core::fixtures`]: ../../koru_store_core/fixtures/index.html

use koru_core::attributes::{
    KEY_DUE, KEY_LIST, KEY_PRIORITY, KEY_TAG, KEY_TASK, KEY_TASK_STATE, Lifecycle, Priority,
    TaskReason,
};
use koru_store_core::ids::{DocumentId, WorkspaceId};

use crate::model::{DocumentProjection, ListedDocument, ProjectedAttr, ProjectedNode};
use crate::store::{ProjectionError, Projections};

/// The corpus to build.
///
/// [`roadmap`](Self::roadmap) is the shape rev6 §6 names and the one a
/// measurement should quote; [`small`](Self::small) is the same generator at a
/// size a unit test can afford, for the properties that are about the *shape*
/// of an answer rather than about scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskCorpus {
    /// Everything variable in the corpus is derived from this. Change it and
    /// you get a different corpus; keep it and you get the same one forever.
    pub seed: u64,
    /// How many workspaces, cold ones included.
    pub workspaces: usize,
    /// How many of them are left **cold** — planned, with documents, and
    /// nothing written to the read model until [`Corpus::sweep_in`] writes it.
    /// The last workspaces of the list are the cold ones.
    pub cold_workspaces: usize,
    /// Documents across every workspace, dealt round-robin.
    pub documents: usize,
    /// Task rows across every document, dealt round-robin.
    pub tasks: usize,
    /// Rows that are **not** tasks. They are what makes the corpus a corpus: a
    /// query that got slower because of them would be reading the whole
    /// workspace to answer a question about a fraction of it.
    pub other_rows: usize,
}

/// The seed the shipped corpus is built from. A constant, and named, so a
/// report can say which corpus produced a number.
pub const SEED: u64 = 0x_0BAD_5EED_0000_0001;

impl TaskCorpus {
    /// **Roadmap rev6 §6**: 3 workspaces, 250 documents, 5,000 tasks, 50,000
    /// other rows, one workspace cold.
    pub const fn roadmap() -> Self {
        Self {
            seed: SEED,
            workspaces: 3,
            cold_workspaces: 1,
            documents: 250,
            tasks: 5_000,
            other_rows: 50_000,
        }
    }

    /// The same generator at a size a suite can afford several times over.
    ///
    /// **For the rules that are not about scale** — a filter narrows, a repeat
    /// costs one query — where building fifty-five thousand rows again would
    /// buy nothing but seconds. The size claims belong to
    /// [`roadmap`](Self::roadmap), and one test carrying it is enough.
    pub const fn medium() -> Self {
        Self {
            seed: SEED,
            workspaces: 3,
            cold_workspaces: 1,
            documents: 40,
            tasks: 800,
            other_rows: 8_000,
        }
    }

    /// The same generator, small enough for a unit test that is asserting a
    /// rule rather than a size.
    pub const fn small() -> Self {
        Self {
            seed: SEED,
            workspaces: 3,
            cold_workspaces: 1,
            documents: 9,
            tasks: 45,
            other_rows: 90,
        }
    }

    pub const fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    /// Deal the whole corpus out, deterministically, touching no database.
    pub fn plan(&self) -> Corpus {
        let mut workspaces: Vec<PlannedWorkspace> = (0..self.workspaces)
            .map(|w| PlannedWorkspace {
                id: WorkspaceId::parse(&uuid_from(self.seed, TAG_WORKSPACE, w as u64))
                    .expect("a fixture workspace id is a uuid"),
                name: format!("Workspace {}", w + 1),
                cold: w + self.cold_workspaces >= self.workspaces,
                lists: (0..LISTS_PER_WORKSPACE)
                    .map(|l| uuid_from(self.seed, TAG_LIST, (w * LISTS_PER_WORKSPACE + l) as u64))
                    .collect(),
                documents: Vec::new(),
            })
            .collect();

        for d in 0..self.documents {
            let workspace = d % self.workspaces.max(1);
            let h = mix(self.seed, TAG_DOCUMENT, d as u64);
            let ordinal = workspaces[workspace].documents.len() as u32;
            workspaces[workspace].documents.push(PlannedDocument {
                id: DocumentId::parse(&uuid_from(self.seed, TAG_DOCUMENT, d as u64))
                    .expect("a fixture document id is a uuid"),
                title: format!("Document {:04}", d + 1),
                ordinal,
                // A binned document is a fact about the *manifest*, so its rows
                // are still answered — flagged, never hidden. One in
                // twenty-five, which is enough that a page meets one.
                trashed: h % 25 == 24,
                root: uuid_from(self.seed, TAG_ROOT, d as u64),
                section: uuid_from(self.seed, TAG_SECTION, d as u64),
                tasks: Vec::new(),
                other_rows: 0,
            });
        }

        // Documents in workspace order, so a task's document is found by index
        // without a second walk.
        let mut order: Vec<(usize, usize)> = Vec::with_capacity(self.documents);
        for (w, workspace) in workspaces.iter().enumerate() {
            for d in 0..workspace.documents.len() {
                order.push((w, d));
            }
        }
        // Dealt in the *original* round-robin order, so which workspace a task
        // lands in does not depend on how the documents were grouped.
        order.sort_by_key(|(w, d)| d * self.workspaces + w);

        if !order.is_empty() {
            for t in 0..self.tasks {
                let (w, d) = order[t % order.len()];
                let task = self.plan_task(t, &workspaces[w], &workspaces[w].documents[d]);
                workspaces[w].documents[d].tasks.push(task);
            }
            for r in 0..self.other_rows {
                let (w, d) = order[r % order.len()];
                workspaces[w].documents[d].other_rows += 1;
            }
        }

        Corpus {
            workspaces,
            other_rows: if order.is_empty() { 0 } else { self.other_rows },
            shape: *self,
        }
    }

    fn plan_task(
        &self,
        index: usize,
        workspace: &PlannedWorkspace,
        document: &PlannedDocument,
    ) -> PlannedTask {
        let h = mix(self.seed, TAG_TASK, index as u64);
        let node = uuid_from(self.seed, TAG_TASK, index as u64);
        // Seven in ten carry a due date; of those, one in ten carries **no
        // marker**, which is the due-derived task the derivation exists for.
        let due = (h % 10 < 7).then(|| a_date(h));
        let marker = if due.is_some() && (h / 10) % 10 == 0 {
            None
        } else {
            Some(REASONS[(h / 100) as usize % REASONS.len()])
        };
        PlannedTask {
            workspace: workspace.id,
            document: document.id,
            reference: format!(
                "task:{}:{}:{node}",
                workspace.id.to_storage(),
                document.id.to_storage()
            ),
            node,
            kind: KINDS[(h / 1_000) as usize % KINDS.len()],
            title: format!("Task {:05}", index + 1),
            marker,
            due,
            priority: ((h / 10_000) % 10 < 4)
                .then(|| PRIORITIES[(h / 100_000) as usize % PRIORITIES.len()]),
            list: ((h / 1_000_000) % 2 == 0).then(|| {
                workspace.lists[(h / 10_000_000) as usize % workspace.lists.len()].clone()
            }),
            completed: (h / 100_000_000) % 4 == 0,
            // A node in its own document's bin: rare, and off the default
            // answer until `include_trashed` asks for it.
            in_trash: h % 50 == 0,
            // Half hang off the document root and half off its one section, so
            // a breadcrumb has more than one shape to build.
            under_section: h % 2 == 0,
            document_trashed: document.trashed,
            cold: workspace.cold,
        }
    }
}

/// One workspace of the plan.
#[derive(Clone, Debug)]
pub struct PlannedWorkspace {
    pub id: WorkspaceId,
    pub name: String,
    /// Nothing of this workspace is written until [`Corpus::sweep_in`] writes
    /// it: no listing, no rows, so a port names it in `updating_workspaces` and
    /// a query answers what it has.
    pub cold: bool,
    /// The `koru.list` ids this workspace's tasks are dealt from.
    pub lists: Vec<String>,
    pub documents: Vec<PlannedDocument>,
}

/// One document of the plan.
#[derive(Clone, Debug)]
pub struct PlannedDocument {
    pub id: DocumentId,
    pub title: String,
    pub ordinal: u32,
    /// The **manifest** binned it. Its rows are still answered, flagged.
    pub trashed: bool,
    /// The document's own title row, which every other row hangs under.
    pub root: String,
    /// One intermediate row, so a breadcrumb has an ancestor to find.
    pub section: String,
    pub tasks: Vec<PlannedTask>,
    /// How many rows in this document are not tasks.
    pub other_rows: usize,
}

/// One task of the plan — every field the query can be asked about, so a test
/// states its expectation against the plan rather than against the generator's
/// arithmetic.
#[derive(Clone, Debug)]
pub struct PlannedTask {
    pub workspace: WorkspaceId,
    pub document: DocumentId,
    pub node: String,
    /// `task:<workspace>:<document>:<node>` — the row's identity, its page
    /// cursor and its activity subject, spelled here exactly as the query
    /// spells it.
    pub reference: String,
    pub kind: &'static str,
    pub title: String,
    /// The `koru.task` marker's reason, or `None` for a task that exists only
    /// because it carries a due date.
    pub marker: Option<TaskReason>,
    pub due: Option<String>,
    pub priority: Option<Priority>,
    pub list: Option<String>,
    pub completed: bool,
    pub in_trash: bool,
    pub under_section: bool,
    pub document_trashed: bool,
    pub cold: bool,
}

impl PlannedTask {
    /// Whether the **default** tasks query answers this row: open, and not in
    /// its document's own bin. A binned *document* is flagged, never hidden, so
    /// it is not part of this.
    pub fn in_default_answer(&self) -> bool {
        !self.completed && !self.in_trash && !self.cold
    }
}

/// A dealt corpus: what will be written, before anything is.
#[derive(Clone, Debug)]
pub struct Corpus {
    pub workspaces: Vec<PlannedWorkspace>,
    /// Rows that are not tasks, across the whole corpus.
    pub other_rows: usize,
    /// What was asked for.
    pub shape: TaskCorpus,
}

impl Corpus {
    /// Every task of the plan, cold workspaces included.
    pub fn tasks(&self) -> impl Iterator<Item = &PlannedTask> {
        self.workspaces
            .iter()
            .flat_map(|w| w.documents.iter())
            .flat_map(|d| d.tasks.iter())
    }

    /// The workspaces with rows in the read model.
    pub fn warm(&self) -> impl Iterator<Item = &PlannedWorkspace> {
        self.workspaces.iter().filter(|w| !w.cold)
    }

    /// The workspaces nothing has swept.
    pub fn cold(&self) -> impl Iterator<Item = &PlannedWorkspace> {
        self.workspaces.iter().filter(|w| w.cold)
    }

    /// Write every **warm** workspace into the read model.
    ///
    /// `at_millis` is the projector's clock, passed rather than read, so two
    /// builds of one corpus produce identical `projected_at` stamps as well as
    /// identical rows.
    pub fn write_into(
        &self,
        projections: &mut Projections,
        at_millis: i64,
    ) -> std::result::Result<(), ProjectionError> {
        for workspace in self.warm() {
            self.write(projections, workspace, at_millis)?;
        }
        Ok(())
    }

    /// Write one workspace by id — **what a sweep landing looks like**, which
    /// is how a cold workspace converges: nobody edited anything, somebody
    /// asked.
    pub fn sweep_in(
        &self,
        projections: &mut Projections,
        workspace: WorkspaceId,
        at_millis: i64,
    ) -> std::result::Result<(), ProjectionError> {
        let Some(planned) = self.workspaces.iter().find(|w| w.id == workspace) else {
            return Ok(());
        };
        self.write(projections, planned, at_millis)
    }

    fn write(
        &self,
        projections: &mut Projections,
        workspace: &PlannedWorkspace,
        at_millis: i64,
    ) -> std::result::Result<(), ProjectionError> {
        let mut listed = Vec::with_capacity(workspace.documents.len());
        for document in &workspace.documents {
            projections.replace(
                &DocumentProjection {
                    workspace: workspace.id,
                    document: document.id,
                    // Deterministic, and different per document: the digest
                    // decides whether a stamp moves, so a shared revision would
                    // make every document look like the same one.
                    source_revision: document.id.to_storage().into_bytes(),
                    nodes: nodes_of(document),
                },
                at_millis,
                Some(at_millis),
            )?;
            listed.push(ListedDocument {
                document: document.id,
                parent: None,
                ordinal: document.ordinal,
                title: document.title.clone(),
                kind: "page".to_owned(),
                mime: None,
                trashed_at: document.trashed.then_some(at_millis),
                trashed_by: None,
            });
        }
        projections.set_listing(workspace.id, &workspace.name, &listed, at_millis)?;
        Ok(())
    }
}

/// One document's rows: the title row, one section, its tasks, and the rows
/// that are not tasks.
fn nodes_of(document: &PlannedDocument) -> Vec<ProjectedNode> {
    let mut nodes = Vec::with_capacity(2 + document.tasks.len() + document.other_rows);
    nodes.push(ProjectedNode {
        kind: "document".to_owned(),
        ..bare(&document.root, &document.title, None, 0, 0)
    });
    nodes.push(bare(
        &document.section,
        "Section",
        Some(&document.root),
        0,
        1,
    ));

    for (i, task) in document.tasks.iter().enumerate() {
        let (parent, depth) = if task.under_section {
            (&document.section, 2)
        } else {
            (&document.root, 1)
        };
        let mut node = bare(&task.node, &task.title, Some(parent), i + 1, depth);
        node.kind = task.kind.to_owned();
        node.in_trash = task.in_trash;
        // An image and a file carry no text of their own — the title chain
        // falls through to the authored name, which is what a leaf task's row
        // shows.
        if matches!(task.kind, "image" | "file") {
            node.name = Some(node.text.clone());
            node.text = String::new();
            node.blob = Some(format!("blob-{}", task.node));
            node.mime = Some("image/png".to_owned());
        }
        if let Some(reason) = task.marker {
            node.attrs.push(attr(
                &task.node,
                "marker",
                KEY_TASK,
                "text",
                reason.as_str(),
                None,
            ));
        }
        if let Some(due) = &task.due {
            node.attrs
                .push(attr(&task.node, "due", KEY_DUE, "date", due, None));
        }
        if let Some(priority) = task.priority {
            node.attrs.push(attr(
                &task.node,
                "priority",
                KEY_PRIORITY,
                "text",
                priority.as_str(),
                None,
            ));
        }
        if let Some(list) = &task.list {
            node.attrs
                .push(attr(&task.node, "list", KEY_LIST, "text", list, None));
        }
        if task.completed {
            node.done = true;
            node.attrs.push(attr(
                &task.node,
                "state",
                KEY_TASK_STATE,
                "text",
                Lifecycle::Completed.as_str(),
                None,
            ));
        }
        nodes.push(node);
    }

    // The rows that are not tasks. One in ten carries a tag, so the corpus
    // holds rows that are **in** the attribute query's base set — a node with
    // at least one attribute — and out of the task set, which is the pair that
    // makes the derivation predicate do work rather than nothing.
    for i in 0..document.other_rows {
        let id = format!("{}-row-{i:05}", document.root);
        let mut node = bare(
            &id,
            &format!("Line {} of {}", i + 1, document.title),
            Some(&document.root),
            document.tasks.len() + i + 1,
            1,
        );
        if i % 10 == 0 {
            node.attrs.push(attr(
                &id,
                "tag",
                KEY_TAG,
                "text",
                &format!("tag-{}", i % 4),
                None,
            ));
        }
        nodes.push(node);
    }
    nodes
}

fn bare(id: &str, text: &str, parent: Option<&str>, ordinal: usize, depth: usize) -> ProjectedNode {
    ProjectedNode {
        id: id.to_owned(),
        parent: parent.map(str::to_owned),
        ordinal,
        depth,
        kind: "paragraph".to_owned(),
        text: text.to_owned(),
        heading: None,
        list_style: None,
        done: false,
        level_name: None,
        name: None,
        icon: None,
        colour: None,
        language: None,
        blob: None,
        mime: None,
        embed: None,
        in_trash: false,
        runs: Vec::new(),
        attrs: Vec::new(),
    }
}

fn attr(
    node: &str,
    what: &str,
    role: &str,
    kind: &str,
    value: &str,
    zone: Option<&str>,
) -> ProjectedAttr {
    ProjectedAttr {
        id: format!("{node}-{what}"),
        role: role.to_owned(),
        kind: kind.to_owned(),
        value: value.to_owned(),
        zone: zone.map(str::to_owned),
        label: None,
    }
}

// ------------------------------------------------------------ the arithmetic

const LISTS_PER_WORKSPACE: usize = 4;

const KINDS: [&str; 8] = [
    "paragraph",
    "task",
    "paragraph",
    "task",
    "image",
    "file",
    "paragraph",
    "document",
];

/// The marker's three reasons and the four priorities, taken from the domain's
/// own lists rather than spelled again here — a token added there appears in
/// the corpus without this file being edited.
const REASONS: [TaskReason; 3] = TaskReason::ALL;

const PRIORITIES: [Priority; 4] = Priority::ALL;

const TAG_WORKSPACE: u64 = 1;
const TAG_DOCUMENT: u64 = 2;
const TAG_TASK: u64 = 3;
const TAG_LIST: u64 = 4;
const TAG_ROOT: u64 = 5;
const TAG_SECTION: u64 = 6;

/// SplitMix64. Sixty lines of PRNG would be sixty lines nobody can check;
/// this is one round of a published mixer, it needs no state, and the same
/// `(seed, tag, index)` gives the same number on every platform and in every
/// build — which is the only property being asked of it.
fn mix(seed: u64, tag: u64, index: u64) -> u64 {
    let mut z = seed
        .wrapping_add(tag.wrapping_mul(0x9E37_79B9_7F4A_7C15))
        .wrapping_add(index.wrapping_mul(0xBF58_476D_1CE4_E5B9));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A v4-shaped UUID string minted from the seed. **Shaped, not random**: the
/// version and variant nibbles are set so every parser and every join behaves
/// as it does on a real id.
fn uuid_from(seed: u64, tag: u64, index: u64) -> String {
    let hi = mix(seed, tag, index);
    let lo = mix(seed ^ 0xA5A5_A5A5_A5A5_A5A5, tag, index);
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        hi >> 32,
        (hi >> 16) & 0xFFFF,
        hi & 0x0FFF,
        0x8000 | ((lo >> 48) & 0x3FFF),
        lo & 0xFFFF_FFFF_FFFF,
    )
}

/// A due date, spread over one year of a fixed calendar. **No clock**: a
/// fixture whose dates moved with the day it ran would sort differently every
/// morning, and every "what is overdue" assertion on it would be about the
/// machine rather than about the query.
fn a_date(h: u64) -> String {
    let month = 1 + (h / 7) % 12;
    let day = 1 + (h / 11) % 28;
    format!("2026-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests;
