// SPDX-License-Identifier: MPL-2.0

//! What is in a folder, before anything is opened.
//!
//! Three questions get asked before there is a window: is this folder a profile,
//! whose is it, and what workspaces are in it. All three have to be answerable
//! **without decoding a CRDT**, because they are asked by the folder picker, by
//! `--where`, and by the profile picker — none of which has a document open, and
//! one of which runs with no window at all.
//!
//! So a profile folder describes itself in plain text. `profile.conf` is a cache
//! with exactly the status [`koru_core::Ref`]'s `home` has: the document wins on
//! disagreement and the file is rewritten from it. What it buys is that a lost
//! registry costs nothing — pointing Koru at the folder again brings back the
//! same id, name and email, so re-picking cannot accidentally mint a *second*
//! profile over the same content.
//!
//! ## And a workspace folder describes itself too
//!
//! `workspaces/<id>/workspace.conf` is the same sentence one level down, with
//! the same status: the profile document wins on disagreement, and the file is
//! rewritten from it. What *it* buys is different, and worth spelling out.
//!
//! A workspace's name, icon and colour live in the profile document, which is
//! the only copy that converges between peers. But a workspace folder is a thing
//! people move: onto a stick, into a shared drive, into somebody else's profile.
//! Arriving there, it has no entry — so without the conf it would be adopted as
//! an untitled folder, and its identity would exist only in a profile it is no
//! longer part of. The conf is what makes the folder self-sufficient, and it is
//! authority in that one case: **adoption, where there is no entry to disagree
//! with.**
//!
//! The second thing it buys is speed, and this one is measurable.
//! [`peek_workspace_name`] used to read the whole `snapshot.loro` and decode it
//! to get at one map key — 1.1 ms to decode and **17.7 ms to reindex** on a real
//! workspace, ~94% of it thrown away, once per folder on every adoption. A
//! two-hundred-byte conf answers the same question and several more.
//!
//! ## Adoption, not migration
//!
//! A folder written by a build that predates profiles has `workspaces/` and
//! nothing else. It is **adopted**: a profile document is created beside the
//! workspaces and every one of them is registered. Nothing moves, nothing is
//! rewritten, and no byte inside `workspaces/` is touched — which is what lets
//! the old build and this one take turns over the same folder while the change
//! is in flight.

use std::fs;
use std::path::{Path, PathBuf};

use koru_core::schema::WorkspaceDoc;
use koru_core::{ProfileId, WorkspaceId};

/// The plain-text sidecar. Beside `profile/`, not inside it, so it survives a
/// quarantine of the document directory.
pub const PROFILE_FILE: &str = "profile.conf";

/// What a folder turned out to be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FolderKind {
    /// A profile lives here. Carries what the sidecar said, when it could be
    /// read — the document is still the authority.
    Profile {
        id: Option<ProfileId>,
        name: Option<String>,
        email: Option<String>,
    },
    /// `workspaces/` is here but no profile is: written by a build from before
    /// profiles existed.
    Legacy,
    /// Nothing of ours. A new profile goes here, and the caller confirms that
    /// with the user first.
    Empty,
}

impl FolderKind {
    /// Whether Koru has anything in this folder — what the first-run dialogs ask
    /// before they let an empty folder be chosen silently.
    pub fn holds_content(&self) -> bool {
        !matches!(self, Self::Empty)
    }
}

/// Classify a folder. Never fails: an unreadable folder is [`FolderKind::Empty`]
/// as far as *this* is concerned, and the failure surfaces on the open attempt
/// where there is somewhere to report it.
pub fn inspect(dir: &Path) -> FolderKind {
    let sidecar = fs::read_to_string(dir.join(PROFILE_FILE))
        .ok()
        .as_deref()
        .and_then(parse_profile_conf);
    // The document is what makes it a profile; the sidecar only describes one.
    // Either alone is enough to recognise the folder, because a person deleting
    // `profile.conf` has not stopped owning the profile.
    let has_doc = dir.join("profile").join("snapshot.loro").exists();
    if has_doc || sidecar.is_some() {
        let (id, name, email) = match sidecar {
            Some((id, name, email)) => (Some(id), Some(name), email),
            None => (None, None, None),
        };
        return FolderKind::Profile { id, name, email };
    }
    if !workspace_dirs(dir).is_empty() {
        return FolderKind::Legacy;
    }
    FolderKind::Empty
}

/// The body of `profile.conf`. Same tolerant `key = value` stance as the rest
/// of the conf files, and it explains itself for the same reason.
pub fn profile_conf_body(id: ProfileId, name: &str, email: Option<&str>) -> String {
    format!(
        "# This folder is a Koru profile. Koru wrote this file; it is safe to read.\n\
         # Losing it is survivable - profile/snapshot.loro is the truth, and this is\n\
         # rewritten from it the next time the profile is opened.\n\
         id    = {}\n\
         name  = {name}\n\
         email = {}\n",
        id.to_storage(),
        email.unwrap_or(""),
    )
}

