// SPDX-License-Identifier: MPL-2.0

//! The staging inbox between a share extension and Koru — [ADR-137] on iOS,
//! [ADR-416] for the macOS port.
//!
//! **The extension is dumb and never opens the Store.** One SQLite root, one
//! iroh endpoint, one process: a share extension is a second process, so it
//! stages what was shared into a directory and the app imports it on its next
//! activation. That is the whole reason this format exists, and it is why
//! nothing here knows what a node, a workspace or a document is.
//!
//! ```text
//! <channel root>/share-inbox/
//!     <uuid>/                     one share
//!         <uuid>                  a payload, named by uuid, never by its title
//!         <uuid>
//!         manifest.json           written LAST — the commit marker
//! ```
//!
//! **`manifest.json` is written last, atomically, and that is the whole
//! crash-safety story.** An entry without one is an extension that died
//! mid-copy: invisible to a drain, and swept once it is old enough that no
//! extension could still be writing it. A half-written entry must never look
//! complete.
//!
//! # One format, two writers
//!
//! The Swift half is `apps/ios/KoruShared/ShareInbox.swift`, and the
//! macOS `KoruShare.appex` compiles *that same file* rather than a copy — see
//! `apps/macos/KoruShare/`. So the drift risk is between Swift and this file,
//! and `an_ios_manifest_decodes` is the pin: it is the literal fixture out of
//! `apps/ios/KoruMobileTests/ShareInboxTests.swift`.
//!
//! Three details that are load-bearing rather than decorative:
//!
//! - **The field names are Swift's**, camelCase, because Swift's synthesized
//!   `Codable` spells them that way and the format is Swift's first.
//! - **A `nil` optional is an absent key**, because Swift's synthesized
//!   encoder uses `encodeIfPresent`. `workspaceId` predates the picker and its
//!   absence is the format's whole versioning story, so it must decode.
//! - **`file` is a filename, never a path**, and this side is where that stops
//!   being a comment: [`Entry::payloads`] refuses a separator, a `..`, or
//!   anything that resolves outside its own entry directory. The writer is a
//!   different process; a reader that trusts it turns a buggy extension into a
//!   file-read primitive.
//!
//! [ADR-137]: ../../../docs/decisions/ADR-126-150.md
//! [ADR-416]: ../../../docs/decisions/ADR-416-macshare.md

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

/// The inbox directory's name, under the channel root beside `store/` and
/// `agent/`. The Swift half spells this too; both are pinned by
/// `the_inbox_sits_under_the_channel_root`.
pub const DIR_NAME: &str = "share-inbox";

/// The commit marker's name.
pub const MANIFEST_FILE: &str = "manifest.json";

/// How old an *uncommitted* entry has to be before it counts as debris rather
/// than an extension that is copying right now. iOS's number, kept identical
/// so a person reading one side is not surprised by the other.
pub const SWEEP_AFTER: Duration = Duration::from_secs(3600);

/// One shared payload, as the manifest records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct File {
    /// The payload's filename inside the entry directory. **Never a path** —
    /// [`Entry::payloads`] is what enforces that.
    pub file: String,
    /// The display name the import gives the document.
    pub name: String,
    /// What the sharer's process said the bytes are. Advisory: the importer
    /// derives its own from the name, and this is what tells it what to
    /// derive *for* (see `koru_import_files`).
    pub mime_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_size: Option<i64>,
}

/// The commit marker's contents.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub files: Vec<File>,
    /// The workspace the sharer chose, when the extension had a picker to
    /// choose in. **macOS never writes this today** — its extension has no
    /// picker and the desktop drain imports into the open workspace, exactly
    /// as a drag-and-drop does. It is carried, and round-trips, because iOS
    /// writes it and because one reader shape is the point of this file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
}

/// A committed entry, ready to import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The entry directory's name — a uuid, and the only id an entry has.
    pub id: String,
    pub directory: PathBuf,
    pub manifest: Manifest,
    /// When the entry directory was created, as the filesystem records it.
    /// Carried on the entry rather than kept beside it because
    /// [`drain_every_inbox`] merges several inboxes, and oldest-first has to
    /// survive that merge.
    pub staged_at: SystemTime,
}

