// SPDX-License-Identifier: MPL-2.0

//! The profile document: who you are, which workspaces you own, and where the
//! things you refer to live.
//!
//! ```text
//! Profile doc
//! ├── meta        LoroMap   id, name, email, avatar, account, schema_version
//! ├── library     LoroTree  profile-scoped nodes
//! ├── workspaces  LoroMap   workspace-uuid -> LoroMap { name, icon, created }
//! └── locations   LoroMap   node-uuid -> workspace-uuid
//! ```
//!
//! **A separate document, not a container inside the workspace** — ADR-3. The
//! document is the unit of sharing, so inviting somebody into one workspace must
//! not hand them the profile that lists every other one. Scoping *by* document
//! is free; scoping *inside* a shared document is a permissions system you then
//! have to enforce on every operation.
//!
//! ## Identity, and what the cloud will need
//!
//! Nothing here syncs to a server yet, and these rules exist so that it can
//! without renaming anything:
//!
//! - **ProfileId is not an account id.** A profile is a *document*; an account
//!   is *who may read it*. Store profile metadata may carry an additive
//!   koru:<identity-uuid> authorization binding, but this legacy CRDT view does
//!   not persist it. Two devices converge on one ProfileId by syncing the
//!   profile document, which is the mechanism that already exists.
//! - **The folder is a replica, not an identity.** A profile folder copied to
//!   another machine is a second replica of the same profile and the two merge.
//!   That is why the id is written into the document (and mirrored into
//!   `profile.conf`) rather than being derived from the path.
//! - **Nothing a server would see is keyed by folder, path or peer.** What a
//!   server needs is an ACL by document uuid, an op relay by document uuid,
//!   snapshots by document uuid, and presence by (document uuid, replica).
//!   Every id here is one of those.
//! - **`locations` answers *where do I look*, never *what does it contain*.**
//!   Content lives in exactly one workspace; this index only says which. See
//!   `docs/architecture/DATAMODEL.md`.
//!
//! ## Why `workspaces` is a map and not a list
//!
//! [`ProfileDoc::ensure_workspace`] runs on **every** open — adoption of a
//! folder written by an older build re-registers everything it finds. With a
//! list, two windows adopting the same folder would each append an entry and the
//! merge would keep both, because a CRDT list has no idea the two inserts meant
//! the same thing. Keying by the workspace's own uuid makes registration
//! idempotent by construction rather than by convention.
//!
//! Order comes from `created`, so it is data rather than list position — which
//! is also what lets a drag-reorder arrive later as an additive `order` field
//! with no migration.

use std::collections::HashMap;

use loro::{ExportMode, LoroDoc, LoroMap, TreeID, TreeParentId, ValueOrContainer};

use crate::ids::{NodeId, ProfileId, WorkspaceId};
use crate::node::Node;
use crate::schema::{
    C_LIBRARY, C_META, Compatibility, K_ICON, K_ID, K_NAME, K_SCHEMA, Result, StoreError, as_bool,
    as_i64, as_string, configure_styles, read_node, read_node_id, write_node,
};

// Root containers of this document. `meta` and `library` deliberately share the
// workspace's names: they are the same idea one level up, and a reader opening
// either document in a debugger should not have to learn two vocabularies.
const C_WORKSPACES: &str = "workspaces";
const C_LOCATIONS: &str = "locations";

// Profile meta keys.
const K_EMAIL: &str = "email";
/// The blob hash of the picture, when the user has set one.
const K_AVATAR: &str = "avatar";
/// When a workspace was first registered, unix milliseconds. The list's sort
/// key, and the only ordering there is.
const K_CREATED: &str = "created";

// The workspace's *identity*, beyond its name. Every one of these is optional
// and absent by default, so an entry written by a build that predates them
// reads exactly as it did before — see [`read_entry`]. That is why none of them
// moves `PROFILE_SCHEMA_VERSION`: a key an older build ignores is not a
// compatibility break, and treating it as one would make every future field a
// forced upgrade.
//
// Spelled `color`, not `colour`, per the repo's rule for new identifiers and new
// storage keys. `schema.rs` has a `K_COLOUR` for `NodeMeta.colour`, which is
// stable storage and must not move — copying that spelling here would be wrong.
/// A name from `theme::WORKSPACE_COLORS`, never a hex literal: the palette
/// resolves to different RGB in the two themes, so a stored name stays legible
/// after a theme flip and a stored colour does not.
const K_COLOR: &str = "color";
/// Sorted to the top of the switcher.
const K_PINNED: &str = "pinned";
/// Kept out of the switcher, but never out of the profile — an archived
/// workspace is one you have stopped looking at, not one you have deleted.
const K_ARCHIVED: &str = "archived";
const K_DELETED_AT: &str = "deleted_at";
/// Stable Store-backed position. Absent entries fall back to `created`.
const K_ORDER: &str = "order";