/// Read `profile.conf`. `None` unless it names a parseable id — a file that
/// cannot say which profile this is says nothing useful at all.
pub fn parse_profile_conf(text: &str) -> Option<(ProfileId, String, Option<String>)> {
    let mut id = None;
    let mut name = String::new();
    let mut email = None;
    for line in text.trim_start_matches('\u{feff}').lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "id" => id = ProfileId::parse(value),
            "name" => name = value.to_string(),
            "email" if !value.is_empty() => email = Some(value.to_string()),
            _ => {}
        }
    }
    Some((id?, name, email))
}

/// Every `workspaces/<uuid>/` in the folder.
///
/// Ids only, and never fails: a directory whose name is not a uuid is something
/// else's and is skipped. This is what adoption walks, so it has to be
/// unshakeable — a stray folder must not be able to stop a launch.
pub fn workspace_dirs(dir: &Path) -> Vec<WorkspaceId> {
    let Ok(entries) = fs::read_dir(dir.join("workspaces")) else {
        return Vec::new();
    };
    let mut out: Vec<WorkspaceId> = entries
        .filter_map(|e| {
            let e = e.ok()?;
            if !e.file_type().ok()?.is_dir() {
                return None;
            }
            WorkspaceId::parse(&e.file_name().to_string_lossy())
        })
        .collect();
    // Stable, so adoption registers them in the same order on every machine.
    out.sort_by_key(|a| a.to_storage());
    out
}

/// A workspace's display name.
///
/// The conf first, the snapshot only if it has none — so a folder written before
/// the sidecar existed still answers, and one written since costs a small read
/// instead of a decode and a reindex.
///
/// `None` for anything that will not decode, deliberately: adoption calls this
/// for every folder it finds, and one damaged snapshot must not stop the other
/// nine being registered. The damaged one is still adopted — under a fallback
/// name — and its own recovery happens when it is opened.
pub fn peek_workspace_name(dir: &Path, id: WorkspaceId) -> Option<String> {
    if let Some(facts) = peek_workspace(dir, id) {
        if !facts.name.is_empty() {
            return Some(facts.name);
        }
    }
    let bytes = fs::read(workspace_dir(dir, id).join("snapshot.loro")).ok()?;
    WorkspaceDoc::from_snapshot(&bytes).ok()?.name()
}

/// Where `profile.conf` goes.
pub fn profile_conf_path(dir: &Path) -> PathBuf {
    dir.join(PROFILE_FILE)
}

/// What a folder somebody picked turned out to be, for **import**.
///
/// Import is the one action that can *create* a duplicate workspace id, which
/// is the thing [ADR-39] exists to prevent — so the answer is an enum a caller
/// cannot ignore rather than an `Option` it can `unwrap_or_default`.
///
/// [ADR-39]: ../../../docs/decisions/README.md
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Importable {
    /// A workspace directory: it holds a `snapshot.loro`, and its id is either
    /// its folder name or what its `workspace.conf` says.
    Workspace { id: WorkspaceId, name: String },
    /// It looks like a workspace but this profile already lists that id.
    /// **Refused**, because the alternative is two entries for one identity.
    AlreadyHere { name: String },
    /// Nothing of ours.
    NotAWorkspace,
}

/// Classify a folder a person pointed at, against the profile that would take it.
///
/// The id comes from the folder's own `workspace.conf` when it has one, and from
/// the folder's *name* otherwise — which is what a workspace directory is named
/// after. A folder that is neither is not importable; minting a fresh id for it
/// would turn "import" into "adopt these bytes under a new identity", and two
/// people importing the same folder would then have two workspaces that never
/// converge.
pub fn inspect_import(folder: &Path, profile_dir: &Path) -> Importable {
    if !folder.join("snapshot.loro").exists() {
        return Importable::NotAWorkspace;
    }
    let by_name = folder
        .file_name()
        .and_then(|n| WorkspaceId::parse(&n.to_string_lossy()));
    let conf = fs::read_to_string(folder.join(WORKSPACE_FILE))
        .ok()
        .as_deref()
        .and_then(parse_workspace_conf);
    let Some(id) = conf.as_ref().map(|f| f.id).or(by_name) else {
        return Importable::NotAWorkspace;
    };
    let name = conf
        .map(|f| f.name)
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Workspace".to_string());
    if workspace_dir(profile_dir, id).exists() {
        return Importable::AlreadyHere { name };
    }
    Importable::Workspace { id, name }
}

// ------------------------------------------------------------ workspace.conf

/// The workspace's own sidecar, beside its `snapshot.loro`.
///
/// **Beside the document, not inside a directory the store manages.** Quarantine
/// moves aside anything that would not decode, and the file that says what a
/// workspace *is* must not be able to go with it.
pub const WORKSPACE_FILE: &str = "workspace.conf";

