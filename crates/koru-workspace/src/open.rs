// SPDX-License-Identifier: MPL-2.0

//! Opening a profile folder: what is in it, what has to be adopted, and — for a
//! genuinely empty one — what gets seeded.
//!
//! This is the only caller of [`crate::seed`], and the only place a profile
//! document is created. Everything about *when* a profile is new lives here, so
//! that the seed itself can be a function that takes two documents.
//!
//! ## Adoption
//!
//! A folder written by a build that predates profiles has `workspaces/` and
//! nothing else. It gets a profile document beside them and every workspace it
//! finds is registered. **Nothing inside `workspaces/` is read for adoption
//! except to ask a snapshot its name, and nothing inside it is written at all.**
//! That is what lets the old build and this one take turns over one folder while
//! the change is in flight, and it is pinned by a byte comparison.
//!
//! ## The seeding rule, and why it is so heavy
//!
//! [ADR-21](../../../docs/decisions/README.md) records a seed that did not tell the
//! session it had happened: the fresh workspace was never published, so two
//! peers opening it each seeded their own and both survived the merge. So here
//! the seed is written **and published, synchronously, before this function
//! returns** — before any window exists. A second instance opening the same
//! folder a moment later finds `Opened::Normal` and seeds nothing, because the
//! bytes are already there rather than merely intended.
//!
//! A failure to write them is fatal, not a warning. A profile whose snapshot did
//! not land would look fresh again on the next launch and seed a second Example
//! beside the first.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use koru_core::profile::{ProfileDoc, WorkspaceEntry};
use koru_core::schema::{Parent, WorkspaceDoc};
use koru_core::{Kind, ProfileId, WorkspaceId};
use koru_docstore::Opened;
use koru_docstore::profilefolder::{self, FolderKind};

use crate::profile::ProfileSession;
use crate::seed;
use crate::session::{Instance, OpenError, Report, Session};

/// Everything a launch needs, once the folder has been sorted out.
pub struct OpenedProfile {
    pub profile: ProfileDoc,
    pub profile_session: ProfileSession,
    /// How the profile document itself opened — a recovery here is about the
    /// *list* of workspaces, and is announced separately from the workspace's.
    pub profile_report: Report,

    pub workspace: WorkspaceDoc,
    pub session: Session,
    pub workspace_report: Report,
    /// Which workspace is open.
    pub current: WorkspaceId,

    /// How many workspaces were adopted from a folder written by an older
    /// build. Non-zero exactly once per folder, and worth telling the user
    /// about — a silent structural change to somebody's data directory is the
    /// kind of thing that gets noticed a week later and misread as damage.
    pub adopted: usize,
    /// Whether this launch created the profile and its first document.
    pub seeded: bool,
}