/// What this build understands. Versioned **independently of the workspace
/// document**: a build that reads workspaces at 2 and profiles at 1 is a real
/// and safe state, and one shared constant could not say so.
pub const PROFILE_SCHEMA_VERSION: i64 = 1;

/// One workspace, as the profile records it.
///
/// A cache of the workspace document's own `meta.name`, refreshed when that
/// document is opened. The entry exists so the switcher can list workspaces
/// **without opening every snapshot on disk**; it is never the truth about a
/// workspace, only about which ones this profile owns.
///
/// The same facts are also written to `workspaces/<id>/workspace.conf`, so a
/// folder that is copied or handed to someone else describes itself. **This is
/// the authority when the two disagree**: it is the only one of the pair that
/// converges between peers, because a plain file is last-writer-wins per file
/// and would silently clobber a concurrent change. The conf is authority in
/// exactly one case — adoption, where there is no entry to disagree with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceEntry {
    pub id: WorkspaceId,
    pub name: String,
    pub icon: Option<String>,
    /// A name from `theme::WORKSPACE_COLORS`. See [`K_COLOR`].
    pub color: Option<String>,
    pub pinned: bool,
    pub archived: bool,
    /// Unix milliseconds while this is in Store-managed workspace trash.
    pub deleted_at: Option<i64>,
    /// Stable position in the Store-backed workspace list.
    pub order: i64,
    /// Unix milliseconds, supplied by the caller — see [`ProfileDoc::ensure_workspace`].
    pub created: i64,
}

impl WorkspaceEntry {
    /// A workspace with a name and nothing else said about it.
    ///
    /// What a caller building a list by hand wants — chiefly tests and the
    /// switcher's fixtures. Every field this does not name has a default that
    /// means "not set", so adding a field to the struct does not break them.
    pub fn new(id: WorkspaceId, name: &str, created: i64) -> Self {
        Self {
            id,
            name: name.to_string(),
            icon: None,
            color: None,
            pinned: false,
            archived: false,
            deleted_at: None,
            order: created,
            created,
        }
    }
}

/// A profile's identity, flattened for anything that has to *show* one.
///
/// Lives here rather than in the chrome because two crates that must not see
/// each other both need it: `koru-workspace-host` reads it off the document and
/// the explore pane draws it, and neither may depend on the other. It is plain
/// data, so neither ends up holding a `ProfileDoc`.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ProfileSummary {
    pub id: ProfileId,
    pub name: String,
    /// Empty when there is none. A profile is a local record until there is
    /// something to sign in to.
    pub email: String,
    /// The blob hash of the picture, when one is set. Whoever draws it resolves
    /// the hash; nothing here decodes anything.
    pub avatar: Option<String>,
    /// Store-backed switcher appearance. Folder profiles leave these absent.
    pub icon: Option<String>,
    pub color: Option<String>,
    /// Exact Store binding (`koru:<identity-uuid>`), or `None` while local-only.
    pub account: Option<String>,
    /// Where it lives, **for display only**.
    ///
    /// Two profiles can share a name — the name comes from the folder's own,
    /// and a worktree's `Koru` beside a checkout's `Koru` gives two
    /// rows spelled identically. A switcher has to be able to tell them apart,
    /// and the path is the thing that differs.
    ///
    /// It is **not** identity and nothing may key on it: a folder moves, and a
    /// folder copied elsewhere is the same profile (ADR-39). Empty when the
    /// caller has nothing to say.
    pub folder: String,
}

/// A profile: its identity, its library, and the workspaces it owns.
pub struct ProfileDoc {
    doc: LoroDoc,
    /// `NodeId -> TreeID` for the library. No realm beside it, unlike
    /// [`crate::WorkspaceDoc`]: a profile has one tree, so a `TreeID` is
    /// unambiguous on its own.
    index: HashMap<NodeId, TreeID>,
}