impl Entry {
    /// The staged payloads, in manifest order, paired with the display name
    /// the manifest gave each.
    ///
    /// **Every name is checked before it becomes a path.** A `file` carrying a
    /// separator, a `..`, a drive letter or a leading `/` is refused, and so is
    /// one that does not exist — a manifest naming a payload that was never
    /// copied is a torn entry that reached `commit` anyway. The check is
    /// syntactic *and* positional: the joined path must still have this entry's
    /// directory as its parent.
    pub fn payloads(&self) -> Result<Vec<(PathBuf, &str)>, String> {
        let mut out = Vec::with_capacity(self.manifest.files.len());
        for file in &self.manifest.files {
            let name = file.file.as_str();
            if name.is_empty()
                || name == "."
                || name == ".."
                || name.contains('/')
                || name.contains('\\')
                || name.contains('\0')
            {
                return Err(format!(
                    "{} names a path rather than a file: {name:?}",
                    self.id
                ));
            }
            let path = self.directory.join(name);
            if path.parent() != Some(self.directory.as_path()) {
                return Err(format!("{} escapes its entry directory: {name:?}", self.id));
            }
            if !path.is_file() {
                return Err(format!(
                    "{} names a payload that is not there: {name:?}",
                    self.id
                ));
            }
            out.push((path, file.name.as_str()));
        }
        Ok(out)
    }