/// Unix milliseconds. The one clock in this crate, kept here rather than in
/// `koru-core` so the model stays testable without one.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Open the profile in `dir`, and one of its workspaces.
///
/// `wanted` is the workspace to open — from `--workspace`, or the one this
/// window had open last. A workspace that is not in this profile's list is
/// ignored rather than obeyed: it names content in a folder that is not this
/// one, and opening it would create an empty workspace directory here under
/// somebody else's id.
pub fn open_profile(
    dir: &Path,
    instance: &Instance,
    wanted: Option<WorkspaceId>,
) -> Result<OpenedProfile, OpenError> {
    let kind = profilefolder::inspect(dir);

    // What to call the profile if this launch has to create it. Read from the
    // sidecar when there is one; otherwise the folder's own name, because that
    // is what people actually name these ("Koru/Work"), and because guessing
    // something cleverer would be guessing.
    let (id, name, email) = match &kind {
        FolderKind::Profile { id, name, email } => (
            id.unwrap_or_else(ProfileId::new),
            name.clone().unwrap_or_else(|| folder_name(dir)),
            email.clone(),
        ),
        FolderKind::Legacy | FolderKind::Empty => (ProfileId::new(), folder_name(dir), None),
    };

    let span = koru_core::timing::Span::start("profile doc");
    let (mut profile_session, mut profile, profile_report, opened) =
        ProfileSession::open(dir, id, &name, email.as_deref(), &instance.peer)?;
    span.stop();

    // Adoption: register every workspace directory already here. Idempotent, so
    // it is safe on every open — `ensure_workspace` never renames — and it is
    // what turns a folder from an older build into a profile without moving a
    // byte of it.
    let mut adopted = 0;
    if matches!(kind, FolderKind::Legacy) {
        for ws_id in profilefolder::workspace_dirs(dir) {
            if profile.workspace(ws_id).is_some() {
                continue;
            }
            // **The conf is authority here, and only here.** There is no entry
            // to disagree with — that is what makes this folder an adoption —
            // so whatever it says about itself is the best information there
            // is. A folder that came from another profile keeps the name, icon
            // and colour it had there rather than arriving anonymous.
            let facts = adopt_facts(dir, ws_id);
            let ws_name = facts
                .as_ref()
                .map(|f| f.name.clone())
                .filter(|n| !n.is_empty())
                .or_else(|| profilefolder::peek_workspace_name(dir, ws_id))
                // A snapshot that will not decode still gets adopted: its own
                // recovery happens when it is opened, and leaving it out of the
                // list would be the one way to make it unreachable.
                .unwrap_or_else(|| "Workspace".to_string());
            profile
                .ensure_workspace(ws_id, &ws_name, now_ms())
                .map_err(|e| OpenError::Storage(e.to_string()))?;
            if let Some(facts) = facts {
                // Carried across one field at a time rather than by replacing
                // the entry, because `ensure_workspace` owns `created` — the
                // date this profile first saw it, which is not the date the
                // folder was made and must not be overwritten by the conf.
                let set = profile
                    .set_workspace_icon(ws_id, facts.icon.as_deref())
                    .and_then(|_| profile.set_workspace_color(ws_id, facts.color.as_deref()))
                    .and_then(|_| profile.set_workspace_pinned(ws_id, facts.pinned))
                    .and_then(|_| profile.set_workspace_archived(ws_id, facts.archived));
                set.map_err(|e| OpenError::Storage(e.to_string()))?;
            }
            adopted += 1;
        }
    }

    // Which workspace to open. A `wanted` that this profile does not list is
    // ignored — see the doc comment.
    //
    // **`wanted` passes even when it is archived, and the fallback does not.**
    // Not a hedge: archiving is "stop showing me this in the switcher", and if
    // it also meant "and move me somewhere else on restart", archiving the
    // workspace you are looking at would teleport you. What archiving must not
    // do is *choose* a workspace for a launch that has no preference.
    //
    // And the last `or_else` is the safety net. Archiving the last unarchived
    // workspace is refused by the UI *twice*, but a peer could sync that state
    // in and somebody could hand-edit it — and without this line, "every
    // workspace is archived" falls through to the `None` arm below and **mints
    // a fresh empty workspace named Example**, which reads exactly like the app
    // having lost everything. Opening an archived workspace is a far smaller
    // surprise than that, and it is visibly recoverable.
    let existing = wanted
        .filter(|id| profile.workspace(*id).is_some())
        .or_else(|| {
            profile
                .workspaces()
                .into_iter()
                .find(|w| !w.archived)
                .map(|w| w.id)
        })
        .or_else(|| profile.workspaces().first().map(|w| w.id));

    let (ws_id, ws_name, seeding) = match existing {
        Some(id) => {
            let entry_name = profile
                .workspace(id)
                .map(|w| w.name)
                .unwrap_or_else(|| "Workspace".into());
            (id, entry_name, false)
        }
        // No workspaces at all. Either a genuine first run, or a profile that
        // came back from a recovery with an empty list — both need one, and
        // only the first gets the seeded document.
        None => (WorkspaceId::new(), "Example".to_string(), true),
    };

    let span = koru_core::timing::Span::start("workspace doc");
    let (mut session, mut workspace, workspace_report) =
        Session::open(dir, ws_id, &ws_name, &instance.peer)?;
    span.stop_with(|| ws_name.clone());
    // The profile entry is the authority for the name; the document catches up
    // here. See the same reconcile in `Workspace::switch_to` — this is the other
    // door into an already-existing workspace.
    if !seeding && workspace.name().as_deref() != Some(ws_name.as_str()) {
        let _ = workspace.rename(&ws_name);
    }

    let mut seeded = false;
    if seeding {
        if matches!(opened, Opened::Fresh) {
            seeded = seed::seed_new_profile(&mut profile, &mut workspace, now_ms())
                .map_err(|e| OpenError::Storage(e.to_string()))?;
        }
        if !seeded {
            // Recovered with an empty list: it needs a workspace but not the
            // welcome copy, which would read as the app having forgotten
            // everything rather than having recovered it.
            profile
                .ensure_workspace(ws_id, &ws_name, now_ms())
                .map_err(|e| OpenError::Storage(e.to_string()))?;
        }
    }

    // Whatever the list gained — adopted, seeded or repaired — is on disk and
    // published **before this function returns**. See the module docs: a second
    // instance a moment behind must find `Opened::Normal`, and that has to be
    // true of the bytes, not of the intention.
    if adopted > 0 || seeding {
        profile_session.flush(&profile).map_err(|e| {
            OpenError::Storage(format!(
                "the profile's workspace list could not be written: {e}"
            ))
        })?;
    }
    // The workspace is written only when this launch **made** it. Adoption must
    // not, and that is not a nicety: a save is a full re-export, and rewriting a
    // workspace that an older build is still opening is the one thing that would
    // stop the two taking turns over one folder. Pinned by the byte comparison
    // in `a_legacy_workspaces_folder_is_adopted_without_moving_anything`, which
    // caught this the first time it was written the other way round.
    if seeding {
        session.save(&workspace);
    }

    // The sidecar is a cache of the document, so it is rewritten from it — and
    // only ever from it. Best-effort: a folder that will not take this file is
    // still a perfectly good profile, it just cannot introduce itself to the
    // picker without being opened.
    write_sidecar(dir, &profile);
    // Anything deleted long enough ago really goes now. On open rather than on
    // a timer: it is a directory walk, and AGENTS.md's rule about the idle loop
    // is that anything put there is paid for as long as the app is running.
    koru_docstore::trash::prune_trash(dir, std::time::SystemTime::now());
    // And the one workspace this launch opened describes itself too.
    //
    // **Only that one, and not on the launch that adopted the folder.** Two
    // separate restraints, and the second is the load-bearing one: ADR-35 says
    // adoption never writes a byte inside `workspaces/`, and that has to stay
    // literally true rather than nearly true. A sidecar is not a document and an
    // older build would ignore it — but "we only added a *small* file" is how a
    // guarantee stops being one, and the byte comparison in
    // `a_legacy_workspaces_folder_is_adopted_without_moving_anything` is there
    // to make that argument unavailable. An adopted folder gains its conf on the
    // *next* open, by which point this profile has plainly claimed it.
    //
    // The first restraint is milder: only the open workspace, because writing a
    // conf into every folder in the list would touch workspaces this launch has
    // no business writing to. The others gain theirs when they are next opened,
    // and until then the entry is the only copy — the ordinary case, and free.
    if adopted == 0 {
        if let Some(entry) = profile.workspace(ws_id) {
            write_workspace_sidecar(dir, &entry, document_count(&workspace));
        }
    }

    Ok(OpenedProfile {
        profile,
        profile_session,
        profile_report,
        workspace,
        session,
        workspace_report,
        current: ws_id,
        adopted,
        seeded,
    })
}