impl ProfileDoc {
    pub fn create(id: ProfileId, name: &str, email: Option<&str>) -> Result<Self> {
        let doc = LoroDoc::new();
        // The profile library holds text nodes with pills exactly as a workspace
        // library does, and expansion is runtime configuration that is never
        // persisted — so it has to be set here *and* in `from_snapshot`.
        configure_styles(&doc);
        let meta = doc.get_map(C_META);
        meta.insert(K_ID, id.to_storage())?;
        meta.insert(K_NAME, name)?;
        meta.insert(K_SCHEMA, PROFILE_SCHEMA_VERSION)?;
        if let Some(email) = email {
            meta.insert(K_EMAIL, email)?;
        }
        doc.commit();
        Ok(Self {
            doc,
            index: HashMap::new(),
        })
    }

    /// **A profile view with no identity and no name** — first run (S21).
    ///
    /// The counterpart of [`create`](Self::create) for the one launch where
    /// there is nobody to create it *for*: the Store holds no profile, and the
    /// window opens to ask whether one should exist. `id()` and `name()` answer
    /// `None`, which they have always been able to, so every reader that already
    /// treats the identity as optional is already right about this one.
    ///
    /// **It is not a profile that failed to load.** Nothing is missing here —
    /// the store was asked and its answer was "none", so an unseated document is
    /// that answer carried in the shape the window already reads
    /// ([ADR-269](../../../docs/decisions/ADR-251-275.md)). The schema version is
    /// still written, because a document this build made is a document this
    /// build understands whether or not anybody is named in it.
    ///
    /// Never saved, like every other `ProfileDoc` the store path builds — see
    /// `koru_workspace::fromstore`.
    pub fn unseated() -> Result<Self> {
        let doc = LoroDoc::new();
        configure_styles(&doc);
        let meta = doc.get_map(C_META);
        meta.insert(K_SCHEMA, PROFILE_SCHEMA_VERSION)?;
        doc.commit();
        Ok(Self {
            doc,
            index: HashMap::new(),
        })
    }

    pub fn from_snapshot(bytes: &[u8]) -> Result<Self> {
        let doc = LoroDoc::new();
        configure_styles(&doc);
        doc.import(bytes)?;
        let mut profile = Self {
            doc,
            index: HashMap::new(),
        };
        profile.reindex()?;
        Ok(profile)
    }

    // ------------------------------------------------------------ identity

    pub fn id(&self) -> Option<ProfileId> {
        ProfileId::parse(&as_string(self.doc.get_map(C_META).get(K_ID)?)?)
    }

    pub fn name(&self) -> Option<String> {
        as_string(self.doc.get_map(C_META).get(K_NAME)?)
    }

    pub fn email(&self) -> Option<String> {
        as_string(self.doc.get_map(C_META).get(K_EMAIL)?)
    }

    /// The blob hash of the profile picture, if one is set. The *bytes* live in
    /// the blob store like any other image — never in the CRDT (ADR-12).
    pub fn avatar(&self) -> Option<String> {
        as_string(self.doc.get_map(C_META).get(K_AVATAR)?)
    }

    pub fn set_name(&mut self, name: &str) -> Result<()> {
        self.doc.get_map(C_META).insert(K_NAME, name)?;
        self.doc.commit();
        Ok(())
    }

    /// `None` clears it. An email is optional because a profile is a local
    /// record until there is something to sign in to.
    pub fn set_email(&mut self, email: Option<&str>) -> Result<()> {
        let meta = self.doc.get_map(C_META);
        match email {
            Some(e) => meta.insert(K_EMAIL, e)?,
            None => meta.delete(K_EMAIL)?,
        }
        self.doc.commit();
        Ok(())
    }

    pub fn set_avatar(&mut self, hash: Option<&str>) -> Result<()> {
        let meta = self.doc.get_map(C_META);
        match hash {
            Some(h) => meta.insert(K_AVATAR, h)?,
            None => meta.delete(K_AVATAR)?,
        }
        self.doc.commit();
        Ok(())
    }

    pub fn schema_version(&self) -> Option<i64> {
        as_i64(self.doc.get_map(C_META).get(K_SCHEMA)?)
    }

    /// Whether this build may write to the profile it just opened.
    ///
    /// Independent of the workspace gate. A read-only profile means the
    /// workspace *list* cannot be edited; each workspace still opens under its
    /// own version check.
    pub fn compatibility(&self) -> Compatibility {
        Compatibility::against(self.schema_version(), PROFILE_SCHEMA_VERSION)
    }