    /// Throw this entry away. An entry knows its own directory, so removal
    /// does not need the [`Inbox`] it came out of — which is what lets the
    /// drain merge several inboxes and hand the caller plain entries.
    pub fn discard(&self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

/// The macOS bundle identifier of the app the share extension lives in.
///
/// Here rather than in the packager because the *extension's* identifier is
/// derived from it, and the extension's identifier is how a sandboxed
/// extension's container is found — so the app has to be able to spell it too.
/// The packager reads these rather than keeping literals.
pub const DESKTOP_BUNDLE_ID: &str = "do.koru.desktop";

/// The share extension's bundle identifier, per channel.
///
/// **Two rules meet here.** Apple requires an app extension's identifier to be
/// prefixed by its containing app's, and five channels install side by side —
/// `pluginkit` keys a plugin by its identifier, so five copies sharing one
/// would be one registration and four losers. The app's own identifier is
/// *not* per-channel today and that question is deliberately still open
/// (docs/features/macos/RELEASING.md): changing it re-prompts every Gatekeeper and
/// LaunchServices decision already taken. The extension's is free to be
/// per-channel because nothing is deployed with it yet, so it is — every
/// channel, no special case for `stable`.
pub fn share_extension_bundle_id(segment: &str) -> String {
    format!("{DESKTOP_BUNDLE_ID}.share.{segment}")
}

/// What the sheet calls this row.
///
/// Five channels install side by side and every one of them is an app called
/// "Koru", so an undecorated row leaves nobody able to tell which build would
/// capture. `stable` is the product and reads as itself; the other four say so.
pub fn share_extension_display_name(segment: &str) -> String {
    if segment == "stable" {
        "Koru".into()
    } else {
        format!("Koru ({segment})")
    }
}

/// The inbox at one directory.
///
/// Constructed from a path rather than found, so every test has its own and
/// none of them can reach the developer's real one — the same discipline
/// `SearchReader` learned the hard way.
#[derive(Debug, Clone)]
pub struct Inbox {
    root: PathBuf,
}

/// This build's inbox: `<channel root>/share-inbox`.
///
/// `None` only when there is no home directory to resolve, which is a broken
/// environment rather than a state — every caller treats it as "nothing was
/// shared" rather than as an error, because a share sheet nobody used looks
/// identical.
pub fn for_this_channel() -> Option<Inbox> {
    koru_channel::channel_root().map(|root| Inbox::at(root.join(DIR_NAME)))
}

/// Every place a share extension for this build could have staged into.
///
/// **This is the one part of the port that is not in the plan, and it is here
/// because of a fact the plan asserts the other way.** §M9 reasons that a
/// Developer ID app is not sandboxed, so the extension writes to the channel
/// root directly and needs no App Group. The containing app is indeed not
/// sandboxed — but a macOS *app extension* is widely held to be sandboxed
/// regardless of its host, and that cannot be settled without a signed,
/// installed build. It decides where the bytes land: a sandboxed extension
/// asking Foundation for Application Support is silently answered with its own
/// container, `~/Library/Containers/<appex id>/Data/…`, and every share would
/// be staged somewhere the app never looked.
///
/// Rather than guess, **the extension asks Foundation and the app looks in
/// both places**. If the sandbox is off the two paths are the same directory
/// and the second candidate does not exist; if it is on, the app — which can
/// read anywhere the person can — finds the container. No entitlement, no App
/// Group, no Apple-account work, and no branch that is wrong on one of the two
/// answers. The cost is one failed `read_dir` per activation.
///
/// The app still *creates* only [`for_this_channel`], because that is the
/// candidate whose existence tells an unsandboxed extension which of macOS's
/// two channel roots this install actually uses.
pub fn every_inbox() -> Vec<Inbox> {
    let mut roots = Vec::new();
    if let Some(inbox) = for_this_channel() {
        roots.push(inbox);
    }
    if cfg!(target_os = "macos")
        && let Some(home) = std::env::var_os("HOME").map(PathBuf::from)
    {
        let segment = koru_channel::segment();
        let container = home
            .join("Library/Containers")
            .join(share_extension_bundle_id(segment))
            .join("Data/Library/Application Support/Koru")
            .join(segment)
            .join(DIR_NAME);
        if !roots.iter().any(|inbox| inbox.root() == container) {
            roots.push(Inbox::at(container));
        }
    }
    roots
}

/// Every committed entry across every candidate inbox, oldest first.
///
/// Entries carry their own directory, so a caller never has to remember which
/// inbox one came out of — [`Entry::discard`] is enough.
pub fn drain_every_inbox() -> Vec<Entry> {
    let mut entries: Vec<Entry> = every_inbox()
        .iter()
        .flat_map(|inbox| inbox.drain())
        .collect();
    // Merged across roots, so each inbox being sorted is not enough on its own.
    sort_oldest_first(&mut entries);
    entries
}

/// Oldest first, and the id breaks a tie — so two entries staged inside one
/// filesystem timestamp tick still drain in a defined order rather than in
/// `read_dir`'s.
fn sort_oldest_first(entries: &mut [Entry]) {
    entries.sort_by(|left, right| {
        left.staged_at
            .cmp(&right.staged_at)
            .then_with(|| left.id.cmp(&right.id))
    });
}

impl Inbox {
    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Create the directory if it is not there.
    ///
    /// **The app calls this at startup, and that is not tidiness.** The
    /// extension resolves the channel root by asking which candidate already
    /// holds a `share-inbox` (macOS keeps a sticky legacy XDG root — see
    /// `koru_channel::channel_root_of`), so the app creating it is what makes
    /// the extension's answer agree with the app's. An inbox nothing ever
    /// drains is worse than no inbox at all.
    pub fn ensure(&self) -> io::Result<()> {
        fs::create_dir_all(&self.root)
    }

    /// Every committed entry, oldest first — and, as a side effect, the sweep.
    pub fn drain(&self) -> Vec<Entry> {
        self.drain_at(SystemTime::now())
    }

    /// [`drain`](Self::drain) with the clock passed in, so the sweep is
    /// testable without waiting an hour.
    pub fn drain_at(&self, now: SystemTime) -> Vec<Entry> {
        let Ok(listing) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut entries: Vec<Entry> = Vec::new();
        for item in listing.flatten() {
            let directory = item.path();
            if !directory.is_dir() {
                continue;
            }
            // `created` is what iOS orders and sweeps by; not every filesystem
            // carries one, so `modified` is the fallback and the epoch is the
            // last resort — an entry whose age cannot be read is never swept,
            // because deleting somebody's shared file on a guess is the one
            // outcome this whole design exists to avoid.
            let stamped = fs::metadata(&directory)
                .and_then(|meta| meta.created().or_else(|_| meta.modified()))
                .ok();
            let manifest = fs::read(directory.join(MANIFEST_FILE))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Manifest>(&bytes).ok());
            let Some(manifest) = manifest else {
                let old = stamped
                    .is_some_and(|at| now.duration_since(at).is_ok_and(|age| age > SWEEP_AFTER));
                if old {
                    let _ = fs::remove_dir_all(&directory);
                }
                continue;
            };
            let id = directory
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            entries.push(Entry {
                id,
                directory,
                manifest,
                staged_at: stamped.unwrap_or(SystemTime::UNIX_EPOCH),
            });
        }
        sort_oldest_first(&mut entries);
        entries
    }

    /// Remove one entry — after a successful import, or an explicit discard.
    /// **Never after a failed one**: the entry is the only copy of what was
    /// shared, and the next activation is a free retry.
    pub fn remove(&self, entry: &Entry) {
        entry.discard();
    }

    // ------------------------------------------------------------ the writer

    /// A directory to stage one share's payloads into. Nothing in it is
    /// visible to a drain until [`commit`](Self::commit) writes the manifest.
    ///
    /// Rust writes no share today — the extensions do, in Swift. This half
    /// exists so the tests build entries through the same door the format
    /// documents, rather than through hand-rolled `fs::write` calls that would
    /// pass while disagreeing with it.
    pub fn begin(&self) -> io::Result<PathBuf> {
        let entry = self.root.join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&entry)?;
        Ok(entry)
    }

    /// The commit marker. Written last, and written whole.
    ///
    /// `serde_json::to_vec_pretty` sorts nothing, and Swift's encoder is
    /// configured `.sortedKeys` — that is a difference in *bytes* and not in
    /// meaning, and the reader on either side is a JSON parser. What both
    /// sides must agree on is that this file appears after every payload does.
    pub fn commit(&self, entry: &Path, manifest: &Manifest) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(manifest)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        // Write beside and rename: a reader that catches a half-written
        // manifest.json sees a torn entry as a *committed* one.
        let staging = entry.join("manifest.json.part");
        fs::write(&staging, bytes)?;
        fs::rename(staging, entry.join(MANIFEST_FILE))
    }
}

#[cfg(test)]
mod tests;