/// Rewrite `profile.conf` from the document, when the two disagree.
///
/// Only when they disagree, so an ordinary launch does not touch the file — and
/// only *from* the document, because the sidecar is a cache with exactly the
/// status a `Ref`'s `home` has.
pub fn write_sidecar(dir: &Path, profile: &ProfileDoc) {
    let Some(id) = profile.id() else { return };
    let name = profile.name().unwrap_or_default();
    let email = profile.email();
    let want = profilefolder::profile_conf_body(id, &name, email.as_deref());
    let path = profilefolder::profile_conf_path(dir);
    if std::fs::read_to_string(&path).is_ok_and(|have| have == want) {
        return;
    }
    if let Err(e) = std::fs::write(&path, want) {
        eprintln!("warning: could not write {} ({e})", path.display());
    }
}

/// Rewrite one workspace's `workspace.conf` from the profile entry.
///
/// The same shape and the same status as [`write_sidecar`]: written only from
/// the document, skipped when the file already says it, and best-effort. What is
/// different is what it is *for* — a profile's sidecar exists so a folder can be
/// re-picked without minting a second profile, and a workspace's exists so a
/// folder that is **moved out of this profile entirely** still knows its own
/// name, icon and colour when it lands somewhere else.
///
/// `docs` is threaded through rather than recomputed, because counting them
/// means walking the library and this is called on every rename.
pub fn write_workspace_sidecar(dir: &Path, entry: &WorkspaceEntry, docs: Option<i64>) {
    let facts = profilefolder::WorkspaceFacts {
        id: entry.id,
        name: entry.name.clone(),
        icon: entry.icon.clone(),
        color: entry.color.clone(),
        pinned: entry.pinned,
        archived: entry.archived,
        docs,
    };
    let want = profilefolder::workspace_conf_body(&facts);
    let path = profilefolder::workspace_conf_path(dir, entry.id);
    if std::fs::read_to_string(&path).is_ok_and(|have| have == want) {
        return;
    }
    // A workspace whose directory does not exist yet is not an error to report:
    // `create_workspace` writes the conf after `Session::open` has made the
    // folder, and every other caller is acting on one that is already there.
    if path.parent().is_some_and(|p| p.exists()) {
        if let Err(e) = std::fs::write(&path, want) {
            eprintln!("warning: could not write {} ({e})", path.display());
        }
    }
}