/// What a workspace folder says about itself.
///
/// Everything but `id` is optional, and a field this build does not recognise is
/// skipped rather than refused — the same tolerance every other conf here has,
/// for the same reason: a file written by a newer build must still be readable
/// enough to identify the folder.
///
/// **No `Default`**, deliberately. `WorkspaceId`'s own `Default` mints a fresh
/// v4 UUID, so a derived one here would hand out a struct carrying a plausible
/// identity that names nothing — precisely the failure ADR-39 exists to prevent.
/// Every one of these is built around an id its caller already has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceFacts {
    pub id: WorkspaceId,
    pub name: String,
    pub icon: Option<String>,
    /// A palette *name*, never a hex literal — see `koru_core::profile`.
    pub color: Option<String>,
    pub pinned: bool,
    pub archived: bool,
    /// How many documents it held when it was last closed. A hint for a list
    /// that would otherwise have to decode a snapshot to say anything; `None`
    /// renders as a dash, never as zero.
    pub docs: Option<i64>,
}

impl WorkspaceFacts {
    /// A workspace that has said nothing about itself but its id and name.
    pub fn new(id: WorkspaceId, name: &str) -> Self {
        Self {
            id,
            name: name.to_string(),
            icon: None,
            color: None,
            pinned: false,
            archived: false,
            docs: None,
        }
    }
}

/// The body of `workspace.conf`.
///
/// Absent rather than empty for everything optional, so a workspace that has
/// never had an icon and one whose icon was cleared produce the same file.
pub fn workspace_conf_body(facts: &WorkspaceFacts) -> String {
    let mut s = String::from(
        "# This folder is a Koru workspace. Koru wrote this file; it is safe to read.\n\
         # Losing it is survivable - snapshot.loro is the content, and this is rewritten\n\
         # from the profile the next time the workspace is opened. It exists so that a\n\
         # workspace folder moved somewhere else still knows its own name.\n",
    );
    s.push_str(&format!("id   = {}\n", facts.id.to_storage()));
    s.push_str(&format!("name = {}\n", facts.name));
    if let Some(icon) = &facts.icon {
        s.push_str(&format!("icon = {icon}\n"));
    }
    if let Some(color) = &facts.color {
        s.push_str(&format!("color = {color}\n"));
    }
    if facts.pinned {
        s.push_str("pinned = true\n");
    }
    if facts.archived {
        s.push_str("archived = true\n");
    }
    if let Some(docs) = facts.docs {
        s.push_str(&format!("docs = {docs}\n"));
    }
    s
}

/// Read `workspace.conf`. `None` unless it names a parseable id — a file that
/// cannot say which workspace it describes says nothing useful at all.
pub fn parse_workspace_conf(text: &str) -> Option<WorkspaceFacts> {
    let mut id = None;
    let mut name = String::new();
    let mut icon = None;
    let mut color = None;
    let mut pinned = false;
    let mut archived = false;
    let mut docs = None;
    for line in text.trim_start_matches('\u{feff}').lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "id" => id = WorkspaceId::parse(value),
            "name" => name = value.to_string(),
            "icon" if !value.is_empty() => icon = Some(value.to_string()),
            "color" if !value.is_empty() => color = Some(value.to_string()),
            "pinned" => pinned = value == "true",
            "archived" => archived = value == "true",
            "docs" => docs = value.parse().ok(),
            _ => {}
        }
    }
    // The id last, and by `?`: everything above is a description of a workspace
    // and this is the workspace it describes. Without it there is nothing to
    // attach the description to.
    Some(WorkspaceFacts {
        id: id?,
        name,
        icon,
        color,
        pinned,
        archived,
        docs,
    })
}

/// Where a workspace's folder is.
pub fn workspace_dir(dir: &Path, id: WorkspaceId) -> PathBuf {
    dir.join("workspaces").join(id.to_storage())
}

/// Where `workspace.conf` goes.
pub fn workspace_conf_path(dir: &Path, id: WorkspaceId) -> PathBuf {
    workspace_dir(dir, id).join(WORKSPACE_FILE)
}

/// What the folder says about itself, if it says anything.
///
/// Never fails: a missing or unreadable conf is a folder that has not described
/// itself yet, which is an ordinary state and not an error.
pub fn peek_workspace(dir: &Path, id: WorkspaceId) -> Option<WorkspaceFacts> {
    let text = fs::read_to_string(workspace_conf_path(dir, id)).ok()?;
    let facts = parse_workspace_conf(&text)?;
    // A conf naming a *different* workspace describes some other folder that was
    // copied here whole. Its id is the one thing about it that cannot be
    // reconciled, so the file is ignored rather than believed.
    (facts.id == id).then_some(facts)
}

/// Write `workspace.conf`, best effort.
///
/// The return is `io::Result` so a caller that has somewhere to report can, but
/// nothing is *required* to: failing to write a cache must never fail the
/// operation that prompted it — a rename that succeeded in the document has
/// succeeded, whatever the sidecar did.
pub fn write_workspace_conf(dir: &Path, facts: &WorkspaceFacts) -> std::io::Result<()> {
    fs::write(
        workspace_conf_path(dir, facts.id),
        workspace_conf_body(facts),
    )
}

#[cfg(test)]
mod tests;