    pub fn doc(&self) -> &LoroDoc {
        &self.doc
    }

    /// Everything a switcher needs to draw this profile, and nothing else.
    pub fn summary(&self) -> ProfileSummary {
        ProfileSummary {
            id: self.id().unwrap_or_default(),
            name: self.name().unwrap_or_default(),
            email: self.email().unwrap_or_default(),
            avatar: self.avatar(),
            icon: None,
            color: None,
            account: None,
            // The document does not know where it is — a file cannot say where
            // itself lives. Whoever opened it fills this in.
            folder: String::new(),
        }
    }

    // ------------------------------------------------------------ sync

    pub fn export_snapshot(&self) -> Result<Vec<u8>> {
        Ok(self.doc.export(ExportMode::snapshot())?)
    }

    pub fn export_all_updates(&self) -> Result<Vec<u8>> {
        Ok(self.doc.export(ExportMode::all_updates())?)
    }

    pub fn export_updates_since(&self, from: &loro::VersionVector) -> Result<Vec<u8>> {
        Ok(self.doc.export(ExportMode::updates(from))?)
    }

    pub fn version(&self) -> loro::VersionVector {
        self.doc.oplog_vv()
    }

    /// Merge another replica's changes. Idempotent, and reindexes because
    /// `TreeID`s arriving from elsewhere are new to this replica's map.
    ///
    /// See [`WorkspaceDoc::import`] for why a loop around this is the wrong
    /// shape and [`import_frames`] is the right one.
    ///
    /// [`WorkspaceDoc::import`]: crate::schema::WorkspaceDoc::import
    /// [`import_frames`]: Self::import_frames
    pub fn import(&mut self, bytes: &[u8]) -> Result<()> {
        self.doc.import(bytes)?;
        self.reindex()
    }

    /// Merge many frames, reindexing once. The twin of
    /// [`WorkspaceDoc::import_frames`], and the same reasoning.
    ///
    /// [`WorkspaceDoc::import_frames`]: crate::schema::WorkspaceDoc::import_frames
    pub fn import_frames(&mut self, frames: &[Vec<u8>]) -> Result<()> {
        if frames.is_empty() {
            return Ok(());
        }
        for frame in frames {
            self.doc.import(frame)?;
        }
        self.reindex()
    }

    /// Whether this document already contains everything in `frame`. The twin of
    /// [`WorkspaceDoc::frame_is_known`].
    ///
    /// [`WorkspaceDoc::frame_is_known`]: crate::schema::WorkspaceDoc::frame_is_known
    pub fn frame_is_known(&self, frame: &[u8]) -> bool {
        match LoroDoc::decode_import_blob_meta(frame, false) {
            Ok(meta) => self.version().includes_vv(&meta.partial_end_vv),
            Err(_) => false,
        }
    }

    pub fn commit(&self) {
        self.doc.commit();
    }

    // ------------------------------------------------------------ workspaces

    fn workspaces_map(&self) -> LoroMap {
        self.doc.get_map(C_WORKSPACES)
    }

    /// Every workspace this profile owns, oldest first.
    ///
    /// Ties break on the id so the order is total and stable — two workspaces
    /// created in the same millisecond on two machines must not swap places
    /// between launches.
    pub fn workspaces(&self) -> Vec<WorkspaceEntry> {
        let map = self.workspaces_map();
        let mut out: Vec<WorkspaceEntry> = map
            .keys()
            .filter_map(|k| {
                let key = k.to_string();
                let id = WorkspaceId::parse(&key)?;
                read_entry(&map, id)
            })
            .collect();
        out.sort_by(|a, b| {
            a.order
                .cmp(&b.order)
                .then_with(|| a.created.cmp(&b.created))
                .then_with(|| a.id.to_storage().cmp(&b.id.to_storage()))
        });
        out
    }

    pub fn workspace(&self, id: WorkspaceId) -> Option<WorkspaceEntry> {
        read_entry(&self.workspaces_map(), id)
    }