/// How many documents a workspace holds, for the sidecar to remember.
///
/// `kind_of`, not `read`: the latter materialises each node's text to answer a
/// question about one key of its meta map, and this runs on every open. `None`
/// when the library will not enumerate — a count that is missing renders as a
/// dash, and a count that is wrong is worse than one that is absent.
pub fn document_count(ws: &WorkspaceDoc) -> Option<i64> {
    let children = ws.children(Parent::Root).ok()?;
    Some(
        children
            .into_iter()
            .filter(|&id| ws.kind_of(id) == Some(Kind::Document))
            .count() as i64,
    )
}

/// What the conf already claims about a workspace, if anything.
///
/// Adoption's authority — and **only** adoption's. Once the profile lists a
/// workspace, the entry is the truth and this file is a cache of it; the two can
/// only disagree when a folder arrives from somewhere else, and that is exactly
/// the case where there is no entry to prefer.
pub fn adopt_facts(dir: &Path, id: WorkspaceId) -> Option<profilefolder::WorkspaceFacts> {
    profilefolder::peek_workspace(dir, id)
}

/// Add a workspace to an open profile and put its directory on disk.
///
/// Returns the entry, having already written the list — the switcher is about
/// to show a row for it, and a row for a workspace that is not on disk is a row
/// that fails when clicked.
pub fn create_workspace(
    dir: &Path,
    profile: &mut ProfileDoc,
    profile_session: &mut ProfileSession,
    instance: &Instance,
    name: &str,
) -> Result<WorkspaceId, OpenError> {
    let entry = profile
        .add_workspace(name, now_ms())
        .map_err(|e| OpenError::Storage(e.to_string()))?;

    // Create the directory and its first snapshot now, rather than on the first
    // save: a workspace listed but absent is exactly the state that makes a
    // switcher row lie.
    let (mut session, doc, _) = Session::open(dir, entry.id, name, &instance.peer)?;
    session.save(&doc);
    drop(session);

    // After the session, because the directory has to exist for the file to go
    // in it — and the empty document counts as zero documents, not as unknown.
    write_workspace_sidecar(dir, &entry, Some(0));

    profile_session
        .flush(profile)
        .map_err(|e| OpenError::Storage(e.to_string()))?;
    Ok(entry.id)
}

/// The folder's own name, as a profile name. `Koru` when there is nothing to
/// read — a name is cosmetic and editable, and an empty one reads as broken.
fn folder_name(dir: &Path) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "Koru".to_string())
}

/// Open a profile document just far enough to describe it — no workspace, no
/// session, no writes.
///
/// What `--where` and the profile picker use. A folder is allowed to be
/// unreadable here: the answer is `None`, and the failure is reported properly
/// on the open attempt, where there is somewhere to report it.
pub fn describe(dir: &Path) -> Option<(ProfileId, String, Option<String>)> {
    if let FolderKind::Profile {
        id: Some(id),
        name,
        email,
    } = profilefolder::inspect(dir)
    {
        return Some((id, name.unwrap_or_else(|| folder_name(dir)), email));
    }
    // No sidecar. Decode the document rather than give up — this is the path
    // that makes a lost registry recoverable by re-picking the folder, so it has
    // to work without any plain-text help.
    let bytes = std::fs::read(dir.join("profile").join("snapshot.loro")).ok()?;
    let doc = ProfileDoc::from_snapshot(&bytes).ok()?;
    Some((
        doc.id()?,
        doc.name().unwrap_or_else(|| folder_name(dir)),
        doc.email(),
    ))
}

/// Read a profile's workspace list without opening a session — for `--where`.
pub fn peek_workspaces(dir: &Path) -> Vec<koru_core::WorkspaceEntry> {
    std::fs::read(dir.join("profile").join("snapshot.loro"))
        .ok()
        .and_then(|b| ProfileDoc::from_snapshot(&b).ok())
        .map(|d| d.workspaces())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