    /// Register `id` if this profile does not already list it, and hand back the
    /// entry either way.
    ///
    /// **Does not rename an existing entry.** This runs on every open — adopting
    /// a folder written by an older build re-registers every workspace directory
    /// it finds — so renaming here would silently undo a rename on every launch.
    ///
    /// `created` is passed in rather than read from a clock so that this crate
    /// stays clock-free and the resulting order is exactly assertable. It is
    /// consumed only on first registration.
    pub fn ensure_workspace(
        &mut self,
        id: WorkspaceId,
        name: &str,
        created: i64,
    ) -> Result<WorkspaceEntry> {
        if let Some(existing) = self.workspace(id) {
            return Ok(existing);
        }
        // `ensure_mergeable_map`, never `get_or_create_container`: the latter is
        // deprecated and creates op-id children, so two peers registering the
        // same workspace concurrently would end up with two containers and one
        // of them would win — losing whichever name arrived second.
        let entry = self
            .workspaces_map()
            .ensure_mergeable_map(&id.to_storage())?;
        entry.insert(K_NAME, name)?;
        entry.insert(K_CREATED, created)?;
        self.doc.commit();
        Ok(WorkspaceEntry {
            id,
            name: name.to_string(),
            icon: None,
            color: None,
            pinned: false,
            archived: false,
            deleted_at: None,
            order: created,
            created,
        })
    }

    /// Mint a workspace and register it. The id is a fresh v4 UUID, minted here
    /// and never derived from the name — a workspace that is renamed is the same
    /// workspace, and two profiles may each have a "Personal".
    pub fn add_workspace(&mut self, name: &str, created: i64) -> Result<WorkspaceEntry> {
        self.ensure_workspace(WorkspaceId::new(), name, created)
    }

    pub fn rename_workspace(&mut self, id: WorkspaceId, name: &str) -> Result<()> {
        let entry = self.entry_map(id)?;
        entry.insert(K_NAME, name)?;
        self.doc.commit();
        Ok(())
    }

    pub fn set_workspace_icon(&mut self, id: WorkspaceId, icon: Option<&str>) -> Result<()> {
        let entry = self.entry_map(id)?;
        match icon {
            Some(i) => entry.insert(K_ICON, i)?,
            None => entry.delete(K_ICON)?,
        }
        self.doc.commit();
        Ok(())
    }

    pub fn set_workspace_order(&mut self, id: WorkspaceId, order: i64) -> Result<()> {
        self.entry_map(id)?.insert(K_ORDER, order)?;
        self.doc.commit();
        Ok(())
    }

    /// A name from `theme::WORKSPACE_COLORS`, or `None` to clear it.
    ///
    /// Nothing validates the name here, deliberately: this crate knows nothing
    /// about palettes, and a name it does not recognise draws as no colour
    /// rather than failing to load. That is what lets the palette grow without
    /// this file learning about it.
    pub fn set_workspace_color(&mut self, id: WorkspaceId, color: Option<&str>) -> Result<()> {
        let entry = self.entry_map(id)?;
        match color {
            Some(c) => entry.insert(K_COLOR, c)?,
            None => entry.delete(K_COLOR)?,
        }
        self.doc.commit();
        Ok(())
    }

    pub fn set_workspace_pinned(&mut self, id: WorkspaceId, pinned: bool) -> Result<()> {
        self.set_workspace_flag(id, K_PINNED, pinned)
    }

    pub fn set_workspace_archived(&mut self, id: WorkspaceId, archived: bool) -> Result<()> {
        self.set_workspace_flag(id, K_ARCHIVED, archived)
    }

    pub fn set_workspace_deleted_at(
        &mut self,
        id: WorkspaceId,
        deleted_at: Option<i64>,
    ) -> Result<()> {
        let entry = self.entry_map(id)?;
        match deleted_at {
            Some(at) => entry.insert(K_DELETED_AT, at)?,
            None => entry.delete(K_DELETED_AT)?,
        }
        self.doc.commit();
        Ok(())
    }

    /// `false` is stored as an *absent* key, not as `false`.
    ///
    /// So a workspace that was pinned and then unpinned is byte-identical to one
    /// that never was, and the default in [`read_entry`] is the only place the
    /// meaning of "missing" is written down.
    fn set_workspace_flag(&mut self, id: WorkspaceId, key: &str, on: bool) -> Result<()> {
        let entry = self.entry_map(id)?;
        if on {
            entry.insert(key, true)?;
        } else {
            entry.delete(key)?;
        }
        self.doc.commit();
        Ok(())
    }

    /// Remove the **entry**, never the folder.
    ///
    /// Named `forget` for exactly that reason: a workspace dropped from the list
    /// is a workspace this profile stops showing, and every byte of it is still
    /// on disk. Deleting content is not something a list can do by accident.
    pub fn forget_workspace(&mut self, id: WorkspaceId) -> Result<()> {
        self.workspaces_map().delete(&id.to_storage())?;
        self.doc.commit();
        Ok(())
    }

    fn entry_map(&self, id: WorkspaceId) -> Result<LoroMap> {
        let Some(ValueOrContainer::Container(c)) = self.workspaces_map().get(&id.to_storage())
        else {
            return Err(StoreError::Malformed("no such workspace in this profile"));
        };
        c.into_map()
            .map_err(|_| StoreError::Malformed("workspace entry is not a map"))
    }

    // ------------------------------------------------------------ locations

    fn locations_map(&self) -> LoroMap {
        self.doc.get_map(C_LOCATIONS)
    }

    /// Which workspace holds `node`, as far as this profile knows.
    ///
    /// A hint for *finding* a workspace, never a claim about what it contains —
    /// content lives in exactly one workspace and a reference crosses the
    /// boundary that content does not.
    pub fn locate(&self, node: NodeId) -> Option<WorkspaceId> {
        WorkspaceId::parse(&as_string(self.locations_map().get(&node.to_storage())?)?)
    }

    pub fn record_location(&mut self, node: NodeId, workspace: WorkspaceId) -> Result<()> {
        self.locations_map()
            .insert(&node.to_storage(), workspace.to_storage())?;
        self.doc.commit();
        Ok(())
    }

    pub fn forget_location(&mut self, node: NodeId) -> Result<()> {
        self.locations_map().delete(&node.to_storage())?;
        self.doc.commit();
        Ok(())
    }

    // ------------------------------------------------------------ library

    fn library(&self) -> loro::LoroTree {
        self.doc.get_tree(C_LIBRARY)
    }

    /// Add a node to the profile library — the third home a node can have, above
    /// a task and above a workspace. Flat: the profile library lists things, it
    /// does not nest them.
    pub fn insert_library(&mut self, node: Node) -> Result<NodeId> {
        let tree = self.library();
        let tid = tree.create(TreeParentId::Root)?;
        write_node(&tree.get_meta(tid)?, &node)?;
        self.doc.commit();
        self.index.insert(node.id, tid);
        Ok(node.id)
    }

    pub fn library_children(&self) -> Result<Vec<NodeId>> {
        let tree = self.library();
        Ok(tree
            .children(TreeParentId::Root)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|tid| tree.get_meta(tid).ok().and_then(|m| read_node_id(&m)))
            .collect())
    }

    pub fn read(&self, id: NodeId) -> Result<Node> {
        let tid = self
            .index
            .get(&id)
            .copied()
            .ok_or(StoreError::NoSuchNode(id))?;
        read_node(&self.library().get_meta(tid)?)
    }

    /// Rebuild `NodeId -> TreeID`. Loro's tree ids are only meaningful within the
    /// replica that minted them, so this runs after every import.
    fn reindex(&mut self) -> Result<()> {
        self.index.clear();
        let tree = self.library();
        for tid in tree.nodes() {
            if tree.is_node_deleted(&tid).unwrap_or(false) {
                continue;
            }
            if let Some(id) = read_node_id(&tree.get_meta(tid)?) {
                self.index.insert(id, tid);
            }
        }
        Ok(())
    }
}

fn read_entry(map: &LoroMap, id: WorkspaceId) -> Option<WorkspaceEntry> {
    let ValueOrContainer::Container(c) = map.get(&id.to_storage())? else {
        return None;
    };
    let entry = c.into_map().ok()?;
    // Every field but the id defaults, so an entry written before any of these
    // keys existed reads as one that simply has none of them set. That is the
    // whole of the migration, and it is why adding a key here is free.
    let created = entry.get(K_CREATED).and_then(as_i64).unwrap_or(0);
    Some(WorkspaceEntry {
        id,
        name: entry.get(K_NAME).and_then(as_string).unwrap_or_default(),
        icon: entry.get(K_ICON).and_then(as_string),
        color: entry.get(K_COLOR).and_then(as_string),
        pinned: entry.get(K_PINNED).and_then(as_bool).unwrap_or(false),
        archived: entry.get(K_ARCHIVED).and_then(as_bool).unwrap_or(false),
        deleted_at: entry.get(K_DELETED_AT).and_then(as_i64),
        order: entry.get(K_ORDER).and_then(as_i64).unwrap_or(created),
        created,
    })
}

#[cfg(test)]
mod tests;
